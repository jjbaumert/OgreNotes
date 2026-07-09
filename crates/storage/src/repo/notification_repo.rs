// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for user notifications.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::notification::{NotifPref, NotifType, Notification};
use crate::models::NotifLevel;

pub struct NotificationRepo {
    db: DynamoClient,
}

impl NotificationRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Create a notification for a user.
    pub async fn create(&self, notif: &Notification) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(notif.pk()));
        item.insert("SK".to_string(), AttributeValue::S(notif.sk()));
        item.insert("notif_id".to_string(), AttributeValue::S(notif.notif_id.clone()));
        item.insert("user_id".to_string(), AttributeValue::S(notif.user_id.clone()));
        item.insert(
            "notif_type".to_string(),
            AttributeValue::S(
                serde_json::to_string(&notif.notif_type)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
        if let Some(ref doc_id) = notif.doc_id {
            item.insert("doc_id".to_string(), AttributeValue::S(doc_id.clone()));
        }
        if let Some(ref thread_id) = notif.thread_id {
            item.insert("thread_id".to_string(), AttributeValue::S(thread_id.clone()));
        }
        item.insert("actor_id".to_string(), AttributeValue::S(notif.actor_id.clone()));
        item.insert("message".to_string(), AttributeValue::S(notif.message.clone()));
        if let Some(ref preview) = notif.preview {
            item.insert("preview".to_string(), AttributeValue::S(preview.clone()));
        }
        if let Some(ref block_id) = notif.block_id {
            item.insert("block_id".to_string(), AttributeValue::S(block_id.clone()));
        }
        item.insert(
            "is_read".to_string(),
            AttributeValue::Bool(notif.read),
        );
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(notif.created_at.to_string()),
        );

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List notifications for a user (newest first).
    /// Returns up to `limit` notifications.
    pub async fn list(
        &self,
        user_id: &str,
        limit: usize,
    ) -> Result<Vec<Notification>, RepoError> {
        let pk = format!("USER#{user_id}");

        // Query with NOTIF# prefix, scan backwards (newest first)
        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(":prefix", AttributeValue::S("NOTIF#".to_string()))
            .scan_index_forward(false)
            .limit(limit as i32)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        items
            .iter()
            .map(|item| notif_from_item(item, user_id))
            .collect()
    }

    /// List a user's unread notifications created since `since_usec`
    /// (inclusive), newest first. Used by the daily-digest scheduler to
    /// pick up the last 24 hours of unseen activity.
    ///
    /// The SK range query scopes the scan to notifications newer than the
    /// cutoff; the `is_read = false` filter is applied server-side so the
    /// handler sees only candidate digest rows. `limit` is applied after
    /// filtering, so a low limit can under-count if many recent reads sit
    /// above the window — fine for digests, which are best-effort summary
    /// emails.
    pub async fn list_unread_since(
        &self,
        user_id: &str,
        since_usec: i64,
        limit: usize,
    ) -> Result<Vec<Notification>, RepoError> {
        let pk = format!("USER#{user_id}");
        let sk_start = format!("NOTIF#{:020}", since_usec);
        // '~' sorts after any digit, so this upper bound includes every
        // NOTIF# row regardless of the 20-digit prefix and trailing id.
        let sk_end = "NOTIF#~".to_string();

        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .key_condition_expression("PK = :pk AND SK BETWEEN :start AND :end")
            .filter_expression("is_read = :false")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(":start", AttributeValue::S(sk_start))
            .expression_attribute_values(":end", AttributeValue::S(sk_end))
            .expression_attribute_values(":false", AttributeValue::Bool(false))
            .scan_index_forward(false)
            .limit(limit as i32)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        items
            .iter()
            .map(|item| notif_from_item(item, user_id))
            .collect()
    }

    /// Mark a specific notification as read.
    /// Validates SK format and requires the item to exist.
    pub async fn mark_read(
        &self,
        user_id: &str,
        sk: &str,
    ) -> Result<(), RepoError> {
        if !sk.starts_with("NOTIF#") {
            return Err(RepoError::MissingField("invalid notification SK format".to_string()));
        }

        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(":read".to_string(), AttributeValue::Bool(true));

        // Use conditional update to prevent creating phantom items
        self.db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(sk.to_string()))
            .update_expression("SET is_read = :read")
            .expression_attribute_values(":read", AttributeValue::Bool(true))
            .condition_expression("attribute_exists(notif_id)")
            .send()
            .await
            .map_err(|e| {
                // Typed check on the operation error rather than substring-
                // matching its Display output. The condition guards
                // attribute_exists(notif_id), so a conditional failure means
                // the notification row isn't there.
                let svc = e.into_service_error();
                if svc.is_conditional_check_failed_exception() {
                    RepoError::MissingField("notification not found".to_string())
                } else {
                    RepoError::Dynamo(svc.to_string())
                }
            })?;

        Ok(())
    }

    /// Mark all of a user's unread notifications as read; returns the count
    /// flipped.
    ///
    /// `db.query` paginates internally, so this sees every `NOTIF#` row, not
    /// just the first 1 MB page. The cost that scales with the unread count
    /// is the per-row conditional `mark_read` UpdateItem below: a user with
    /// thousands of unread notifications incurs that many sequential writes.
    /// Accepted for now (typical unread counts are small) — batching would
    /// mean dropping the per-row `attribute_exists` condition, so it's left
    /// as a future optimization.
    pub async fn mark_all_read(&self, user_id: &str) -> Result<usize, RepoError> {
        let pk = format!("USER#{user_id}");
        let items = self
            .db
            .query(&pk, Some("NOTIF#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        let mut count = 0;
        for item in &items {
            let is_read = item
                .get("is_read")
                .and_then(|v| v.as_bool().ok())
                .copied()
                .unwrap_or(false);

            if !is_read {
                if let Ok(sk) = get_s(item, "SK") {
                    // Best-effort per row: a single row that can't be
                    // marked (e.g. a legacy/partial row that fails
                    // mark_read's existence guard, or a transient write
                    // error) must NOT abort the whole batch. The previous
                    // `?` here meant one bad row 500'd the entire request,
                    // which the UI surfaces as "mark all read does
                    // nothing." Skip the failure, keep going, and report
                    // the count actually flipped.
                    match self.mark_read(user_id, &sk).await {
                        Ok(()) => count += 1,
                        Err(e) => {
                            tracing::warn!(sk = %sk, error = %e, "mark_all_read: skipping row");
                        }
                    }
                }
            }
        }

        Ok(count)
    }

    /// Count a user's unread notifications.
    ///
    /// `db.query` paginates internally, so the count is exact regardless of
    /// how many `NOTIF#` rows the user has. It does read every row's
    /// attributes to do so; a future optimization could project only
    /// `is_read` or maintain a counter, but exactness is preferred over
    /// micro-optimizing the read here.
    pub async fn unread_count(&self, user_id: &str) -> Result<usize, RepoError> {
        let pk = format!("USER#{user_id}");
        let items = self
            .db
            .query(&pk, Some("NOTIF#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        Ok(items
            .iter()
            .filter(|item| {
                item.get("is_read")
                    .and_then(|v| v.as_bool().ok())
                    .copied()
                    .unwrap_or(false)
                    == false
            })
            .count())
    }

    // ─── Notification preferences ───────────────────────────────

    /// Set notification level for a thread (PutItem — creates or overwrites).
    pub async fn set_pref(&self, pref: &NotifPref) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(pref.pk()));
        item.insert("SK".to_string(), AttributeValue::S(pref.sk()));
        item.insert("thread_id".to_string(), AttributeValue::S(pref.thread_id.clone()));
        item.insert("user_id".to_string(), AttributeValue::S(pref.user_id.clone()));
        item.insert(
            "level".to_string(),
            AttributeValue::S(
                serde_json::to_string(&pref.level).unwrap().trim_matches('"').to_string(),
            ),
        );

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get notification level for a thread.
    pub async fn get_pref(
        &self,
        user_id: &str,
        thread_id: &str,
    ) -> Result<Option<NotifPref>, RepoError> {
        let pk = format!("USER#{user_id}");
        let sk = format!("NOTIF_PREF#{thread_id}");
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => {
                let level_str = get_s(&item, "level")?;
                let level: NotifLevel = serde_json::from_str(&format!("\"{level_str}\""))
                    .map_err(|e| RepoError::MissingField(format!("level: {e}")))?;
                Ok(Some(NotifPref {
                    user_id: user_id.to_string(),
                    thread_id: thread_id.to_string(),
                    level,
                }))
            }
            None => Ok(None),
        }
    }

    /// Dismiss (hard-delete) all of a user's notifications. Best-effort
    /// per row — a single delete failure is logged and skipped so it
    /// can't abort the batch (mirrors `mark_all_read`). Returns the count
    /// actually deleted. The `NOTIF#` prefix excludes `NOTIF_PREF#` rows.
    pub async fn delete_all(&self, user_id: &str) -> Result<usize, RepoError> {
        let pk = format!("USER#{user_id}");
        let items = self
            .db
            .query(&pk, Some("NOTIF#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        let mut count = 0;
        for item in &items {
            if let Some(sk) = item.get("SK").and_then(|v| v.as_s().ok()) {
                match self.db.delete_item(&pk, sk).await {
                    Ok(()) => count += 1,
                    Err(e) => {
                        tracing::warn!(sk = %sk, error = %e, "delete_all: skipping row");
                    }
                }
            }
        }

        Ok(count)
    }

    /// Dismiss (hard-delete) a single notification by its sort key.
    pub async fn delete_one(&self, user_id: &str, sk: &str) -> Result<(), RepoError> {
        if !sk.starts_with("NOTIF#") {
            return Err(RepoError::MissingField(
                "invalid notification SK format".to_string(),
            ));
        }
        let pk = format!("USER#{user_id}");
        self.db
            .delete_item(&pk, sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Delete notification preference (revert to default).
    pub async fn delete_pref(&self, user_id: &str, thread_id: &str) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let sk = format!("NOTIF_PREF#{thread_id}");
        self.db
            .delete_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Check whether a notification should be created for a user on a thread.
    /// `is_direct` is true for @mentions and direct replies.
    /// Default level is `Direct` (only notify on @mentions/replies).
    pub async fn should_notify(
        &self,
        user_id: &str,
        thread_id: &str,
        is_direct: bool,
    ) -> bool {
        match self.get_pref(user_id, thread_id).await {
            Ok(Some(pref)) => match pref.level {
                NotifLevel::All => true,
                NotifLevel::Direct => is_direct,
                NotifLevel::Mute => false,
            },
            _ => is_direct, // default: direct responses only
        }
    }
}

fn notif_from_item(
    item: &HashMap<String, AttributeValue>,
    user_id: &str,
) -> Result<Notification, RepoError> {
    let type_str = get_s(item, "notif_type")?;
    let notif_type: NotifType = serde_json::from_str(&format!("\"{type_str}\""))
        .map_err(|e| RepoError::MissingField(format!("notif_type: {e}")))?;

    let doc_id = item
        .get("doc_id")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string());
    let thread_id = item
        .get("thread_id")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string());
    let preview = item
        .get("preview")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string());
    let block_id = item
        .get("block_id")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string());
    let read = item
        .get("is_read")
        .and_then(|v| v.as_bool().ok())
        .copied()
        .unwrap_or(false);

    Ok(Notification {
        notif_id: get_s(item, "notif_id")?,
        user_id: user_id.to_string(),
        notif_type,
        doc_id,
        thread_id,
        actor_id: get_s(item, "actor_id")?,
        message: get_s(item, "message")?,
        preview,
        block_id,
        read,
        created_at: get_n(item, "created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Notification {
        Notification {
            notif_id: "n1".to_string(),
            user_id: "u1".to_string(),
            notif_type: NotifType::Mentioned,
            doc_id: Some("doc1".to_string()),
            thread_id: Some("t1".to_string()),
            actor_id: "u2".to_string(),
            message: "mentioned you".to_string(),
            preview: Some("hey @u1 …".to_string()),
            block_id: Some("blk-1".to_string()),
            read: false,
            created_at: 1_700_000_000_000_000,
        }
    }

    /// Mimic `create`'s column construction (no live table),
    /// including its conditional writes and the `read` →
    /// `is_read` column rename.
    fn item_for(notif: &Notification) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(notif.pk()));
        item.insert("SK".to_string(), AttributeValue::S(notif.sk()));
        item.insert("notif_id".to_string(), AttributeValue::S(notif.notif_id.clone()));
        item.insert("user_id".to_string(), AttributeValue::S(notif.user_id.clone()));
        item.insert(
            "notif_type".to_string(),
            AttributeValue::S(
                serde_json::to_string(&notif.notif_type).unwrap().trim_matches('"').to_string(),
            ),
        );
        if let Some(ref doc_id) = notif.doc_id {
            item.insert("doc_id".to_string(), AttributeValue::S(doc_id.clone()));
        }
        if let Some(ref thread_id) = notif.thread_id {
            item.insert("thread_id".to_string(), AttributeValue::S(thread_id.clone()));
        }
        item.insert("actor_id".to_string(), AttributeValue::S(notif.actor_id.clone()));
        item.insert("message".to_string(), AttributeValue::S(notif.message.clone()));
        if let Some(ref preview) = notif.preview {
            item.insert("preview".to_string(), AttributeValue::S(preview.clone()));
        }
        if let Some(ref block_id) = notif.block_id {
            item.insert("block_id".to_string(), AttributeValue::S(block_id.clone()));
        }
        item.insert("is_read".to_string(), AttributeValue::Bool(notif.read));
        item.insert("created_at".to_string(), AttributeValue::N(notif.created_at.to_string()));
        item
    }

    #[test]
    fn notif_round_trips_with_all_optional_fields() {
        let notif = fixture();
        let back = notif_from_item(&item_for(&notif), "u1").expect("from_item");
        assert_eq!(back.notif_id, notif.notif_id);
        assert_eq!(back.notif_type, notif.notif_type);
        assert_eq!(back.doc_id, notif.doc_id);
        assert_eq!(back.thread_id, notif.thread_id);
        assert_eq!(back.actor_id, notif.actor_id);
        assert_eq!(back.message, notif.message);
        assert_eq!(back.preview, notif.preview);
        assert_eq!(back.block_id, notif.block_id);
        assert_eq!(back.read, notif.read);
        assert_eq!(back.created_at, notif.created_at);
    }

    #[test]
    fn notif_round_trips_read_flag_via_is_read_column() {
        // The model field is `read` but the storage column is
        // `is_read` (avoids clashing with any future reserved word).
        // Both polarities must survive the rename.
        let mut notif = fixture();
        notif.read = true;
        let item = item_for(&notif);
        assert!(item.contains_key("is_read"), "column must be is_read");
        assert!(!item.contains_key("read"), "model field name must not leak");
        let back = notif_from_item(&item, "u1").expect("from_item");
        assert!(back.read);
    }

    #[test]
    fn notif_minimal_row_decodes_optionals_as_absent() {
        let mut notif = fixture();
        notif.doc_id = None;
        notif.thread_id = None;
        notif.preview = None;
        notif.block_id = None;
        let back = notif_from_item(&item_for(&notif), "u1").expect("from_item");
        assert_eq!(back.doc_id, None);
        assert_eq!(back.thread_id, None);
        assert_eq!(back.preview, None);
        assert_eq!(back.block_id, None);
    }

    #[test]
    fn notif_missing_is_read_defaults_to_unread() {
        // A row predating the read-tracking column must surface as
        // unread (the safe direction — the user sees it once more)
        // rather than failing decode.
        let mut item = item_for(&fixture());
        item.remove("is_read");
        let back = notif_from_item(&item, "u1").expect("from_item");
        assert!(!back.read);
    }

    #[test]
    fn notif_type_round_trips_for_every_variant() {
        for nt in [
            NotifType::Shared,
            NotifType::Mentioned,
            NotifType::Commented,
            NotifType::ChatMessage,
            NotifType::DocumentEdited,
            NotifType::DocumentOpened,
            NotifType::RequestAccess,
        ] {
            let mut notif = fixture();
            notif.notif_type = nt.clone();
            let back = notif_from_item(&item_for(&notif), "u1")
                .unwrap_or_else(|e| panic!("roundtrip failed for {nt:?}: {e}"));
            assert_eq!(back.notif_type, nt);
        }
    }

    #[test]
    fn notif_unknown_type_errors() {
        let mut item = item_for(&fixture());
        item.insert("notif_type".to_string(), AttributeValue::S("telegram".to_string()));
        match notif_from_item(&item, "u1") {
            Err(RepoError::MissingField(msg)) => {
                assert!(msg.contains("notif_type"), "must name the field: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    // ── SK-prefix guards on mark_read / delete_one ────────────────
    // Both bail on a malformed SK *before* any network call, so they
    // are testable with an offline client. The guard is what stops a
    // crafted SK (e.g. "NOTIF_PREF#t1" or "PROFILE") from updating or
    // deleting a non-notification row under the same USER# partition.

    fn offline_repo() -> NotificationRepo {
        let conf = aws_sdk_dynamodb::Config::builder()
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .build();
        let client = aws_sdk_dynamodb::Client::from_conf(conf);
        NotificationRepo::new(DynamoClient::new(client, "test-table".to_string()))
    }

    #[tokio::test]
    async fn mark_read_rejects_non_notif_sk_before_any_io() {
        let repo = offline_repo();
        for bad_sk in ["PROFILE", "NOTIF_PREF#t1", "SESSION#s1", ""] {
            let err = repo
                .mark_read("u1", bad_sk)
                .await
                .expect_err("non-NOTIF# SK must be rejected");
            match err {
                RepoError::MissingField(msg) => {
                    assert!(msg.contains("invalid notification SK"), "got: {msg}")
                }
                other => panic!("expected MissingField for {bad_sk:?}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn delete_one_rejects_non_notif_sk_before_any_io() {
        let repo = offline_repo();
        // NOTIF_PREF# rows share the USER# partition; deleting one via
        // delete_one would silently reset a user's mute preference.
        let err = repo
            .delete_one("u1", "NOTIF_PREF#t1")
            .await
            .expect_err("NOTIF_PREF# SK must be rejected");
        match err {
            RepoError::MissingField(msg) => {
                assert!(msg.contains("invalid notification SK"), "got: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }
}
