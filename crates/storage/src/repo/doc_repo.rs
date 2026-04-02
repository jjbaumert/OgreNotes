use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::document::{DocUpdate, DocumentMeta};
use crate::models::DocType;
use crate::repo::{RepoError, get_s, get_n, get_n_u64};
use crate::s3::S3Client;

/// Repository for document operations.
pub struct DocRepo {
    db: DynamoClient,
    s3: S3Client,
}

impl DocRepo {
    pub fn new(db: DynamoClient, s3: S3Client) -> Self {
        Self { db, s3 }
    }

    /// Access the S3 client (for presigned URL generation in blob routes).
    pub fn s3(&self) -> &S3Client {
        &self.s3
    }

    /// Update snapshot metadata with a condition expression (for optimistic locking).
    pub async fn conditional_update_snapshot(
        &self,
        pk: &str,
        update_expression: &str,
        condition_expression: &str,
        expression_values: std::collections::HashMap<String, aws_sdk_dynamodb::types::AttributeValue>,
    ) -> Result<(), RepoError> {
        self.db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", aws_sdk_dynamodb::types::AttributeValue::S(pk.to_string()))
            .key("SK", aws_sdk_dynamodb::types::AttributeValue::S("METADATA".to_string()))
            .update_expression(update_expression)
            .condition_expression(condition_expression)
            .set_expression_attribute_values(Some(expression_values))
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;
        Ok(())
    }

    /// Create a new document with an empty initial snapshot.
    /// Writes DynamoDB metadata first (conditional), then S3 snapshot.
    pub async fn create(
        &self,
        meta: &DocumentMeta,
        initial_snapshot: &[u8],
    ) -> Result<(), RepoError> {
        // Write metadata to DynamoDB first (conditional to prevent duplicates)
        let mut item = doc_meta_to_item(meta);
        item.insert("PK".to_string(), AttributeValue::S(meta.pk()));
        item.insert("SK".to_string(), AttributeValue::S(DocumentMeta::sk().to_string()));
        item.insert("owner_id_gsi".to_string(), AttributeValue::S(meta.owner_id.clone()));

        self.db
            .put_item_conditional(item, "attribute_not_exists(PK)")
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        // Then write snapshot to S3
        self.s3
            .put_object(&meta.snapshot_key(), initial_snapshot.to_vec())
            .await
            .map_err(|e| RepoError::S3(e.to_string()))
    }

    /// Get document metadata by ID.
    pub async fn get(&self, doc_id: &str) -> Result<Option<DocumentMeta>, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let item = self
            .db
            .get_item(&pk, DocumentMeta::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(doc_meta_from_item(&item)?)),
            None => Ok(None),
        }
    }

    /// Update document metadata (title, updated_at).
    pub async fn update_metadata(
        &self,
        doc_id: &str,
        title: Option<&str>,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let mut expr_parts = vec!["updated_at = :updated_at".to_string()];
        let mut values = HashMap::new();

        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));

        if let Some(t) = title {
            expr_parts.push("title = :title".to_string());
            values.insert(":title".to_string(), AttributeValue::S(t.to_string()));
        }

        let update_expr = format!("SET {}", expr_parts.join(", "));

        self.db
            .update_item(&pk, DocumentMeta::sk(), &update_expr, values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Soft delete a document.
    pub async fn soft_delete(&self, doc_id: &str, deleted_at: i64) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let mut values = HashMap::new();
        values.insert(":is_deleted".to_string(), AttributeValue::Bool(true));
        values.insert(":deleted_at".to_string(), AttributeValue::N(deleted_at.to_string()));
        values.insert(":updated_at".to_string(), AttributeValue::N(deleted_at.to_string()));

        self.db
            .update_item(
                &pk,
                DocumentMeta::sk(),
                "SET is_deleted = :is_deleted, deleted_at = :deleted_at, updated_at = :updated_at",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Save a new snapshot to S3 and update metadata.
    /// Writes DynamoDB metadata first, then S3 snapshot.
    pub async fn save_snapshot(
        &self,
        doc_id: &str,
        snapshot: &[u8],
        new_version: u64,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let s3_key = format!("docs/{doc_id}/snapshots/{new_version}.bin");

        // Update metadata in DynamoDB first
        let pk = format!("DOC#{doc_id}");
        let mut values = HashMap::new();
        values.insert(":snapshot_version".to_string(), AttributeValue::N(new_version.to_string()));
        values.insert(":snapshot_s3_key".to_string(), AttributeValue::S(s3_key.clone()));
        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));

        self.db
            .update_item(
                &pk,
                DocumentMeta::sk(),
                "SET snapshot_version = :snapshot_version, snapshot_s3_key = :snapshot_s3_key, updated_at = :updated_at",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        // Then write snapshot to S3
        self.s3
            .put_object(&s3_key, snapshot.to_vec())
            .await
            .map_err(|e| RepoError::S3(e.to_string()))
    }

    /// Load the latest snapshot from S3.
    pub async fn load_snapshot(&self, doc_id: &str) -> Result<Option<Vec<u8>>, RepoError> {
        let meta = self.get(doc_id).await?;
        match meta {
            Some(m) => match m.snapshot_s3_key {
                Some(key) => {
                    let data = self
                        .s3
                        .get_object(&key)
                        .await
                        .map_err(|e| RepoError::S3(e.to_string()))?;
                    Ok(Some(data))
                }
                None => Ok(None),
            },
            None => Ok(None),
        }
    }

    /// Append a CRDT update to the op log (conditional to prevent duplicate writes).
    pub async fn append_update(&self, update: &DocUpdate) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(update.pk()));
        item.insert("SK".to_string(), AttributeValue::S(update.sk()));
        item.insert(
            "update_bytes".to_string(),
            AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(update.update_bytes.clone())),
        );
        item.insert("user_id".to_string(), AttributeValue::S(update.user_id.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(update.created_at.to_string()));

        self.db
            .put_item_conditional(item, "attribute_not_exists(PK) AND attribute_not_exists(SK)")
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get pending updates for a document (after the last snapshot).
    pub async fn get_pending_updates(&self, doc_id: &str) -> Result<Vec<DocUpdate>, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let items = self
            .db
            .query(&pk, Some("UPDATE#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items
            .iter()
            .map(|item| {
                let sk = get_s(item, "SK")?;
                let clock = sk
                    .strip_prefix("UPDATE#")
                    .ok_or_else(|| RepoError::MissingField(format!("SK missing UPDATE# prefix: {sk}")))?
                    .to_string();

                let update_bytes = item
                    .get("update_bytes")
                    .and_then(|v| v.as_b().ok())
                    .map(|b| b.as_ref().to_vec())
                    .ok_or_else(|| RepoError::MissingField("update_bytes".to_string()))?;

                Ok(DocUpdate {
                    doc_id: doc_id.to_string(),
                    clock,
                    update_bytes,
                    user_id: get_s(item, "user_id")?,
                    created_at: get_n(item, "created_at")?,
                })
            })
            .collect()
    }
    /// Delete UPDATE# rows for a document that were created before `before_usec`.
    /// Used after compaction snapshots to prune only updates included in the snapshot.
    pub async fn delete_updates_before(
        &self,
        doc_id: &str,
        before_usec: i64,
    ) -> Result<usize, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let items = self
            .db
            .query(&pk, Some("UPDATE#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        let mut count = 0;
        for item in &items {
            // Only delete if created_at < before_usec
            let created_at = item
                .get("created_at")
                .and_then(|v| v.as_n().ok())
                .and_then(|n| n.parse::<i64>().ok())
                .unwrap_or(i64::MAX);
            if created_at >= before_usec {
                continue;
            }
            if let Some(sk) = item.get("SK").and_then(|v| v.as_s().ok()) {
                self.db
                    .delete_item(&pk, sk)
                    .await
                    .map_err(|e| RepoError::Dynamo(e.to_string()))?;
                count += 1;
            }
        }
        Ok(count)
    }
}

fn doc_meta_to_item(meta: &DocumentMeta) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert("doc_id".to_string(), AttributeValue::S(meta.doc_id.clone()));
    item.insert("title".to_string(), AttributeValue::S(meta.title.clone()));
    item.insert("owner_id".to_string(), AttributeValue::S(meta.owner_id.clone()));
    if let Some(ref fid) = meta.folder_id {
        item.insert("folder_id".to_string(), AttributeValue::S(fid.clone()));
    }
    item.insert(
        "doc_type".to_string(),
        AttributeValue::S(serde_json::to_string(&meta.doc_type).unwrap().trim_matches('"').to_string()),
    );
    item.insert("snapshot_version".to_string(), AttributeValue::N(meta.snapshot_version.to_string()));
    if let Some(ref key) = meta.snapshot_s3_key {
        item.insert("snapshot_s3_key".to_string(), AttributeValue::S(key.clone()));
    }
    item.insert("is_deleted".to_string(), AttributeValue::Bool(meta.is_deleted));
    if let Some(deleted_at) = meta.deleted_at {
        item.insert("deleted_at".to_string(), AttributeValue::N(deleted_at.to_string()));
    }
    item.insert("created_at".to_string(), AttributeValue::N(meta.created_at.to_string()));
    item.insert("updated_at".to_string(), AttributeValue::N(meta.updated_at.to_string()));
    item
}

fn doc_meta_from_item(item: &HashMap<String, AttributeValue>) -> Result<DocumentMeta, RepoError> {
    let doc_type_str = get_s(item, "doc_type")?;
    let doc_type: DocType = serde_json::from_str(&format!("\"{doc_type_str}\""))
        .map_err(|e| RepoError::MissingField(format!("doc_type: {e}")))?;

    Ok(DocumentMeta {
        doc_id: get_s(item, "doc_id")?,
        title: get_s(item, "title")?,
        owner_id: get_s(item, "owner_id")?,
        folder_id: item.get("folder_id").and_then(|v| v.as_s().ok()).cloned(),
        doc_type,
        snapshot_version: get_n_u64(item, "snapshot_version")?,
        snapshot_s3_key: item.get("snapshot_s3_key").and_then(|v| v.as_s().ok()).cloned(),
        is_deleted: item.get("is_deleted").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false),
        deleted_at: item.get("deleted_at").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<i64>().ok()),
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
}
