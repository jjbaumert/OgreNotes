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
