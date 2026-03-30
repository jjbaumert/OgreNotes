//! Repository for user notifications.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::notification::{NotifType, Notification};

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
                let err_str = e.into_service_error().to_string();
                if err_str.contains("ConditionalCheckFailed") {
                    RepoError::MissingField("notification not found".to_string())
                } else {
                    RepoError::Dynamo(err_str)
                }
            })?;

        Ok(())
    }

    /// Mark all notifications as read for a user.
    /// NOTE: Uses non-paginated query — only processes first DynamoDB page (~1MB).
    /// Acceptable for MVP (typical users have < 100 notifications).
    /// TODO: Add pagination loop for production scale.
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
                    self.mark_read(user_id, &sk).await?;
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Get count of unread notifications for a user.
    /// NOTE: Uses non-paginated query — see mark_all_read note.
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
        read,
        created_at: get_n(item, "created_at")?,
    })
}
