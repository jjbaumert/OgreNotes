// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for comment threads and messages.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::thread::{Message, Reaction, ReadReceipt, Thread, ThreadStatus, ThreadType};

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
        if !thread.doc_id.is_empty() {
            item.insert(
                "doc_id_gsi".to_string(),
                AttributeValue::S(thread.doc_id.clone()),
            );
        }
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
                // This is slower but works without the GSI. Dev path
                // only; production has the GSI and never reaches
                // here.
                //
                // Cap at 500 threads/doc (#39): well above any
                // realistic per-doc thread count, low enough to
                // prevent an unbounded scan from materializing
                // arbitrarily many items into memory. If a doc
                // legitimately exceeds the cap the GSI is the
                // answer, not lifting the bound.
                const MAX_THREADS_PER_DOC_SCAN: usize = 500;
                let (items, truncated) = self.db.scan_with_filter(
                    "doc_id",
                    doc_id,
                    MAX_THREADS_PER_DOC_SCAN,
                ).await.map_err(|e| RepoError::Dynamo(e.to_string()))?;
                if truncated {
                    tracing::warn!(
                        doc_id,
                        cap = MAX_THREADS_PER_DOC_SCAN,
                        "list_threads_for_doc scan hit the row cap — \
                         GSI5-docid-updated likely missing and the doc \
                         has more threads than the dev-fallback budget"
                    );
                }
                items
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

    /// Update anchor positions for an inline comment thread.
    pub async fn update_anchors(
        &self,
        thread_id: &str,
        anchor_start: u32,
        anchor_end: u32,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let mut values = HashMap::new();
        values.insert(":start".to_string(), AttributeValue::N(anchor_start.to_string()));
        values.insert(":end".to_string(), AttributeValue::N(anchor_end.to_string()));

        self.db
            .update_item(
                &pk,
                "METADATA",
                "SET anchor_start = :start, anchor_end = :end",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List all chat threads where the user is a member.
    ///
    /// Full-table scan filtered to `METADATA` rows whose `member_ids`
    /// contains the user. Paginates on `LastEvaluatedKey` so the user's
    /// chats are never silently truncated: a single-page scan returns only
    /// the first ~1 MB *scanned* (pre-filter), so once the table exceeds
    /// that it could drop a user's chats regardless of how few they have.
    /// Still O(table size); the real perf fix is a membership-row model +
    /// GSI for user → thread, tracked separately.
    pub async fn list_user_chats(&self, user_id: &str) -> Result<Vec<Thread>, RepoError> {
        let mut threads = Vec::new();
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let mut builder = self
                .db
                .inner()
                .scan()
                .table_name(self.db.table_name())
                .filter_expression("contains(member_ids, :uid) AND SK = :meta")
                .expression_attribute_values(":uid", AttributeValue::S(user_id.to_string()))
                .expression_attribute_values(":meta", AttributeValue::S("METADATA".to_string()));
            if let Some(start) = start_key.take() {
                builder = builder.set_exclusive_start_key(Some(start));
            }
            let result = builder
                .send()
                .await
                .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

            for item in result.items.unwrap_or_default() {
                if let Some(thread_id) = item
                    .get("PK")
                    .and_then(|v| v.as_s().ok())
                    .and_then(|pk| pk.strip_prefix("THREAD#"))
                {
                    if let Ok(t) = thread_from_item(&item, thread_id) {
                        threads.push(t);
                    }
                }
            }

            match result.last_evaluated_key {
                Some(key) => start_key = Some(key),
                None => break,
            }
        }
        Ok(threads)
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

        if !msg.parts.is_empty() {
            let json = serde_json::to_string(&msg.parts)
                .map_err(|e| RepoError::MissingField(format!("parts: {e}")))?;
            item.insert("parts".to_string(), AttributeValue::S(json));
        }
        if !msg.mentions.is_empty() {
            let json = serde_json::to_string(&msg.mentions)
                .map_err(|e| RepoError::MissingField(format!("mentions: {e}")))?;
            item.insert("mentions".to_string(), AttributeValue::S(json));
        }
        if !msg.attachments.is_empty() {
            let json = serde_json::to_string(&msg.attachments)
                .map_err(|e| RepoError::MissingField(format!("attachments: {e}")))?;
            item.insert("attachments".to_string(), AttributeValue::S(json));
        }

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

    /// Count the messages in a thread.
    ///
    /// Uses a `Select::Count` query so DynamoDB returns only the tally,
    /// not the item bodies — cheap enough to call per-thread in the
    /// thread-list view alongside `get_first_message`. Comment threads
    /// stay well under the 1 MB single-page scan limit, so the returned
    /// count is exact. Best-effort callers treat an error as zero.
    pub async fn count_messages(&self, thread_id: &str) -> Result<u32, RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .key_condition_expression("PK = :pk AND begins_with(SK, :sk)")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(":sk", AttributeValue::S("MSG#".to_string()))
            .select(aws_sdk_dynamodb::types::Select::Count)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;
        Ok(result.count().max(0) as u32)
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

    // ─── Reactions ──────────────────────────────────────────────

    /// Add a user's reaction to a message. Idempotent: DynamoDB string sets
    /// deduplicate, so adding the same (message, emoji, user) twice is a
    /// no-op. The row stores the emoji string explicitly so callers can
    /// reconstruct it without parsing the SK.
    pub async fn add_reaction(
        &self,
        thread_id: &str,
        message_id: &str,
        emoji: &str,
        user_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let sk = format!("REACTION#{message_id}#{emoji}");

        self.db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(sk))
            .update_expression(
                "ADD user_ids :uid SET message_id = :mid, emoji = :emoji",
            )
            .expression_attribute_values(
                ":uid",
                AttributeValue::Ss(vec![user_id.to_string()]),
            )
            .expression_attribute_values(":mid", AttributeValue::S(message_id.to_string()))
            .expression_attribute_values(":emoji", AttributeValue::S(emoji.to_string()))
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;
        Ok(())
    }

    /// Remove a user's reaction from a message. If the user wasn't in the
    /// set, the DELETE is a no-op. When removing the last user, the
    /// `user_ids` attribute is dropped by DynamoDB; we then delete the row
    /// so subsequent list queries don't return empty reactions.
    pub async fn remove_reaction(
        &self,
        thread_id: &str,
        message_id: &str,
        emoji: &str,
        user_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let sk = format!("REACTION#{message_id}#{emoji}");

        // DELETE from the set. Returns the updated item so we can decide
        // whether to drop the row entirely.
        let result = self
            .db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk.clone()))
            .key("SK", AttributeValue::S(sk.clone()))
            .update_expression("DELETE user_ids :uid")
            .expression_attribute_values(
                ":uid",
                AttributeValue::Ss(vec![user_id.to_string()]),
            )
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let is_empty = result
            .attributes
            .as_ref()
            .map(|a| {
                a.get("user_ids")
                    .and_then(|v| v.as_ss().ok())
                    .map(|s| s.is_empty())
                    .unwrap_or(true)
            })
            .unwrap_or(true);

        if is_empty {
            self.db
                .delete_item(&pk, &sk)
                .await
                .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        }
        Ok(())
    }

    /// List every reaction row on a thread, grouped per message. Returned
    /// reactions may have `user_ids.is_empty()` in the narrow race where a
    /// last-user removal left an empty row before cleanup; callers should
    /// filter those out.
    pub async fn list_reactions_for_thread(
        &self,
        thread_id: &str,
    ) -> Result<Vec<Reaction>, RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let items = self
            .db
            .query(&pk, Some("REACTION#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items
            .iter()
            .map(|item| reaction_from_item(item, thread_id))
            .collect()
    }

    // ─── Read receipts ──────────────────────────────────────────

    /// Record or bump the caller's read timestamp for a thread. `last_read_at`
    /// is strictly monotonic per user — if a stale concurrent call would
    /// *decrease* it, the conditional expression keeps the existing higher
    /// value. (Without the guard, an out-of-order GET could roll a
    /// newer-read user back to an older timestamp and break unread-badge
    /// logic.)
    pub async fn upsert_read_receipt(
        &self,
        thread_id: &str,
        user_id: &str,
        last_read_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let sk = format!("READ#{user_id}");

        let result = self
            .db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(sk))
            .update_expression("SET last_read_at = :ts, user_id = :uid")
            .condition_expression(
                "attribute_not_exists(last_read_at) OR last_read_at < :ts",
            )
            .expression_attribute_values(":ts", AttributeValue::N(last_read_at.to_string()))
            .expression_attribute_values(":uid", AttributeValue::S(user_id.to_string()))
            .send()
            .await;

        // Condition-check failure means an equal-or-newer receipt already
        // exists; not an error from the caller's perspective.
        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                let svc = e.into_service_error();
                if svc.is_conditional_check_failed_exception() {
                    Ok(())
                } else {
                    Err(RepoError::Dynamo(svc.to_string()))
                }
            }
        }
    }

    /// List every read receipt on a thread. Used by thread/chat summary
    /// endpoints so the frontend can render read/unread state.
    pub async fn list_read_receipts_for_thread(
        &self,
        thread_id: &str,
    ) -> Result<Vec<ReadReceipt>, RepoError> {
        let pk = format!("THREAD#{thread_id}");
        let items = self
            .db
            .query(&pk, Some("READ#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items
            .iter()
            .map(|item| read_receipt_from_item(item, thread_id))
            .collect()
    }

    /// Delete a thread and all its messages, reactions, and read receipts.
    /// Retries once to catch items added concurrently during the first pass.
    pub async fn delete_thread(&self, thread_id: &str) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");

        for _pass in 0..2 {
            let items = self.db
                .query(&pk, None)
                .await
                .map_err(|e| RepoError::Dynamo(e.to_string()))?;

            if items.is_empty() {
                break;
            }

            for item in &items {
                if let Some(sk) = item.get("SK").and_then(|v| v.as_s().ok()) {
                    self.db.delete_item(&pk, sk)
                        .await
                        .map_err(|e| RepoError::Dynamo(e.to_string()))?;
                }
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
    let parts = item
        .get("parts")
        .and_then(|v| v.as_s().ok())
        .map(|s| serde_json::from_str(s).unwrap_or_else(|e| {
            tracing::warn!(thread_id, "failed to deserialize message parts: {e}");
            Vec::new()
        }))
        .unwrap_or_default();
    let mentions = item
        .get("mentions")
        .and_then(|v| v.as_s().ok())
        .map(|s| serde_json::from_str(s).unwrap_or_else(|e| {
            tracing::warn!(thread_id, "failed to deserialize message mentions: {e}");
            Vec::new()
        }))
        .unwrap_or_default();
    let attachments = item
        .get("attachments")
        .and_then(|v| v.as_s().ok())
        .map(|s| serde_json::from_str(s).unwrap_or_else(|e| {
            tracing::warn!(thread_id, "failed to deserialize message attachments: {e}");
            Vec::new()
        }))
        .unwrap_or_default();

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
        parts,
        mentions,
        attachments,
    })
}

fn reaction_from_item(
    item: &HashMap<String, AttributeValue>,
    thread_id: &str,
) -> Result<Reaction, RepoError> {
    let user_ids: Vec<String> = item
        .get("user_ids")
        .and_then(|v| v.as_ss().ok())
        .cloned()
        .unwrap_or_default();

    Ok(Reaction {
        thread_id: thread_id.to_string(),
        message_id: get_s(item, "message_id")?,
        emoji: get_s(item, "emoji")?,
        user_ids,
    })
}

fn read_receipt_from_item(
    item: &HashMap<String, AttributeValue>,
    thread_id: &str,
) -> Result<ReadReceipt, RepoError> {
    Ok(ReadReceipt {
        thread_id: thread_id.to_string(),
        user_id: get_s(item, "user_id")?,
        last_read_at: get_n(item, "last_read_at")?,
    })
}
