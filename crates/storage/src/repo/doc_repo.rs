// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::document::{
    Collection, CollectionItem, DocMember, DocOpen, DocRelationship, DocUpdate, DocumentMeta,
    Favorite, RelationType,
};
use crate::models::snapshot::DocSnapshot;
use crate::models::{AccessLevel, DocType};
use crate::repo::snapshot_repo::SnapshotRepo;
use crate::repo::{RepoError, get_s, get_n, get_n_u64};
use crate::s3::S3Client;

/// M-E7 item 9 — single partition value for the sparse
/// `is_deleted_gsi` HASH key on `GSI7-deleted-at`. All soft-deleted
/// docs share this partition; the trash-cleanup worker queries with
/// the literal "deleted" PK and ranges over `deleted_at`. At current
/// scale a single partition is fine; if write rate ever pushes it
/// past the DDB-per-partition WCU/RCU ceiling, the right fix is
/// date-bucketing (e.g. `deleted-202605`) which keeps the same
/// query shape but distributes across multiple partitions.
const IS_DELETED_GSI_PARTITION: &str = "deleted";

/// Threshold for inline storage of a `DocUpdate.update_bytes` Blob
/// on the DDB `UPDATE#` row. DynamoDB hard-caps a single item at
/// 400 KB (sum of attribute names + values + binary-encoding
/// overhead); above this threshold we PutObject the bytes to S3 and
/// store only a pointer (`update_s3_key`) on the DDB row, so single
/// updates of any size (e.g. a multi-MB paste) persist cleanly. The
/// inline path remains the steady-state choice — most live updates
/// from a keystroke or short edit are well under 1 KB.
///
/// Sized at 256 KiB to leave ~140 KB of headroom for PK/SK/clock,
/// user_id, created_at, client_version, attribute-name overhead and
/// DDB's internal encoding margin. (#38)
pub const UPDATE_INLINE_MAX_BYTES: usize = 256 * 1024;

/// Outcome of an optimistic-locked snapshot write
/// ([`DocRepo::save_snapshot_conditional`]).
///
/// A version conflict is a normal, expected outcome (a concurrent
/// writer won the race), not a storage error — so it's surfaced as a
/// value the caller matches on rather than an `Err` the caller has to
/// string-match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotWrite {
    /// The conditional update succeeded — `snapshot_version` was bumped.
    Committed,
    /// Lost the optimistic-lock race: another writer already advanced
    /// `snapshot_version`. The caller should surface a 409 Conflict.
    VersionConflict,
}

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
    /// Writes DynamoDB metadata first (conditional), then S3 snapshot,
    /// then a SNAPSHOT# row pointing at the v=1 blob so the new doc
    /// appears in the edit-history pane immediately.
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
        if let Some(ws) = &meta.workspace_id {
            item.insert("workspace_id_gsi".to_string(), AttributeValue::S(ws.clone()));
        }

        self.db
            .put_item_conditional(item, "attribute_not_exists(PK)")
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        // Then write snapshot to S3
        let s3_key = meta.snapshot_key();
        self.s3
            .put_object(&s3_key, initial_snapshot.to_vec())
            .await
            .map_err(|e| RepoError::S3(e.to_string()))?;

        // Record a SNAPSHOT# row for the initial v=meta.snapshot_version
        // (almost always 1). Spawn rather than await — the row is purely
        // for the edit-history pane and is not load-bearing for any
        // synchronous client behavior. Awaiting it doubles the DDB
        // round-trip on the create_doc hot path and, under CI parallel
        // load, has been observed to surface ECONNRESET / throttling
        // from DynamoDB Local that turn create_doc into a 500.
        let snap = DocSnapshot {
            doc_id: meta.doc_id.clone(),
            version: meta.snapshot_version,
            s3_key,
            size_bytes: initial_snapshot.len() as u64,
            user_id: meta.owner_id.clone(),
            created_at: meta.created_at,
        };
        let db = self.db.clone();
        let doc_id_for_log = meta.doc_id.clone();
        let version_for_log = meta.snapshot_version;
        tokio::spawn(async move {
            if let Err(e) = SnapshotRepo::new(db).create(&snap).await {
                tracing::warn!(
                    doc_id = %doc_id_for_log,
                    version = version_for_log,
                    error = %e,
                    "create: initial SNAPSHOT# row write failed",
                );
            }
        });
        Ok(())
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

    /// Update link sharing settings on a document.
    pub async fn update_link_settings(
        &self,
        doc_id: &str,
        link_sharing_mode: Option<&crate::models::LinkSharingMode>,
        link_view_options: Option<&crate::models::ViewOptions>,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let mut set_parts = vec!["updated_at = :updated_at".to_string()];
        let mut remove_parts: Vec<String> = vec![];
        let mut values = HashMap::new();
        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));

        if let Some(mode) = link_sharing_mode {
            set_parts.push("link_sharing_mode = :mode".to_string());
            values.insert(
                ":mode".to_string(),
                AttributeValue::S(
                    serde_json::to_string(mode).unwrap().trim_matches('"').to_string(),
                ),
            );
        }

        if let Some(opts) = link_view_options {
            if *opts == crate::models::ViewOptions::default() {
                // All flags off → REMOVE the attribute so the row stays
                // consistent with the write-when-non-default invariant in
                // doc_meta_to_item; absence decodes to the default on read
                // (same path as a legacy row). Without this, a reset would
                // leave a stale all-false attribute behind.
                remove_parts.push("link_view_options".to_string());
            } else {
                set_parts.push("link_view_options = :vo".to_string());
                values.insert(
                    ":vo".to_string(),
                    AttributeValue::S(serde_json::to_string(opts).unwrap()),
                );
            }
        }

        let mut update_expr = format!("SET {}", set_parts.join(", "));
        if !remove_parts.is_empty() {
            update_expr.push_str(&format!(" REMOVE {}", remove_parts.join(", ")));
        }
        // Guard with attribute_exists(PK): a bare update_item upserts, so a doc
        // hard-deleted between the caller's access check and this write would
        // otherwise resurrect a partial METADATA row (only the fields set here).
        // The condition makes that race fail cleanly instead. Mirrors
        // set_workspace_id.
        self.db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(DocumentMeta::sk().to_string()))
            .update_expression(&update_expr)
            .condition_expression("attribute_exists(PK)")
            .set_expression_attribute_values(Some(values))
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;
        Ok(())
    }

    /// #140: set or clear a document's edit-lock. Stored sparsely — the
    /// `locked` attribute is written only when `true` and REMOVEd when `false`,
    /// matching the write-when-true invariant in `doc_meta_to_item` so a row
    /// never carries a stale `locked=false`. Guarded by `attribute_exists(PK)`
    /// so a doc hard-deleted between the access check and this write fails
    /// cleanly instead of resurrecting a partial row. Mirrors
    /// `update_link_settings`.
    pub async fn set_locked(
        &self,
        doc_id: &str,
        locked: bool,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let mut values = HashMap::new();
        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));
        let update_expr = if locked {
            values.insert(":locked".to_string(), AttributeValue::Bool(true));
            "SET updated_at = :updated_at, locked = :locked".to_string()
        } else {
            "SET updated_at = :updated_at REMOVE locked".to_string()
        };
        self.db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(DocumentMeta::sk().to_string()))
            .update_expression(&update_expr)
            .condition_expression("attribute_exists(PK)")
            .set_expression_attribute_values(Some(values))
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;
        Ok(())
    }

    /// #142: set or clear a document's template flag. Stored sparsely — the
    /// `is_template` attribute is written only when `true` and REMOVEd when
    /// `false`, matching the `locked` write-when-true invariant in
    /// `doc_meta_to_item` so a row never carries a stale `is_template=false`.
    /// Guarded by `attribute_exists(PK)` so a doc hard-deleted between the
    /// access check and this write fails cleanly instead of resurrecting a
    /// partial row. Mirrors `set_locked`.
    pub async fn set_is_template(
        &self,
        doc_id: &str,
        is_template: bool,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let mut values = HashMap::new();
        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));
        let update_expr = if is_template {
            values.insert(":is_template".to_string(), AttributeValue::Bool(true));
            "SET updated_at = :updated_at, is_template = :is_template".to_string()
        } else {
            "SET updated_at = :updated_at REMOVE is_template".to_string()
        };
        self.db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(DocumentMeta::sk().to_string()))
            .update_expression(&update_expr)
            .condition_expression("attribute_exists(PK)")
            .set_expression_attribute_values(Some(values))
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;
        Ok(())
    }

    /// Assign or change a document's workspace. Updates both the model field
    /// and the `workspace_id_gsi` attribute so the row is discoverable via
    /// GSI3. Called by the M1 backfill migration and any future move-doc flow.
    ///
    /// Guarded by `attribute_exists(PK)`: `update_item` without a condition
    /// would upsert the row, which for a deleted or non-existent doc would
    /// write a partial METADATA with only PK/SK/workspace_id and corrupt the
    /// GSI. The condition returns a conditional-check error instead.
    pub async fn set_workspace_id(
        &self,
        doc_id: &str,
        workspace_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        self.db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(DocumentMeta::sk().to_string()))
            .update_expression("SET workspace_id = :ws, workspace_id_gsi = :ws")
            .condition_expression("attribute_exists(PK)")
            .expression_attribute_values(":ws", AttributeValue::S(workspace_id.to_string()))
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;
        Ok(())
    }

    /// List all documents owned by a user, newest first. Queries GSI1
    /// (owner_id_gsi/updated_at). Used by the template gallery to surface a
    /// caller's own templates even when the doc has no `workspace_id` set —
    /// the create-document handler does not currently default workspace, so
    /// a workspace-only query would miss them (#142 follow-up: default
    /// workspace inheritance on create).
    pub async fn query_docs_by_owner(
        &self,
        owner_id: &str,
    ) -> Result<Vec<DocumentMeta>, RepoError> {
        let items = self
            .db
            .query_index(
                "GSI1-owner-updated",
                "owner_id_gsi",
                owner_id,
                None,
                None,
                false, // newest first
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items
            .iter()
            .filter(|item| {
                item.get("SK")
                    .and_then(|v| v.as_s().ok())
                    .map(|s| s == "METADATA")
                    .unwrap_or(false)
            })
            .map(doc_meta_from_item)
            .collect()
    }

    /// List all documents in a workspace, newest first. Queries GSI3
    /// (workspace_id_gsi/updated_at).
    pub async fn query_docs_by_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DocumentMeta>, RepoError> {
        let items = self
            .db
            .query_index(
                "GSI3-workspace-updated",
                "workspace_id_gsi",
                workspace_id,
                None,
                None,
                false, // newest first
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items
            .iter()
            .filter(|item| {
                item.get("SK")
                    .and_then(|v| v.as_s().ok())
                    .map(|s| s == "METADATA")
                    .unwrap_or(false)
            })
            .map(doc_meta_from_item)
            .collect()
    }

    /// Scan all DocumentMeta rows, paginated. Used by the workspace backfill
    /// migration; prefer GSI1 (owner-updated) or GSI3 (workspace-updated) for
    /// production queries.
    ///
    /// A DynamoDB Scan applies its `Limit` to items evaluated *before* the
    /// `SK = METADATA` filter (and before the in-code `DOC#` PK filter), so a
    /// single page can surface zero documents on a mixed-type table while more
    /// exist further along. This method therefore loops, following
    /// `LastEvaluatedKey`, accumulating DOC# METADATA rows until it has a full
    /// page (`limit`) or the table is exhausted. The returned cursor is the
    /// (PK, SK) of the last document emitted; callers loop until it is `None`.
    pub async fn list_all_meta(
        &self,
        limit: i32,
        cursor: Option<(String, String)>,
    ) -> Result<(Vec<DocumentMeta>, Option<(String, String)>), RepoError> {
        let limit = limit.max(1) as usize;
        let mut metas: Vec<DocumentMeta> = Vec::with_capacity(limit);

        // Scan resume key: seeded from the caller's cursor (the PK/SK of the
        // last doc from the previous page), then tracks DynamoDB's own
        // LastEvaluatedKey between pages within this call.
        let mut start_key: Option<HashMap<String, AttributeValue>> = cursor.map(|(pk, sk)| {
            HashMap::from([
                ("PK".to_string(), AttributeValue::S(pk)),
                ("SK".to_string(), AttributeValue::S(sk)),
            ])
        });

        loop {
            let mut builder = self
                .db
                .inner()
                .scan()
                .table_name(self.db.table_name())
                .filter_expression("SK = :sk")
                .expression_attribute_values(":sk", AttributeValue::S("METADATA".to_string()));
            if let Some(ref key) = start_key {
                builder = builder.set_exclusive_start_key(Some(key.clone()));
            }

            let result = builder
                .send()
                .await
                .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

            for item in result.items.unwrap_or_default().iter() {
                // Scan with SK=METADATA matches folder rows too; keep only DOC# PKs.
                let pk = match item.get("PK").and_then(|v| v.as_s().ok()) {
                    Some(pk) if pk.starts_with("DOC#") => pk.clone(),
                    _ => continue,
                };
                if let Ok(meta) = doc_meta_from_item(item) {
                    metas.push(meta);
                    if metas.len() == limit {
                        // Full page. Resume after this doc on the next call.
                        return Ok((metas, Some((pk, "METADATA".to_string()))));
                    }
                }
            }

            match result.last_evaluated_key {
                // More of the table left to scan; keep accumulating.
                Some(key) => start_key = Some(key),
                // Table exhausted; this is the last page.
                None => return Ok((metas, None)),
            }
        }
    }

    /// Soft delete a document.
    ///
    /// M-E7 item 9: also writes `is_deleted_gsi = "true"` so the row
    /// joins the sparse `GSI7-deleted-at` index. The trash-cleanup
    /// worker queries that index to find eligible-for-purge docs
    /// without scanning the whole table.
    pub async fn soft_delete(&self, doc_id: &str, deleted_at: i64) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let mut values = HashMap::new();
        values.insert(":is_deleted".to_string(), AttributeValue::Bool(true));
        values.insert(":deleted_at".to_string(), AttributeValue::N(deleted_at.to_string()));
        values.insert(":updated_at".to_string(), AttributeValue::N(deleted_at.to_string()));
        values.insert(
            ":is_deleted_gsi".to_string(),
            AttributeValue::S(IS_DELETED_GSI_PARTITION.to_string()),
        );

        self.db
            .update_item(
                &pk,
                DocumentMeta::sk(),
                "SET is_deleted = :is_deleted, deleted_at = :deleted_at, updated_at = :updated_at, is_deleted_gsi = :is_deleted_gsi",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List soft-deleted documents eligible for hard-purge — i.e.
    /// `is_deleted = true` AND `deleted_at < cutoff_usec`. Uses
    /// `GSI7-deleted-at`'s range key to bound the scan to old
    /// rows; ascending order so we drain the oldest backlog first.
    /// `max_batch` caps per-call DDB read volume so a single tick
    /// can't pin the table.
    pub async fn list_eligible_for_purge(
        &self,
        cutoff_usec: i64,
        max_batch: usize,
    ) -> Result<Vec<DocumentMeta>, RepoError> {
        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .index_name("GSI7-deleted-at")
            .key_condition_expression("is_deleted_gsi = :pk AND deleted_at < :cutoff")
            .expression_attribute_values(
                ":pk",
                AttributeValue::S(IS_DELETED_GSI_PARTITION.to_string()),
            )
            .expression_attribute_values(":cutoff", AttributeValue::N(cutoff_usec.to_string()))
            .scan_index_forward(true)
            .limit(max_batch as i32)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        items.iter().map(doc_meta_from_item).collect()
    }

    /// Restore a soft-deleted document into a target folder.
    /// Clears `is_deleted` and `deleted_at`, sets `folder_id` and `updated_at`.
    /// Move a non-trashed document to a new folder. Updates
    /// `folder_id` + `updated_at` and nothing else — the caller is
    /// responsible for the folder_repo `remove_child` / `add_child`
    /// bookkeeping that mirrors the move on the folder side. Used
    /// by the bulk-move endpoint (Phase 5 M-P7 piece B); the
    /// single-doc move path goes through `update_metadata` /
    /// folder routes today.
    pub async fn set_folder(
        &self,
        doc_id: &str,
        target_folder_id: &str,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let mut values = HashMap::new();
        values.insert(
            ":folder_id".to_string(),
            AttributeValue::S(target_folder_id.to_string()),
        );
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(updated_at.to_string()),
        );
        self.db
            .update_item(
                &pk,
                DocumentMeta::sk(),
                "SET folder_id = :folder_id, updated_at = :updated_at",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// #149: add `folder_id` to the doc's additional-folder set (idempotent —
    /// no-op if it's already the primary or already present). The caller
    /// mirrors this with a `folder_repo.add_child` edge for per-folder
    /// listing. Read-modify-write: folder-membership changes are infrequent
    /// and not concurrency-hot, so the get+update is acceptable.
    pub async fn add_doc_folder(
        &self,
        doc_id: &str,
        folder_id: &str,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let meta = self
            .get(doc_id)
            .await?
            .ok_or_else(|| RepoError::InvalidArgument(format!("doc {doc_id} not found")))?;
        if meta.folder_id.as_deref() == Some(folder_id)
            || meta.additional_folder_ids.iter().any(|f| f == folder_id)
        {
            return Ok(());
        }
        let mut list = meta.additional_folder_ids;
        list.push(folder_id.to_string());
        self.write_additional_folders(doc_id, &list, updated_at).await
    }

    /// #149: remove `folder_id` from the doc's additional-folder set (no-op if
    /// absent). Never touches the primary `folder_id`.
    pub async fn remove_doc_folder(
        &self,
        doc_id: &str,
        folder_id: &str,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let meta = self
            .get(doc_id)
            .await?
            .ok_or_else(|| RepoError::InvalidArgument(format!("doc {doc_id} not found")))?;
        if !meta.additional_folder_ids.iter().any(|f| f == folder_id) {
            return Ok(());
        }
        let list: Vec<String> = meta
            .additional_folder_ids
            .into_iter()
            .filter(|f| f != folder_id)
            .collect();
        self.write_additional_folders(doc_id, &list, updated_at).await
    }

    /// #149: clear every additional folder membership (the primary
    /// `folder_id` is untouched). Used on trash/delete, which removes the doc
    /// from all locations — clearing the set so a later restore (which only
    /// re-homes the primary) can't resurrect stale memberships.
    pub async fn clear_additional_folders(
        &self,
        doc_id: &str,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        self.write_additional_folders(doc_id, &[], updated_at).await
    }

    /// Persist the additional-folder set. Writes the list attribute when
    /// non-empty, REMOVEs it when empty — preserving the sparse-when-empty
    /// invariant (so legacy/single-folder rows never carry an empty `L`).
    async fn write_additional_folders(
        &self,
        doc_id: &str,
        list: &[String],
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let mut values = HashMap::new();
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(updated_at.to_string()),
        );
        let expr = if list.is_empty() {
            "SET updated_at = :updated_at REMOVE additional_folder_ids".to_string()
        } else {
            values.insert(
                ":list".to_string(),
                AttributeValue::L(list.iter().cloned().map(AttributeValue::S).collect()),
            );
            "SET additional_folder_ids = :list, updated_at = :updated_at".to_string()
        };
        self.db
            .update_item(&pk, DocumentMeta::sk(), &expr, values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    pub async fn restore(
        &self,
        doc_id: &str,
        target_folder_id: &str,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let mut values = HashMap::new();
        values.insert(
            ":folder_id".to_string(),
            AttributeValue::S(target_folder_id.to_string()),
        );
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(updated_at.to_string()),
        );
        values.insert(":false".to_string(), AttributeValue::Bool(false));

        // Set is_deleted=false (keeping the attribute rather than REMOVE keeps
        // scans that filter on `is_deleted = false` matching).
        //
        // M-E7 item 9: also REMOVE `is_deleted_gsi` so the row drops
        // out of the sparse GSI7-deleted-at index. Doing this before
        // the worker's cutoff would otherwise let a restored doc get
        // hard-purged on the next tick if the cutoff swept past the
        // (still-present) deleted_at — REMOVE drops that risk.
        self.db
            .update_item(
                &pk,
                DocumentMeta::sk(),
                "SET folder_id = :folder_id, updated_at = :updated_at, is_deleted = :false REMOVE deleted_at, is_deleted_gsi",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Hard delete a document and all associated DynamoDB rows (METADATA,
    /// MEMBER#*, OPEN#*, UPDATE#*, REL#*, RREL#*), plus every S3 object under
    /// `docs/<id>/`. Reverse-relationships on other docs must be cleaned up
    /// separately by the caller (use `list_reverse_relationships` +
    /// `delete_relationship`) so the forward side on the other doc is also
    /// removed.
    pub async fn hard_delete(&self, doc_id: &str) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");

        // Enumerate every row under PK=DOC#<id>.
        let items = self
            .db
            .query(&pk, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        // DynamoDB first: doc becomes inaccessible before blobs disappear.
        for item in &items {
            if let Some(sk) = item.get("SK").and_then(|v| v.as_s().ok()) {
                self.db
                    .delete_item(&pk, sk)
                    .await
                    .map_err(|e| RepoError::Dynamo(e.to_string()))?;
            }
        }

        // Then sweep S3.
        self.s3
            .delete_prefix(&format!("docs/{doc_id}/"))
            .await
            .map_err(|e| RepoError::S3(e.to_string()))?;

        Ok(())
    }

    /// Save a new snapshot to S3, bump the metadata pointer, and record a
    /// SNAPSHOT# row for edit history.
    ///
    /// S3 is written *before* DynamoDB so a crash mid-call leaves an
    /// orphaned blob (harmless) rather than a metadata pointer to a
    /// missing object (would corrupt the doc — the live read path
    /// follows snapshot_s3_key blindly). Same ordering as put_content
    /// and restore_version in the routes layer.
    pub async fn save_snapshot(
        &self,
        doc_id: &str,
        snapshot: &[u8],
        new_version: u64,
        updated_at: i64,
        user_id: &str,
    ) -> Result<(), RepoError> {
        let s3_key = format!("docs/{doc_id}/snapshots/{new_version}.bin");

        // S3 first — an orphaned blob is harmless.
        self.s3
            .put_object(&s3_key, snapshot.to_vec())
            .await
            .map_err(|e| RepoError::S3(e.to_string()))?;

        // Then bump the metadata pointer.
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

        // Record a SNAPSHOT# row so the history pane can list this version.
        // Failure here is non-fatal: the live pointer + S3 blob are already
        // consistent, so the worst case is a missing history entry — same
        // posture as the compaction task before this was centralized.
        let snap = DocSnapshot {
            doc_id: doc_id.to_string(),
            version: new_version,
            s3_key,
            size_bytes: snapshot.len() as u64,
            user_id: user_id.to_string(),
            created_at: updated_at,
        };
        if let Err(e) = SnapshotRepo::new(self.db.clone()).create(&snap).await {
            tracing::warn!(doc_id, version = new_version, error = %e, "save_snapshot: SNAPSHOT# row write failed");
        }
        Ok(())
    }

    /// Like [`save_snapshot`](Self::save_snapshot), but guards the
    /// version bump with an optimistic lock: the DynamoDB update only
    /// commits if `snapshot_version` still equals `expected_version`.
    ///
    /// Returns [`SnapshotWrite::VersionConflict`] (not an `Err`) when a
    /// concurrent writer already advanced the version, so the caller can
    /// map it to a 409 without reaching for the storage SDK or
    /// string-matching the DynamoDB error. S3 is written first (an
    /// orphaned blob is harmless); the SNAPSHOT# row is best-effort —
    /// same ordering and posture as [`save_snapshot`](Self::save_snapshot).
    pub async fn save_snapshot_conditional(
        &self,
        doc_id: &str,
        snapshot: &[u8],
        expected_version: u64,
        new_version: u64,
        updated_at: i64,
        user_id: &str,
    ) -> Result<SnapshotWrite, RepoError> {
        let s3_key = format!("docs/{doc_id}/snapshots/{new_version}.bin");

        // S3 first — an orphaned blob is harmless.
        self.s3
            .put_object(&s3_key, snapshot.to_vec())
            .await
            .map_err(|e| RepoError::S3(e.to_string()))?;

        // Then bump the metadata pointer, conditional on the expected version.
        let pk = format!("DOC#{doc_id}");
        let mut values = HashMap::new();
        values.insert(":new_version".to_string(), AttributeValue::N(new_version.to_string()));
        values.insert(":expected_version".to_string(), AttributeValue::N(expected_version.to_string()));
        values.insert(":s3_key".to_string(), AttributeValue::S(s3_key.clone()));
        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));

        let result = self
            .db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(DocumentMeta::sk().to_string()))
            .update_expression(
                "SET snapshot_version = :new_version, snapshot_s3_key = :s3_key, updated_at = :updated_at",
            )
            .condition_expression("snapshot_version = :expected_version")
            .set_expression_attribute_values(Some(values))
            .send()
            .await;

        if let Err(e) = result {
            let svc = e.into_service_error();
            if svc.is_conditional_check_failed_exception() {
                return Ok(SnapshotWrite::VersionConflict);
            }
            return Err(RepoError::Dynamo(svc.to_string()));
        }

        // Record a SNAPSHOT# row so the history pane can list this version.
        // Best-effort: the live pointer + S3 blob are already consistent.
        let snap = DocSnapshot {
            doc_id: doc_id.to_string(),
            version: new_version,
            s3_key,
            size_bytes: snapshot.len() as u64,
            user_id: user_id.to_string(),
            created_at: updated_at,
        };
        if let Err(e) = SnapshotRepo::new(self.db.clone()).create(&snap).await {
            tracing::warn!(doc_id, version = new_version, error = %e, "save_snapshot_conditional: SNAPSHOT# row write failed");
        }
        Ok(SnapshotWrite::Committed)
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

    /// Append a CRDT update to the op log (conditional to prevent
    /// duplicate writes). Payloads at or below `UPDATE_INLINE_MAX_BYTES`
    /// are written inline as a DDB Blob; larger payloads are PutObject'd
    /// to S3 first and only an `update_s3_key` pointer lands on the
    /// DDB row, sidestepping the 400 KB item cap. S3-first ordering
    /// mirrors `save_snapshot`: a DDB row pointing at a missing S3
    /// object would corrupt the doc on next reload, so we never take
    /// that ordering.
    ///
    /// Unlike `save_snapshot`, however, there is no sweep that
    /// re-links orphaned update blobs. A crash between S3 PutObject
    /// and DDB PutItem leaves the bytes permanently unreachable —
    /// the update is effectively lost. The WS handler surfaces this
    /// case to the client via `MessageType::Error` so the user knows
    /// to retry rather than silently serving a stale doc on reload.
    pub async fn append_update(&self, update: &DocUpdate) -> Result<(), RepoError> {
        // L2 trust-boundary shape check: doc_id and clock are
        // interpolated into the DDB PK/SK and the S3 key path. Reject
        // path-unsafe characters defensively even though the edge
        // layer should already produce clean values — a caller bug
        // should surface as a typed error here rather than as a
        // tenant-boundary violation in S3.
        for (label, val) in [("doc_id", &update.doc_id), ("clock", &update.clock)] {
            if val.contains('/') || val.contains("..") {
                return Err(RepoError::InvalidArgument(format!(
                    "{label} contains path-unsafe characters",
                )));
            }
        }

        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(update.pk()));
        item.insert("SK".to_string(), AttributeValue::S(update.sk()));
        item.insert("user_id".to_string(), AttributeValue::S(update.user_id.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(update.created_at.to_string()));
        if let Some(ref version) = update.client_version {
            item.insert("client_version".to_string(), AttributeValue::S(version.clone()));
        }

        if update.update_bytes.len() > UPDATE_INLINE_MAX_BYTES {
            let s3_key = format!(
                "docs/{}/updates/{}.bin",
                update.doc_id, update.clock,
            );
            self.s3
                .put_object(&s3_key, update.update_bytes.clone())
                .await
                .map_err(|e| RepoError::S3(e.to_string()))?;
            item.insert("update_s3_key".to_string(), AttributeValue::S(s3_key));
        } else {
            item.insert(
                "update_bytes".to_string(),
                AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(update.update_bytes.clone())),
            );
        }

        self.db
            .put_item_conditional(item, "attribute_not_exists(PK) AND attribute_not_exists(SK)")
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get pending updates for a document (after the last snapshot).
    /// Rows that store their payload in S3 (large updates, see
    /// `append_update` + `UPDATE_INLINE_MAX_BYTES`) are reconstituted
    /// here by fetching the blob from S3 and populating
    /// `update_bytes`, so the caller sees a uniform `DocUpdate` shape
    /// regardless of where the bytes physically live.
    ///
    /// `max_total_bytes` (#91) bounds the accumulated `update_bytes`
    /// across all returned rows. The function bails with
    /// `RepoError::TooLarge` the instant the running total exceeds
    /// the cap. Callers should map that to `503 Service Unavailable`
    /// — failing a single oversize doc is correct vs. OOM-ing the
    /// task and taking every other doc down with it. Pre-merge the
    /// function returned every row unconditionally; one 49 MiB doc
    /// triggered a 40-minute service-wide outage.
    ///
    /// The bound is on the `update_bytes` payload sum, not on
    /// `len()` of the rows or on the DDB Query response size — DDB
    /// already paginates the latter. Counting payload bytes means
    /// the cap reflects actual heap pressure rather than something
    /// per-row that's easy to game with many tiny rows.
    pub async fn get_pending_updates(
        &self,
        doc_id: &str,
        max_total_bytes: usize,
    ) -> Result<Vec<DocUpdate>, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let items = self
            .db
            .query(&pk, Some("UPDATE#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        let mut result = Vec::with_capacity(items.len());
        let mut running_bytes: usize = 0;
        for item in &items {
            let sk = get_s(item, "SK")?;
            let clock = sk
                .strip_prefix("UPDATE#")
                .ok_or_else(|| RepoError::MissingField(format!("SK missing UPDATE# prefix: {sk}")))?
                .to_string();

            let client_version = item.get("client_version")
                .and_then(|v| v.as_s().ok())
                .map(|s| s.to_string());

            // S3-backed pointer takes precedence over an inline Blob —
            // a future protocol bump that records both for migration
            // should still resolve to the canonical S3 copy.
            let update_bytes = if let Some(s3_key) =
                item.get("update_s3_key").and_then(|v| v.as_s().ok())
            {
                self.s3
                    .get_object(s3_key)
                    .await
                    .map_err(|e| RepoError::S3(format!(
                        "fetch update blob {s3_key}: {e}",
                    )))?
            } else {
                item.get("update_bytes")
                    .and_then(|v| v.as_b().ok())
                    .map(|b| b.as_ref().to_vec())
                    .ok_or_else(|| RepoError::MissingField(
                        "update_bytes or update_s3_key".to_string(),
                    ))?
            };

            // Bail BEFORE pushing — if the new row would cross
            // the cap, return the projected total so the caller
            // sees the actual size that failed.
            running_bytes = running_bytes.saturating_add(update_bytes.len());
            if running_bytes > max_total_bytes {
                return Err(RepoError::TooLarge {
                    what: format!("pending updates for {doc_id}"),
                    actual: running_bytes,
                    cap: max_total_bytes,
                });
            }

            result.push(DocUpdate {
                doc_id: doc_id.to_string(),
                clock,
                update_bytes,
                user_id: get_s(item, "user_id")?,
                created_at: get_n(item, "created_at")?,
                client_version,
            });
        }
        Ok(result)
    }

    /// Existence check for unsnapshotted `UPDATE#` rows.
    ///
    /// Used by the WS disconnect path to decide whether compacting
    /// (snapshot + prune) is worthwhile versus just dropping an empty
    /// read-only room. Without this guard a read-only open/close would
    /// write a redundant snapshot — and a new `SNAPSHOT#` version row —
    /// every time someone views a doc.
    ///
    /// Note: this uses the standard paginating `db.query()`, which returns
    /// all attributes of every matching row — including the inline
    /// `update_bytes` blobs — so on a doc with a large op log it can be a
    /// multi-page, multi-MB read (it does NOT fetch S3-backed payloads).
    /// Accepted because it runs once per last-client disconnect, on a path
    /// already doing I/O-heavy compaction. A `Limit(1)` query primitive was
    /// considered and deferred as scope creep; add one if disconnect
    /// latency becomes observable.
    pub async fn has_pending_updates(&self, doc_id: &str) -> Result<bool, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let items = self
            .db
            .query(&pk, Some("UPDATE#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(!items.is_empty())
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
                // If the row points at an S3-backed payload, delete
                // the S3 blob first. A failure here is logged and
                // skipped — the worst case is an orphan blob that a
                // later prefix sweep cleans up — but the DDB row
                // still gets removed so the doc replay stays clean.
                if let Some(s3_key) = item.get("update_s3_key").and_then(|v| v.as_s().ok()) {
                    if let Err(e) = self.s3.delete_object(s3_key).await {
                        tracing::warn!(
                            doc_id, s3_key, error = %e,
                            "delete_updates_before: S3 blob delete failed, leaving orphan",
                        );
                    }
                }
                self.db
                    .delete_item(&pk, sk)
                    .await
                    .map_err(|e| RepoError::Dynamo(e.to_string()))?;
                count += 1;
            }
        }
        Ok(count)
    }

    // ─── Document Member CRUD (direct sharing) ──────────────────

    /// Add a member to a document (or update if already exists).
    pub async fn add_doc_member(&self, member: &DocMember) -> Result<(), RepoError> {
        let item = doc_member_to_item(member);
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Same as `add_doc_member`, but rejects the write if a membership row
    /// already exists for the same (doc_id, user_id). Used by the `/invite`
    /// slash command to close the check-then-write race where two concurrent
    /// invites could both pass the pre-check and produce duplicate
    /// announcements. On race, surfaces a ConditionalCheckFailed error which
    /// the API layer converts to 409.
    pub async fn add_doc_member_exclusive(&self, member: &DocMember) -> Result<(), RepoError> {
        let item = doc_member_to_item(member);
        self.db
            .put_item_conditional(item, "attribute_not_exists(PK)")
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get a specific document member.
    /// Whether `user_id` has ever opened `doc_id` (a `DOC#/OPEN#` row
    /// exists, written once by `record_open`). Used as a search-ranking
    /// engagement signal (#112) — a previously-opened doc ranks above an
    /// unrelated workspace-link doc of comparable text relevance.
    pub async fn has_opened(&self, doc_id: &str, user_id: &str) -> Result<bool, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let sk = format!("OPEN#{user_id}");
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(item.is_some())
    }

    // ── Favorites (#144) — per-user starred docs (PK USER#, SK FAV#) ──

    /// Mark a document as a favorite for a user. Idempotent (put overwrites).
    pub async fn add_favorite(&self, fav: &Favorite) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(fav.pk()));
        item.insert("SK".to_string(), AttributeValue::S(fav.sk()));
        item.insert("doc_id".to_string(), AttributeValue::S(fav.doc_id.clone()));
        item.insert(
            "added_at".to_string(),
            AttributeValue::N(fav.added_at.to_string()),
        );
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Remove a favorite. Idempotent (delete of a missing row is a no-op).
    pub async fn remove_favorite(&self, user_id: &str, doc_id: &str) -> Result<(), RepoError> {
        self.db
            .delete_item(&format!("USER#{user_id}"), &format!("FAV#{doc_id}"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Whether `user_id` has favorited `doc_id`.
    pub async fn is_favorite(&self, user_id: &str, doc_id: &str) -> Result<bool, RepoError> {
        let item = self
            .db
            .get_item(&format!("USER#{user_id}"), &format!("FAV#{doc_id}"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(item.is_some())
    }

    /// The doc ids a user has favorited (most-recent-first is the caller's job;
    /// DynamoDB returns them in SK order).
    pub async fn list_favorite_doc_ids(&self, user_id: &str) -> Result<Vec<String>, RepoError> {
        let items = self
            .db
            .query(&format!("USER#{user_id}"), Some("FAV#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(items
            .iter()
            .filter_map(|i| i.get("doc_id").and_then(|v| v.as_s().ok()).cloned())
            .collect())
    }

    // ── Collections (#144) — named per-user groups within Favorites ──
    //   PK USER#<uid>, SK COLLECTION#<cid> (the collection)
    //   PK USER#<uid>, SK COLLITEM#<cid>#<doc_id> (a doc's membership)

    /// Create (or overwrite) a collection row. Idempotent on the id.
    pub async fn create_collection(&self, coll: &Collection) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(coll.pk()));
        item.insert("SK".to_string(), AttributeValue::S(coll.sk()));
        item.insert("collection_id".to_string(), AttributeValue::S(coll.collection_id.clone()));
        item.insert("name".to_string(), AttributeValue::S(coll.name.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(coll.created_at.to_string()));
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// All of a user's collections, in SK order (id-sorted).
    pub async fn list_collections(&self, user_id: &str) -> Result<Vec<Collection>, RepoError> {
        let items = self
            .db
            .query(&format!("USER#{user_id}"), Some(Collection::SK_PREFIX))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(items
            .iter()
            .filter_map(|i| {
                Some(Collection {
                    user_id: user_id.to_string(),
                    collection_id: i.get("collection_id").and_then(|v| v.as_s().ok())?.clone(),
                    name: i.get("name").and_then(|v| v.as_s().ok())?.clone(),
                    created_at: i
                        .get("created_at")
                        .and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0),
                })
            })
            .collect())
    }

    /// Whether a collection exists for this user (authorization helper).
    pub async fn collection_exists(&self, user_id: &str, collection_id: &str) -> Result<bool, RepoError> {
        let item = self
            .db
            .get_item(&format!("USER#{user_id}"), &format!("COLLECTION#{collection_id}"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(item.is_some())
    }

    /// Delete a collection and every membership row under it. Idempotent.
    pub async fn delete_collection(&self, user_id: &str, collection_id: &str) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        // Drop all membership rows first so no orphans survive the collection.
        let members = self
            .db
            .query(&pk, Some(&CollectionItem::sk_prefix(collection_id)))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        for m in &members {
            if let Some(sk) = m.get("SK").and_then(|v| v.as_s().ok()) {
                self.db
                    .delete_item(&pk, sk)
                    .await
                    .map_err(|e| RepoError::Dynamo(e.to_string()))?;
            }
        }
        self.db
            .delete_item(&pk, &format!("COLLECTION#{collection_id}"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Add a document to a collection. Idempotent (put overwrites).
    pub async fn add_to_collection(&self, item: &CollectionItem) -> Result<(), RepoError> {
        let mut row = HashMap::new();
        row.insert("PK".to_string(), AttributeValue::S(item.pk()));
        row.insert("SK".to_string(), AttributeValue::S(item.sk()));
        row.insert("collection_id".to_string(), AttributeValue::S(item.collection_id.clone()));
        row.insert("doc_id".to_string(), AttributeValue::S(item.doc_id.clone()));
        row.insert("added_at".to_string(), AttributeValue::N(item.added_at.to_string()));
        self.db
            .put_item(row)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Remove a document from a collection. Idempotent.
    pub async fn remove_from_collection(
        &self,
        user_id: &str,
        collection_id: &str,
        doc_id: &str,
    ) -> Result<(), RepoError> {
        self.db
            .delete_item(
                &format!("USER#{user_id}"),
                &format!("COLLITEM#{collection_id}#{doc_id}"),
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// The doc ids in a collection, in SK order.
    pub async fn list_collection_doc_ids(
        &self,
        user_id: &str,
        collection_id: &str,
    ) -> Result<Vec<String>, RepoError> {
        let items = self
            .db
            .query(
                &format!("USER#{user_id}"),
                Some(&CollectionItem::sk_prefix(collection_id)),
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(items
            .iter()
            .filter_map(|i| i.get("doc_id").and_then(|v| v.as_s().ok()).cloned())
            .collect())
    }

    /// Which of a user's collections contain `doc_id`. Queries all membership
    /// rows (the doc id is the SK suffix, so it can't be range-keyed) and
    /// filters by the `doc_id` attribute.
    pub async fn list_collection_ids_for_doc(
        &self,
        user_id: &str,
        doc_id: &str,
    ) -> Result<Vec<String>, RepoError> {
        let items = self
            .db
            .query(&format!("USER#{user_id}"), Some(CollectionItem::SK_PREFIX_ALL))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(items
            .iter()
            .filter(|i| i.get("doc_id").and_then(|v| v.as_s().ok()).map(|d| d == doc_id).unwrap_or(false))
            .filter_map(|i| i.get("collection_id").and_then(|v| v.as_s().ok()).cloned())
            .collect())
    }

    pub async fn get_doc_member(
        &self,
        doc_id: &str,
        user_id: &str,
    ) -> Result<Option<DocMember>, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let sk = format!("MEMBER#{user_id}");
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(doc_member_from_item(&item, doc_id)?)),
            None => Ok(None),
        }
    }

    /// List all direct members of a document.
    pub async fn list_doc_members(&self, doc_id: &str) -> Result<Vec<DocMember>, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let items = self
            .db
            .query(&pk, Some("MEMBER#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items
            .iter()
            .map(|item| doc_member_from_item(item, doc_id))
            .collect()
    }

    /// Remove a member from a document.
    pub async fn remove_doc_member(&self, doc_id: &str, user_id: &str) -> Result<(), RepoError> {
        let pk = format!("DOC#{doc_id}");
        let sk = format!("MEMBER#{user_id}");
        self.db
            .delete_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    // ─── Open Receipts ──────────────────────────────────────────

    /// Record the first time a user opens a document.
    /// Uses conditional PutItem — succeeds only if the row doesn't exist yet.
    /// Returns Ok(true) if this was the first open, Ok(false) if already opened.
    pub async fn record_open(&self, open: &DocOpen) -> Result<bool, RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(open.pk()));
        item.insert("SK".to_string(), AttributeValue::S(open.sk()));
        item.insert("doc_id".to_string(), AttributeValue::S(open.doc_id.clone()));
        item.insert("user_id".to_string(), AttributeValue::S(open.user_id.clone()));
        item.insert("first_opened_at".to_string(), AttributeValue::N(open.first_opened_at.to_string()));

        match self
            .db
            .put_item_conditional(item, "attribute_not_exists(PK) AND attribute_not_exists(SK)")
            .await
        {
            Ok(()) => Ok(true),
            Err(e) => {
                // The wrapper hands back the top-level `aws_sdk_dynamodb::Error`,
                // so match the variant rather than the operation-level
                // `is_conditional_check_failed_exception()` predicate (which only
                // exists on the per-op error type). A conditional failure means
                // the row already exists → already opened.
                if matches!(e, aws_sdk_dynamodb::Error::ConditionalCheckFailedException(_)) {
                    Ok(false) // already opened
                } else {
                    Err(RepoError::Dynamo(e.to_string()))
                }
            }
        }
    }

    // ─── Document Relationships ────────────────────────────────

    /// Create a directed relationship between two documents.
    /// Stores both forward (REL#) and reverse (RREL#) entries atomically
    /// via a DynamoDB transaction. Uses a condition to prevent silent overwrites.
    pub async fn create_relationship(&self, rel: &DocRelationship) -> Result<(), RepoError> {
        use aws_sdk_dynamodb::types::{Put, TransactWriteItem};

        let attrs = |pk: String, sk: String| -> HashMap<String, AttributeValue> {
            let mut item = HashMap::new();
            item.insert("PK".to_string(), AttributeValue::S(pk));
            item.insert("SK".to_string(), AttributeValue::S(sk));
            item.insert("source_doc_id".to_string(), AttributeValue::S(rel.source_doc_id.clone()));
            item.insert("target_doc_id".to_string(), AttributeValue::S(rel.target_doc_id.clone()));
            item.insert("relation_type".to_string(), AttributeValue::S(rel.relation_type.as_str().to_string()));
            item.insert("created_by".to_string(), AttributeValue::S(rel.created_by.clone()));
            item.insert("created_at".to_string(), AttributeValue::N(rel.created_at.to_string()));
            item
        };

        let fwd = attrs(rel.pk(), rel.sk());
        let rev = attrs(rel.reverse_pk(), rel.reverse_sk());

        let condition = "attribute_not_exists(PK) AND attribute_not_exists(SK)";

        let items = vec![
            TransactWriteItem::builder()
                .put(
                    Put::builder()
                        .table_name(self.db.table_name())
                        .set_item(Some(fwd))
                        .condition_expression(condition)
                        .build()
                        .map_err(|e| RepoError::Dynamo(e.to_string()))?,
                )
                .build(),
            TransactWriteItem::builder()
                .put(
                    Put::builder()
                        .table_name(self.db.table_name())
                        .set_item(Some(rev))
                        .condition_expression(condition)
                        .build()
                        .map_err(|e| RepoError::Dynamo(e.to_string()))?,
                )
                .build(),
        ];

        self.db.transact_write(items).await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        Ok(())
    }

    /// Delete a relationship (both forward and reverse entries atomically).
    pub async fn delete_relationship(
        &self,
        source_doc_id: &str,
        relation_type: &RelationType,
        target_doc_id: &str,
    ) -> Result<(), RepoError> {
        use aws_sdk_dynamodb::types::{Delete, TransactWriteItem};

        let fwd_pk = format!("DOC#{source_doc_id}");
        let fwd_sk = format!("REL#{}#{target_doc_id}", relation_type.as_str());
        let rev_pk = format!("DOC#{target_doc_id}");
        let rev_sk = format!("RREL#{}#{source_doc_id}", relation_type.as_str());

        let items = vec![
            TransactWriteItem::builder()
                .delete(
                    Delete::builder()
                        .table_name(self.db.table_name())
                        .key("PK", AttributeValue::S(fwd_pk))
                        .key("SK", AttributeValue::S(fwd_sk))
                        .build()
                        .map_err(|e| RepoError::Dynamo(e.to_string()))?,
                )
                .build(),
            TransactWriteItem::builder()
                .delete(
                    Delete::builder()
                        .table_name(self.db.table_name())
                        .key("PK", AttributeValue::S(rev_pk))
                        .key("SK", AttributeValue::S(rev_sk))
                        .build()
                        .map_err(|e| RepoError::Dynamo(e.to_string()))?,
                )
                .build(),
        ];

        self.db.transact_write(items).await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        Ok(())
    }

    /// List forward relationships from a document, optionally filtered by type.
    pub async fn list_relationships(
        &self,
        doc_id: &str,
        relation_type: Option<&RelationType>,
    ) -> Result<Vec<DocRelationship>, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let sk_prefix = match relation_type {
            Some(rt) => format!("REL#{}#", rt.as_str()),
            None => "REL#".to_string(),
        };
        let items = self.db.query(&pk, Some(&sk_prefix)).await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(items.iter().filter_map(doc_rel_from_item).collect())
    }

    /// List reverse relationships (documents that point TO this doc).
    pub async fn list_reverse_relationships(
        &self,
        doc_id: &str,
        relation_type: Option<&RelationType>,
    ) -> Result<Vec<DocRelationship>, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let sk_prefix = match relation_type {
            Some(rt) => format!("RREL#{}#", rt.as_str()),
            None => "RREL#".to_string(),
        };
        let items = self.db.query(&pk, Some(&sk_prefix)).await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        Ok(items.iter().filter_map(doc_rel_from_item).collect())
    }
}

fn doc_rel_from_item(item: &HashMap<String, AttributeValue>) -> Option<DocRelationship> {
    let source_doc_id = item.get("source_doc_id")?.as_s().ok()?.clone();
    let target_doc_id = item.get("target_doc_id")?.as_s().ok()?.clone();
    let relation_type_str = item.get("relation_type")?.as_s().ok()?;
    let relation_type = RelationType::from_str(relation_type_str)?;
    let created_by = item.get("created_by")?.as_s().ok()?.clone();
    let created_at = item.get("created_at")?.as_n().ok()?.parse::<i64>().ok()?;

    Some(DocRelationship {
        source_doc_id,
        target_doc_id,
        relation_type,
        created_by,
        created_at,
    })
}

fn doc_member_to_item(member: &DocMember) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert("PK".to_string(), AttributeValue::S(member.pk()));
    item.insert("SK".to_string(), AttributeValue::S(member.sk()));
    item.insert(
        "access_level".to_string(),
        AttributeValue::S(
            serde_json::to_string(&member.access_level)
                .unwrap()
                .trim_matches('"')
                .to_string(),
        ),
    );
    item.insert("added_at".to_string(), AttributeValue::N(member.added_at.to_string()));
    item.insert("user_id".to_string(), AttributeValue::S(member.user_id.clone()));
    item
}

fn doc_member_from_item(
    item: &HashMap<String, AttributeValue>,
    doc_id: &str,
) -> Result<DocMember, RepoError> {
    let access_str = get_s(item, "access_level")?;
    let access_level: AccessLevel = serde_json::from_str(&format!("\"{access_str}\""))
        .map_err(|e| RepoError::MissingField(format!("access_level: {e}")))?;

    Ok(DocMember {
        doc_id: doc_id.to_string(),
        user_id: get_s(item, "user_id")?,
        access_level,
        added_at: get_n(item, "added_at")?,
    })
}

fn doc_meta_to_item(meta: &DocumentMeta) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert("doc_id".to_string(), AttributeValue::S(meta.doc_id.clone()));
    item.insert("title".to_string(), AttributeValue::S(meta.title.clone()));
    item.insert("owner_id".to_string(), AttributeValue::S(meta.owner_id.clone()));
    if let Some(ref fid) = meta.folder_id {
        item.insert("folder_id".to_string(), AttributeValue::S(fid.clone()));
    }
    // #149: additional folder memberships, written sparsely (omitted when
    // empty so legacy/single-folder rows carry no attribute).
    if !meta.additional_folder_ids.is_empty() {
        item.insert(
            "additional_folder_ids".to_string(),
            AttributeValue::L(
                meta.additional_folder_ids
                    .iter()
                    .cloned()
                    .map(AttributeValue::S)
                    .collect(),
            ),
        );
    }
    if let Some(ref wid) = meta.workspace_id {
        item.insert("workspace_id".to_string(), AttributeValue::S(wid.clone()));
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
    if let Some(ref mode) = meta.link_sharing_mode {
        item.insert(
            "link_sharing_mode".to_string(),
            AttributeValue::S(serde_json::to_string(mode).unwrap().trim_matches('"').to_string()),
        );
    }
    // Stored sparsely as a JSON-string attribute: written only when a
    // sub-option is enabled, so all-false (the common case) and legacy
    // rows carry no attribute and decode to the default in from_item.
    if meta.link_view_options != crate::models::ViewOptions::default() {
        item.insert(
            "link_view_options".to_string(),
            AttributeValue::S(serde_json::to_string(&meta.link_view_options).unwrap()),
        );
    }
    // #140: written only when locked, matching the write-when-non-default
    // invariant above. Absence decodes to unlocked (legacy + common case).
    if meta.locked {
        item.insert("locked".to_string(), AttributeValue::Bool(true));
    }
    // #142: written only when the doc is a template. Mirrors `locked`.
    if meta.is_template {
        item.insert("is_template".to_string(), AttributeValue::Bool(true));
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
        additional_folder_ids: item
            .get("additional_folder_ids")
            .and_then(|v| v.as_l().ok())
            .map(|l| l.iter().filter_map(|av| av.as_s().ok().cloned()).collect())
            .unwrap_or_default(),
        workspace_id: item.get("workspace_id").and_then(|v| v.as_s().ok()).cloned(),
        doc_type,
        snapshot_version: get_n_u64(item, "snapshot_version")?,
        snapshot_s3_key: item.get("snapshot_s3_key").and_then(|v| v.as_s().ok()).cloned(),
        is_deleted: item.get("is_deleted").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false),
        deleted_at: item.get("deleted_at").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<i64>().ok()),
        link_sharing_mode: item.get("link_sharing_mode").and_then(|v| v.as_s().ok())
            .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok()),
        link_view_options: item.get("link_view_options")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default(),
        locked: item.get("locked").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false),
        is_template: item.get("is_template").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false),
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DocType, ViewOptions};

    fn sample_meta() -> DocumentMeta {
        DocumentMeta {
            doc_id: "doc1".to_string(),
            title: "T".to_string(),
            owner_id: "u1".to_string(),
            folder_id: None,
            additional_folder_ids: Vec::new(),
            workspace_id: None,
            doc_type: DocType::Document,
            snapshot_version: 1,
            snapshot_s3_key: None,
            is_deleted: false,
            deleted_at: None,
            link_sharing_mode: None,
            link_view_options: ViewOptions::default(),
            locked: false,
            is_template: false,
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn view_options_round_trip_through_item() {
        let mut meta = sample_meta();
        meta.link_view_options = ViewOptions {
            allow_comments: true,
            show_history: true,
            show_conversation: false,
            allow_request_access: true,
        };
        let item = doc_meta_to_item(&meta);
        let back = doc_meta_from_item(&item).expect("from_item");
        assert_eq!(back.link_view_options, meta.link_view_options);
    }

    #[test]
    fn default_view_options_are_sparse_and_decode_as_default() {
        // All-false options write no attribute (sparse), and a row
        // lacking the attribute decodes to the default. This is also
        // the legacy-row path: pre-Phase-1 rows carry no
        // `link_view_options` attribute and must still decode cleanly.
        let item = doc_meta_to_item(&sample_meta());
        assert!(
            !item.contains_key("link_view_options"),
            "default (all-false) options must not write an attribute"
        );
        let back = doc_meta_from_item(&item).expect("from_item");
        assert_eq!(back.link_view_options, ViewOptions::default());
    }

    #[test]
    fn locked_round_trips_through_item() {
        // #140: a locked doc writes the attribute and decodes back to true.
        let mut meta = sample_meta();
        meta.locked = true;
        let item = doc_meta_to_item(&meta);
        assert!(item.contains_key("locked"), "locked=true must write an attribute");
        let back = doc_meta_from_item(&item).expect("from_item");
        assert!(back.locked);
    }

    #[test]
    fn unlocked_is_sparse_and_decodes_as_false() {
        // #140: the common case writes no attribute; legacy rows lacking it
        // decode to unlocked. Mirrors the link_view_options sparse invariant.
        let item = doc_meta_to_item(&sample_meta());
        assert!(
            !item.contains_key("locked"),
            "unlocked (the default) must not write a `locked` attribute"
        );
        let back = doc_meta_from_item(&item).expect("from_item");
        assert!(!back.locked);
    }

    #[test]
    fn additional_folder_ids_round_trip_through_item() {
        // #149: a doc with additional folder memberships writes the attribute
        // and decodes back to the same list. Load-bearing for access control,
        // so the L-of-S encoding is pinned.
        let mut meta = sample_meta();
        meta.additional_folder_ids = vec!["folder-a".to_string(), "folder-b".to_string()];
        let item = doc_meta_to_item(&meta);
        assert!(
            item.contains_key("additional_folder_ids"),
            "non-empty additional_folder_ids must write an attribute"
        );
        let back = doc_meta_from_item(&item).expect("from_item");
        assert_eq!(back.additional_folder_ids, meta.additional_folder_ids);
    }

    #[test]
    fn empty_additional_folder_ids_are_sparse_and_decode_as_empty() {
        // #149: the common single-folder case writes no attribute; legacy
        // rows lacking it decode to an empty Vec (not missing/error).
        let item = doc_meta_to_item(&sample_meta());
        assert!(
            !item.contains_key("additional_folder_ids"),
            "empty additional_folder_ids must not write an attribute"
        );
        let back = doc_meta_from_item(&item).expect("from_item");
        assert!(back.additional_folder_ids.is_empty());
    }

    #[test]
    fn is_template_round_trips_through_item() {
        // #142: a template doc writes the attribute and decodes back to true.
        let mut meta = sample_meta();
        meta.is_template = true;
        let item = doc_meta_to_item(&meta);
        assert!(
            item.contains_key("is_template"),
            "is_template=true must write an attribute",
        );
        let back = doc_meta_from_item(&item).expect("from_item");
        assert!(back.is_template);
    }

    #[test]
    fn non_template_is_sparse_and_decodes_as_false() {
        // #142: the common case writes no attribute; legacy rows lacking it
        // decode to non-template. Mirrors the `locked` sparse invariant.
        let item = doc_meta_to_item(&sample_meta());
        assert!(
            !item.contains_key("is_template"),
            "non-template (the default) must not write an `is_template` attribute",
        );
        let back = doc_meta_from_item(&item).expect("from_item");
        assert!(!back.is_template);
    }
}
