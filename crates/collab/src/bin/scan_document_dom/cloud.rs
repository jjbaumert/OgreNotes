// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Read-only, bounded storage access for the diagnostic binary. No repository
//! loaders are used: they can accumulate an entire operation log before limits
//! are checked, and metadata scans can silently omit malformed rows.

use super::args::Args;
use aws_sdk_dynamodb::{Client as DynamoClient, types::AttributeValue};
use aws_sdk_s3::{Client as S3Client, error::ProvideErrorMetadata};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

pub(super) type Item = HashMap<String, AttributeValue>;

#[derive(Clone, Debug, Serialize)]
pub(super) struct BlobIdentity {
    pub key: String,
    pub sha256: String,
    pub etag: Option<String>,
    pub version_id: Option<String>,
    pub size: usize,
}

pub(super) struct Blob {
    pub bytes: Vec<u8>,
    pub identity: BlobIdentity,
}

pub(super) struct StoredUpdate {
    pub clock: String,
    pub user_id: Option<String>,
    pub client_version: Option<String>,
    pub created_at: Option<String>,
    pub bytes: Vec<u8>,
    pub blob: Option<BlobIdentity>,
}

pub(super) struct UpdateSet {
    pub rows: Vec<StoredUpdate>,
    pub fingerprint: String,
    pub bytes: usize,
}

#[derive(Clone, Serialize)]
pub(super) struct HistoricalSnapshot {
    pub key: String,
    pub version: Option<u64>,
    pub user_id: Option<String>,
    pub created_at: Option<String>,
    pub indexed: bool,
}

pub(super) struct Cloud {
    ddb: DynamoClient,
    s3: S3Client,
    table: String,
    bucket: String,
    page_size: i32,
    max_bytes: usize,
    max_rows: usize,
    body_timeout: std::time::Duration,
}

impl Cloud {
    pub fn new(ddb: DynamoClient, s3: S3Client, args: &Args) -> Self {
        Self {
            ddb,
            s3,
            table: args.table.clone(),
            bucket: args.bucket.clone(),
            page_size: args.page_size,
            max_bytes: args.max_bytes,
            max_rows: args.max_rows,
            body_timeout: std::time::Duration::from_secs(args.timeout_seconds as u64),
        }
    }

    pub async fn scan_page(
        &self,
        start: Option<Item>,
    ) -> Result<(Vec<Item>, Option<Item>), String> {
        // The limit counts evaluated items, not returned matches. An empty page
        // with a continuation key is not exhaustion.
        let result = self.ddb.scan()
            .table_name(&self.table)
            .consistent_read(true)
            .limit(self.page_size)
            .filter_expression("begins_with(#pk, :pk) AND #sk = :sk")
            .expression_attribute_names("#pk", "PK")
            .expression_attribute_names("#sk", "SK")
            .expression_attribute_names("#type", "doc_type")
            .projection_expression("#pk, #sk, doc_id, #type, snapshot_s3_key, snapshot_version, is_deleted, deleted_at, updated_at, created_at, owner_id")
            .expression_attribute_values(":pk", AttributeValue::S("DOC#".into()))
            .expression_attribute_values(":sk", AttributeValue::S("METADATA".into()))
            .set_exclusive_start_key(start.clone())
            .send().await.map_err(|e| service_error("DynamoDB metadata Scan", e.as_service_error().and_then(|e| e.code())))?;
        let next = nonempty_key(result.last_evaluated_key);
        check_cursor(&start, &next)?;
        Ok((result.items.unwrap_or_default(), next))
    }

    pub async fn metadata(&self, doc_id: &str) -> Result<Option<Item>, String> {
        self.ddb
            .get_item()
            .table_name(&self.table)
            .key("PK", AttributeValue::S(format!("DOC#{doc_id}")))
            .key("SK", AttributeValue::S("METADATA".into()))
            .consistent_read(true)
            .send()
            .await
            .map(|r| r.item)
            .map_err(|e| {
                service_error(
                    "DynamoDB metadata GetItem",
                    e.as_service_error().and_then(|e| e.code()),
                )
            })
    }

    async fn query_page(
        &self,
        doc_id: &str,
        prefix: &str,
        start: Option<Item>,
    ) -> Result<(Vec<Item>, Option<Item>), String> {
        let result = self
            .ddb
            .query()
            .table_name(&self.table)
            .consistent_read(true)
            .scan_index_forward(true)
            .limit(self.page_size)
            .key_condition_expression("#pk = :pk AND begins_with(#sk, :sk)")
            .expression_attribute_names("#pk", "PK")
            .expression_attribute_names("#sk", "SK")
            .expression_attribute_values(":pk", AttributeValue::S(format!("DOC#{doc_id}")))
            .expression_attribute_values(":sk", AttributeValue::S(prefix.into()))
            .set_exclusive_start_key(start.clone())
            .send()
            .await
            .map_err(|e| {
                service_error(
                    "DynamoDB document Query",
                    e.as_service_error().and_then(|e| e.code()),
                )
            })?;
        let next = nonempty_key(result.last_evaluated_key);
        check_cursor(&start, &next)?;
        Ok((result.items.unwrap_or_default(), next))
    }

    pub async fn updates(&self, doc_id: &str, budget: usize) -> Result<UpdateSet, String> {
        let budget = budget.min(self.max_bytes);
        let mut result = UpdateSet {
            rows: Vec::new(),
            fingerprint: String::new(),
            bytes: 0,
        };
        let mut fingerprint = Sha256::new();
        let mut start = None;
        let mut pages = 0;
        loop {
            pages += 1;
            self.check_pages(pages)?;
            let (items, next) = self.query_page(doc_id, "UPDATE#", start).await?;
            for item in items {
                self.check_rows(result.rows.len().saturating_add(1))?;
                fingerprint.update(item_digest(&item));
                let clock = required_s(&item, "SK")?
                    .strip_prefix("UPDATE#")
                    .filter(|s| !s.is_empty())
                    .ok_or("invalid update sort key")?
                    .to_string();
                let user_id = optional_s(&item, "user_id")?;
                let client_version = optional_s(&item, "client_version")?;
                let created_at = optional_n(&item, "created_at")?;
                let remaining = budget.saturating_sub(result.bytes);
                // Match the production loader: an S3 pointer is authoritative
                // if a migration row happens to carry both representations.
                let (bytes, blob) = if let Some(key) = optional_s(&item, "update_s3_key")? {
                    let blob = self.blob_with_limit(&key, remaining).await?;
                    (blob.bytes, Some(blob.identity))
                } else {
                    let value = item
                        .get("update_bytes")
                        .and_then(|v| v.as_b().ok())
                        .ok_or("update row has no binary payload or S3 pointer")?;
                    if value.as_ref().len() > remaining {
                        return Err("update byte limit exceeded".into());
                    }
                    (value.as_ref().to_vec(), None)
                };
                result.bytes += bytes.len();
                result.rows.push(StoredUpdate {
                    clock,
                    user_id,
                    client_version,
                    created_at,
                    bytes,
                    blob,
                });
            }
            match next {
                Some(key) => start = Some(key),
                None => break,
            }
        }
        result.fingerprint = format!("{:x}", fingerprint.finalize());
        Ok(result)
    }

    pub async fn update_identity(&self, doc_id: &str) -> Result<String, String> {
        let mut fingerprint = Sha256::new();
        let mut count = 0usize;
        let mut pages = 0usize;
        let mut start = None;
        loop {
            pages += 1;
            self.check_pages(pages)?;
            let (items, next) = self.query_page(doc_id, "UPDATE#", start).await?;
            for item in items {
                count = count.saturating_add(1);
                self.check_rows(count)?;
                fingerprint.update(item_digest(&item));
            }
            match next {
                Some(key) => start = Some(key),
                None => break,
            }
        }
        Ok(format!("{:x}", fingerprint.finalize()))
    }

    pub async fn blob(&self, key: &str) -> Result<Blob, String> {
        self.blob_with_limit(key, self.max_bytes).await
    }

    pub async fn blob_with_limit(&self, key: &str, limit: usize) -> Result<Blob, String> {
        let mut response = self
            .s3
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                service_error("S3 GetObject", e.as_service_error().and_then(|e| e.code()))
            })?;
        let length = response
            .content_length()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("S3 object has no valid content length")?;
        if length > limit {
            return Err("S3 object byte limit exceeded".into());
        }
        let etag = response
            .e_tag()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let version_id = response
            .version_id()
            .filter(|s| !s.is_empty() && *s != "null")
            .map(str::to_owned);
        // Do not trust a small header to bound an unexpectedly large body.
        let mut bytes = Vec::new();
        // SDK operation deadlines end when GetObject returns its headers.
        // Bound the entire streamed body too, including a stalled connection.
        tokio::time::timeout(self.body_timeout, async {
            while let Some(chunk) = response.body.next().await {
                let chunk = chunk.map_err(|_| "S3 object body read failed".to_string())?;
                if chunk.len() > limit.saturating_sub(bytes.len()) {
                    return Err("S3 object byte limit exceeded".to_string());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(())
        })
        .await
        .map_err(|_| "S3 object body deadline exceeded".to_string())??;
        if bytes.len() != length {
            return Err("S3 object length changed or body was truncated".into());
        }
        let identity = BlobIdentity {
            key: key.to_owned(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            etag,
            version_id,
            size: bytes.len(),
        };
        Ok(Blob { bytes, identity })
    }

    pub async fn unchanged_blob(&self, identity: &BlobIdentity) -> Result<bool, String> {
        if identity.etag.is_none() && identity.version_id.is_none() {
            return Err("S3 object has no ETag or immutable version for stability check".into());
        }
        // HEAD the current key, not the captured version: a newer version means
        // the current durable state changed while it was being inspected.
        let head = self
            .s3
            .head_object()
            .bucket(&self.bucket)
            .key(&identity.key)
            .send()
            .await
            .map_err(|e| {
                service_error(
                    "S3 HeadObject stability check",
                    e.as_service_error().and_then(|e| e.code()),
                )
            })?;
        let same_size =
            head.content_length().and_then(|n| usize::try_from(n).ok()) == Some(identity.size);
        let same_etag = identity
            .etag
            .as_deref()
            .is_none_or(|tag| head.e_tag() == Some(tag));
        let same_version = identity
            .version_id
            .as_deref()
            .is_none_or(|version| head.version_id() == Some(version));
        Ok(same_size && same_etag && same_version)
    }

    pub async fn history(&self, doc_id: &str) -> Result<Vec<HistoricalSnapshot>, String> {
        let mut snapshots = BTreeMap::new();
        let mut count = 0usize;
        let mut pages = 0usize;
        let mut start = None;
        loop {
            pages += 1;
            self.check_pages(pages)?;
            let (items, next) = self.query_page(doc_id, "SNAPSHOT#", start).await?;
            for item in items {
                count = count.saturating_add(1);
                self.check_rows(count)?;
                let key = required_s(&item, "s3_key")?.to_owned();
                let version = optional_n(&item, "version")?
                    .map(|n| n.parse::<u64>())
                    .transpose()
                    .map_err(|_| "invalid snapshot version")?;
                let snapshot = HistoricalSnapshot {
                    key: key.clone(),
                    version,
                    user_id: optional_s(&item, "user_id")?,
                    created_at: optional_n(&item, "created_at")?,
                    indexed: true,
                };
                // Two history records pointing to one blob have ambiguous
                // provenance; do not silently overwrite the first attribution.
                if snapshots.insert(key, snapshot).is_some() {
                    return Err("multiple snapshot history rows refer to one object".into());
                }
            }
            match next {
                Some(key) => start = Some(key),
                None => break,
            }
        }
        let prefix = format!("docs/{doc_id}/snapshots/");
        let mut continuation: Option<String> = None;
        let mut listed = 0usize;
        pages = 0;
        loop {
            pages += 1;
            self.check_pages(pages)?;
            let result = self
                .s3
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .max_keys(self.page_size)
                .set_continuation_token(continuation.clone())
                .send()
                .await
                .map_err(|e| {
                    service_error(
                        "S3 snapshot ListObjectsV2",
                        e.as_service_error().and_then(|e| e.code()),
                    )
                })?;
            for object in result.contents() {
                listed = listed.saturating_add(1);
                self.check_rows(listed)?;
                let key = object
                    .key()
                    .ok_or("S3 snapshot listing has no object key")?;
                if !key.ends_with(".bin") {
                    continue;
                }
                if !snapshots.contains_key(key) {
                    self.check_rows(snapshots.len().saturating_add(1))?;
                    let version = key
                        .strip_prefix(&prefix)
                        .and_then(|s| s.strip_suffix(".bin"))
                        .and_then(|s| s.parse::<u64>().ok());
                    snapshots.insert(
                        key.to_owned(),
                        HistoricalSnapshot {
                            key: key.to_owned(),
                            version,
                            user_id: None,
                            created_at: None,
                            indexed: false,
                        },
                    );
                }
            }
            if result.is_truncated() != Some(true) {
                break;
            }
            let next = result
                .next_continuation_token()
                .filter(|s| !s.is_empty())
                .ok_or("S3 truncated listing has no continuation token")?
                .to_owned();
            if continuation.as_ref() == Some(&next) {
                return Err("S3 listing repeated continuation token".into());
            }
            continuation = Some(next);
        }
        let mut snapshots: Vec<_> = snapshots.into_values().collect();
        snapshots.sort_by(|a, b| a.version.cmp(&b.version).then_with(|| a.key.cmp(&b.key)));
        Ok(snapshots)
    }

    fn check_pages(&self, pages: usize) -> Result<(), String> {
        // Allow an empty terminal page after exactly max_rows returned rows.
        if pages > self.max_rows.saturating_add(1) {
            Err("document page limit exceeded".into())
        } else {
            Ok(())
        }
    }

    fn check_rows(&self, count: usize) -> Result<(), String> {
        if count > self.max_rows {
            Err("document row/page limit exceeded".into())
        } else {
            Ok(())
        }
    }
}

fn optional_s(item: &Item, field: &str) -> Result<Option<String>, String> {
    item.get(field)
        .map(|v| {
            v.as_s()
                .cloned()
                .map_err(|_| format!("invalid string field: {field}"))
        })
        .transpose()
}

fn required_s<'a>(item: &'a Item, field: &str) -> Result<&'a str, String> {
    item.get(field)
        .and_then(|v| v.as_s().ok())
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing or invalid string field: {field}"))
}

fn optional_n(item: &Item, field: &str) -> Result<Option<String>, String> {
    item.get(field)
        .map(|v| {
            let value = v
                .as_n()
                .map_err(|_| format!("invalid numeric field: {field}"))?;
            value
                .parse::<i128>()
                .map_err(|_| format!("invalid integer field: {field}"))?;
            Ok(value.clone())
        })
        .transpose()
}

fn nonempty_key(key: Option<Item>) -> Option<Item> {
    key.filter(|item| !item.is_empty())
}

fn check_cursor(previous: &Option<Item>, next: &Option<Item>) -> Result<(), String> {
    if let (Some(a), Some(b)) = (previous, next)
        && metadata_identity(a) == metadata_identity(b)
    {
        return Err("DynamoDB repeated pagination cursor".into());
    }
    Ok(())
}

pub(super) fn metadata_identity(item: &Item) -> String {
    format!("{:x}", item_digest(item))
}

fn item_digest(item: &Item) -> sha2::digest::Output<Sha256> {
    let mut hash = Sha256::new();
    hash_map(&mut hash, item);
    hash.finalize()
}

fn part(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

fn hash_map(hash: &mut Sha256, item: &Item) {
    hash.update((item.len() as u64).to_be_bytes());
    for (name, value) in item.iter().collect::<BTreeMap<_, _>>() {
        part(hash, name.as_bytes());
        hash_value(hash, value);
    }
}

// Hash every attribute, with type tags and lengths. Map/set iteration order
// cannot manufacture a false drift report. Binary data is never formatted or
// copied, and neither content nor raw metadata appears in diagnostic errors.
fn hash_value(hash: &mut Sha256, value: &AttributeValue) {
    match value {
        AttributeValue::B(v) => {
            part(hash, b"B");
            part(hash, v.as_ref());
        }
        AttributeValue::Bool(v) => {
            part(hash, b"Bool");
            part(hash, &[*v as u8]);
        }
        AttributeValue::N(v) => {
            part(hash, b"N");
            part(hash, v.as_bytes());
        }
        AttributeValue::S(v) => {
            part(hash, b"S");
            part(hash, v.as_bytes());
        }
        AttributeValue::Null(v) => {
            part(hash, b"Null");
            part(hash, &[*v as u8]);
        }
        AttributeValue::M(v) => {
            part(hash, b"M");
            hash_map(hash, v);
        }
        AttributeValue::L(v) => {
            part(hash, b"L");
            hash.update((v.len() as u64).to_be_bytes());
            for item in v {
                hash_value(hash, item);
            }
        }
        AttributeValue::Ss(v) | AttributeValue::Ns(v) => {
            part(
                hash,
                if matches!(value, AttributeValue::Ss(_)) {
                    b"SS"
                } else {
                    b"NS"
                },
            );
            let mut values: Vec<_> = v.iter().collect();
            values.sort();
            hash.update((values.len() as u64).to_be_bytes());
            for item in values {
                part(hash, item.as_bytes());
            }
        }
        AttributeValue::Bs(v) => {
            part(hash, b"BS");
            let mut values: Vec<&[u8]> = v.iter().map(|blob| blob.as_ref()).collect();
            values.sort();
            hash.update((values.len() as u64).to_be_bytes());
            for item in values {
                part(hash, item);
            }
        }
        _ => part(hash, b"unknown SDK attribute variant"),
    }
}

// Service codes identify missing objects, permission failures and throttling
// without copying service messages that may contain document data or URLs.
fn service_error(operation: &str, code: Option<&str>) -> String {
    let code = code
        .filter(|value| {
            value.len() <= 80
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .unwrap_or("transport_or_unclassified");
    format!("{operation} failed ({code})")
}
