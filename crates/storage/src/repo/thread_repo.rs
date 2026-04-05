//! Repository for comment threads and messages.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::thread::{Message, Thread, ThreadStatus, ThreadType};

pub struct ThreadRepo {
    db: DynamoClient,
}

impl ThreadRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    // ─── Thread CRUD ────────────────────────────────────────────

    /// Create a new comment thread.
    pub async fn create_thread(&self, thread: &Thread) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(thread.pk()));
        item.insert("SK".to_string(), AttributeValue::S(thread.sk()));
        item.insert("doc_id".to_string(), AttributeValue::S(thread.doc_id.clone()));
        item.insert(
            "doc_id_gsi".to_string(),
            AttributeValue::S(thread.doc_id.clone()),
        );
        item.insert(
            "thread_type".to_string(),
            AttributeValue::S(serde_json::to_string(&thread.thread_type).unwrap().trim_matches('"').to_string()),
        );
        item.insert(
            "status".to_string(),
            AttributeValue::S(serde_json::to_string(&thread.status).unwrap().trim_matches('"').to_string()),
        );
        item.insert("created_by".to_string(), AttributeValue::S(thread.created_by.clone()));
        if let Some(ref bid) = thread.block_id {
            item.insert("block_id".to_string(), AttributeValue::S(bid.clone()));
        }
        if let Some(start) = thread.anchor_start {
            item.insert("anchor_start".to_string(), AttributeValue::N(start.to_string()));
        }
        if let Some(end) = thread.anchor_end {
            item.insert("anchor_end".to_string(), AttributeValue::N(end.to_string()));
        }
        if let Some(ref title) = thread.title {
            item.insert("title".to_string(), AttributeValue::S(title.clone()));
        }
        if !thread.member_ids.is_empty() {
            item.insert(
                "member_ids".to_string(),
                AttributeValue::Ss(thread.member_ids.clone()),
            );
        }
        item.insert("created_at".to_string(), AttributeValue::N(thread.created_at.to_string()));
        item.insert(
            "updated_at".to_string(),
            AttributeValue::N(thread.updated_at.to_string()),
        );

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get a thread by ID.
    pub async fn get_thread(&self, thread_id: &str) -> Result<Option<Thread>, RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let item = self
            .db
            .get_item(&pk, "METADATA")
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(thread_from_item(&item, thread_id)?)),
            None => Ok(None),
        }
    }

    /// List all threads for a document.
    /// Tries the GSI5-docid-updated GSI first; falls back to a scan if the GSI
    /// doesn't exist (common in dev environments without full infra).
    pub async fn list_threads_for_doc(&self, doc_id: &str) -> Result<Vec<Thread>, RepoError> {
        let items = match self.db.query_index(
            "GSI5-docid-updated",
            "doc_id_gsi",
            doc_id,
            None,
            None,
            false,
            None,
        ).await {
            Ok(items) => items,
            Err(_) => {
                // GSI doesn't exist — fall back to scan with filter.
                // This is slower but works without the GSI.
                self.db.scan_with_filter(
                    "doc_id",
                    doc_id,
                ).await.map_err(|e| RepoError::Dynamo(e.to_string()))?
            }
        };

        items
            .iter()
            .filter_map(|item| {
                let sk = item.get("SK")?.as_s().ok()?;
                if sk == "METADATA" {
                    let pk = item.get("PK")?.as_s().ok()?;
                    let thread_id = pk.strip_prefix("THREAD#")?;
                    thread_from_item(item, thread_id).ok()
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(Ok)
            .collect()
    }

    /// Update thread status (resolve/reopen).
    pub async fn update_status(
        &self,
        thread_id: &str,
        status: ThreadStatus,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let status_str = serde_json::to_string(&status).unwrap().trim_matches('"').to_string();

        let mut values = HashMap::new();
        values.insert(":status".to_string(), AttributeValue::S(status_str));
        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));

        self.db
            .update_item(
                &pk,
                "METADATA",
                "SET #status = :status, updated_at = :updated_at",
                values,
                Some({
                    let mut names = HashMap::new();
                    names.insert("#status".to_string(), "status".to_string());
                    names
                }),
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List all chat threads where the user is a member.
    /// For MVP, this scans all THREAD# items and filters by member_ids.
    /// Phase 3 will add a dedicated GSI for this.
    pub async fn list_user_chats(&self, user_id: &str) -> Result<Vec<Thread>, RepoError> {
        // Scan approach: get all threads and filter client-side.
        // This is acceptable for MVP (< 100 chats per user).
        // TODO: Add GSI for user_id → thread_id lookup.
        let items = self
            .db
            .inner()
            .scan()
            .table_name(self.db.table_name())
            .filter_expression("contains(member_ids, :uid) AND SK = :meta")
            .expression_attribute_values(":uid", AttributeValue::S(user_id.to_string()))
            .expression_attribute_values(":meta", AttributeValue::S("METADATA".to_string()))
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = items.items.unwrap_or_default();
        items
            .iter()
            .filter_map(|item| {
                let pk = item.get("PK")?.as_s().ok()?;
                let thread_id = pk.strip_prefix("THREAD#")?;
                thread_from_item(item, thread_id).ok()
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(Ok)
            .collect()
    }

    /// Add a member to a chat thread (atomic — uses DynamoDB ADD on String Set).
    pub async fn add_chat_member(
        &self,
        thread_id: &str,
        user_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let mut values = HashMap::new();
        values.insert(
            ":member".to_string(),
            AttributeValue::Ss(vec![user_id.to_string()]),
        );

        self.db
            .update_item(
                &pk,
                "METADATA",
                "ADD member_ids :member",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Remove a member from a chat thread (atomic — uses DynamoDB DELETE on String Set).
    pub async fn remove_chat_member(
        &self,
        thread_id: &str,
        user_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let mut values = HashMap::new();
        values.insert(
            ":member".to_string(),
            AttributeValue::Ss(vec![user_id.to_string()]),
        );

        self.db
            .update_item(
                &pk,
                "METADATA",
                "DELETE member_ids :member",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Bump the thread's updated_at timestamp (e.g., when a new message is added).
    pub async fn bump_updated_at(
        &self,
        thread_id: &str,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let mut values = HashMap::new();
        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));

        self.db
            .update_item(
                &pk,
                "METADATA",
                "SET updated_at = :updated_at",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    // ─── Message CRUD ───────────────────────────────────────────

    /// Add a message to a thread.
    pub async fn add_message(&self, msg: &Message) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(msg.pk()));
        item.insert("SK".to_string(), AttributeValue::S(msg.sk()));
        item.insert("user_id".to_string(), AttributeValue::S(msg.user_id.clone()));
        item.insert("content".to_string(), AttributeValue::S(msg.content.clone()));
        item.insert("message_id".to_string(), AttributeValue::S(msg.message_id.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(msg.created_at.to_string()));

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get the first message in a thread (or None if no messages exist).
    pub async fn get_first_message(&self, thread_id: &str) -> Result<Option<Message>, RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let result = self.db.inner()
            .query()
            .table_name(self.db.table_name())
            .key_condition_expression("PK = :pk AND begins_with(SK, :sk)")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(":sk", AttributeValue::S("MSG#".to_string()))
            .limit(1)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        if let Some(item) = items.first() {
            Ok(Some(message_from_item(item, thread_id)?))
        } else {
            Ok(None)
        }
    }

    /// List messages in a thread (oldest first).
    pub async fn list_messages(&self, thread_id: &str) -> Result<Vec<Message>, RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let items = self
            .db
            .query(&pk, Some("MSG#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items
            .iter()
            .map(|item| message_from_item(item, thread_id))
            .collect()
    }

    /// Delete a message.
    pub async fn delete_message(
        &self,
        thread_id: &str,
        sk: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        self.db
            .delete_item(&pk, sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Delete a thread and all its messages.
    pub async fn delete_thread(&self, thread_id: &str) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        // Query all items (metadata + messages) under this PK
        let items = self.db
            .query(&pk, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        // Delete each item
        for item in &items {
            if let Some(sk) = item.get("SK").and_then(|v| v.as_s().ok()) {
                self.db.delete_item(&pk, sk)
                    .await
                    .map_err(|e| RepoError::Dynamo(e.to_string()))?;
            }
        }
        Ok(())
    }
}

// ─── Helpers ────────────────────────────────────────────────────

fn thread_from_item(
    item: &HashMap<String, AttributeValue>,
    thread_id: &str,
) -> Result<Thread, RepoError> {
    let type_str = get_s(item, "thread_type")?;
    let thread_type: ThreadType = serde_json::from_str(&format!("\"{type_str}\""))
        .map_err(|e| RepoError::MissingField(format!("thread_type: {e}")))?;

    let status_str = get_s(item, "status")?;
    let status: ThreadStatus = serde_json::from_str(&format!("\"{status_str}\""))
        .map_err(|e| RepoError::MissingField(format!("status: {e}")))?;

    let anchor_start = item
        .get("anchor_start")
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse::<u32>().ok());
    let anchor_end = item
        .get("anchor_end")
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse::<u32>().ok());

    let title = item
        .get("title")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string());
    let member_ids: Vec<String> = item
        .get("member_ids")
        .and_then(|v| v.as_ss().ok())
        .cloned()
        .unwrap_or_default();

    let block_id = item
        .get("block_id")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.to_string());

    Ok(Thread {
        thread_id: thread_id.to_string(),
        doc_id: get_s(item, "doc_id")?,
        thread_type,
        status,
        created_by: get_s(item, "created_by")?,
        title,
        member_ids,
        block_id,
        anchor_start,
        anchor_end,
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
}

fn message_from_item(
    item: &HashMap<String, AttributeValue>,
    thread_id: &str,
) -> Result<Message, RepoError> {
    Ok(Message {
        thread_id: thread_id.to_string(),
        message_id: get_s(item, "message_id")?,
        user_id: get_s(item, "user_id")?,
        content: get_s(item, "content")?,
        created_at: get_n(item, "created_at")?,
        updated_at: item
            .get("updated_at")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<i64>().ok()),
    })
}
