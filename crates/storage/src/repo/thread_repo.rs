// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for comment threads and messages.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::thread::{Mention, Message, MessagePart, Reaction, ReadReceipt, Thread, ThreadStatus, ThreadType};

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
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        // Chat/DM threads carry members; emit a reverse edge per member so
        // `list_user_chats` is a Query, not a table Scan (issue #34).
        // Comment threads have no members, so this is a no-op for them.
        for member in &thread.member_ids {
            self.put_chat_edge(member, &thread.thread_id).await?;
        }
        Ok(())
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
    /// Tries the GSI5-docid-updated GSI first; falls back to a scan only when
    /// the query fails with a classified missing-index/missing-table error
    /// (common in dev environments without full infra — see
    /// `is_missing_index_error`). Any other query failure (throttling, auth,
    /// transient service error) propagates as `RepoError::Dynamo` instead of
    /// silently degrading to a scan (issue #11).
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
            Err(err) => {
                use aws_sdk_dynamodb::error::ProvideErrorMetadata;
                // Only a genuinely absent index/table may degrade to the
                // scan; any other query failure — throttling, auth,
                // transient service error — must surface to the caller
                // instead of silently masquerading as "GSI missing" and
                // triggering a table scan in production (issue #11).
                if !is_missing_index_error(err.code(), err.message()) {
                    return Err(RepoError::Dynamo(err.to_string()));
                }
                tracing::warn!(
                    doc_id,
                    error = %err,
                    "GSI5-docid-updated unavailable; using dev scan fallback"
                );
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
    /// List the chat/DM threads a user belongs to.
    ///
    /// Reads the user's chat-membership edges (`PK=USER#<uid>,
    /// SK=CHAT#<thread_id>`, maintained by `create_thread` /
    /// `add_chat_member` / `remove_chat_member` / `delete_thread`) with
    /// a single-partition `Query`, then hydrates each thread. This
    /// replaced a full-table `Scan` filtering `contains(member_ids,
    /// uid)`, whose cost scaled with *every* thread in the table rather
    /// than the caller's own handful (issue #34).
    ///
    /// An edge whose thread has since been deleted is skipped and the
    /// stale edge is cleaned up best-effort, so the list self-heals
    /// against a partial `delete_thread`.
    pub async fn list_user_chats(&self, user_id: &str) -> Result<Vec<Thread>, RepoError> {
        let edges = self
            .db
            .query(&chat_edge_pk(user_id), Some("CHAT#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        let mut threads = Vec::with_capacity(edges.len());
        for edge in &edges {
            // Prefer the explicit attribute; fall back to parsing the SK
            // so an edge written by any historical shape still resolves.
            let Some(thread_id) = edge
                .get("thread_id")
                .and_then(|v| v.as_s().ok())
                .map(|s| s.to_string())
                .or_else(|| {
                    edge.get("SK")
                        .and_then(|v| v.as_s().ok())
                        .and_then(|sk| sk.strip_prefix("CHAT#"))
                        .map(|s| s.to_string())
                })
            else {
                continue;
            };
            match self.get_thread(&thread_id).await? {
                Some(t) => threads.push(t),
                None => {
                    // Edge outlived its thread — drop it so it doesn't
                    // resurface. Best-effort; a failure here is harmless.
                    let _ = self.delete_chat_edge(user_id, &thread_id).await;
                }
            }
        }
        Ok(threads)
    }

    /// Add a member to a chat thread — atomic across *both* writes.
    ///
    /// A DynamoDB transaction combines the `ADD member_ids :member` on the
    /// METADATA row with the `Put` of the reverse chat edge, so the two can
    /// never be observed out of sync. Before this used two independent
    /// requests: a crash or throttle between them could leave `member_ids`
    /// updated with no edge written (the member silently vanishes from
    /// their own `list_user_chats`) or an edge written with `member_ids`
    /// unchanged (a phantom chat that isn't really shared) — a disclosure
    /// failure mode either way. Same transaction pattern as
    /// `DocRepo::create_relationship` (`crates/storage/src/repo/doc_repo.rs`).
    pub async fn add_chat_member(
        &self,
        thread_id: &str,
        user_id: &str,
    ) -> Result<(), RepoError> {
        use aws_sdk_dynamodb::types::{Put, TransactWriteItem, Update};

        let pk = format!("THREAD#{thread_id}");

        let update = Update::builder()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S("METADATA".to_string()))
            .update_expression("ADD member_ids :member")
            .expression_attribute_values(
                ":member",
                AttributeValue::Ss(vec![user_id.to_string()]),
            )
            .build()
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        let mut edge_item = HashMap::new();
        edge_item.insert("PK".to_string(), AttributeValue::S(chat_edge_pk(user_id)));
        edge_item.insert("SK".to_string(), AttributeValue::S(chat_edge_sk(thread_id)));
        edge_item.insert(
            "thread_id".to_string(),
            AttributeValue::S(thread_id.to_string()),
        );
        let put = Put::builder()
            .table_name(self.db.table_name())
            .set_item(Some(edge_item))
            .build()
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        let items = vec![
            TransactWriteItem::builder().update(update).build(),
            TransactWriteItem::builder().put(put).build(),
        ];

        self.db
            .transact_write(items)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Remove a member from a chat thread — atomic across *both* writes.
    ///
    /// A DynamoDB transaction combines the `DELETE member_ids :member` on
    /// the METADATA row with the `Delete` of the reverse chat edge, so the
    /// two can never be observed out of sync. Before this used two
    /// independent requests: a crash or throttle between them could leave
    /// `member_ids` updated with the edge still present (a removed member
    /// keeps seeing the chat via `list_user_chats`, a disclosure failure)
    /// or the edge dropped with `member_ids` unchanged (the chat vanishes
    /// for a member who should still see it). Same transaction pattern as
    /// `DocRepo::delete_relationship` (`crates/storage/src/repo/doc_repo.rs`).
    pub async fn remove_chat_member(
        &self,
        thread_id: &str,
        user_id: &str,
    ) -> Result<(), RepoError> {
        use aws_sdk_dynamodb::types::{Delete, TransactWriteItem, Update};

        let pk = format!("THREAD#{thread_id}");

        let update = Update::builder()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S("METADATA".to_string()))
            .update_expression("DELETE member_ids :member")
            .expression_attribute_values(
                ":member",
                AttributeValue::Ss(vec![user_id.to_string()]),
            )
            .build()
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        let delete = Delete::builder()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(chat_edge_pk(user_id)))
            .key("SK", AttributeValue::S(chat_edge_sk(thread_id)))
            .build()
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        let items = vec![
            TransactWriteItem::builder().update(update).build(),
            TransactWriteItem::builder().delete(delete).build(),
        ];

        self.db
            .transact_write(items)
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

    /// Edit an existing message: overwrite `content`, set `updated_at`, and
    /// replace the rich `parts`/`mentions` — removing those attributes when
    /// the edit carries none, so a message that used to be rich doesn't keep
    /// stale segments that no longer match the new text. `attachments` are
    /// intentionally left untouched (editing the text shouldn't drop a file
    /// the author attached). The SK is unchanged, so message ordering and
    /// the `created_at` embedded in it are preserved.
    pub async fn update_message(
        &self,
        thread_id: &str,
        sk: &str,
        content: &str,
        parts: &[MessagePart],
        mentions: &[Mention],
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");

        // `content` is aliased via #content out of caution (reserved-word
        // safety); `updated_at` is written raw elsewhere (bump_updated_at),
        // so it needs no alias.
        let mut set_clauses = vec![
            "#content = :content".to_string(),
            "updated_at = :updated_at".to_string(),
        ];
        let mut remove_clauses: Vec<&str> = Vec::new();

        let mut values = HashMap::new();
        values.insert(":content".to_string(), AttributeValue::S(content.to_string()));
        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));

        if parts.is_empty() {
            remove_clauses.push("parts");
        } else {
            let json = serde_json::to_string(parts)
                .map_err(|e| RepoError::MissingField(format!("parts: {e}")))?;
            values.insert(":parts".to_string(), AttributeValue::S(json));
            set_clauses.push("parts = :parts".to_string());
        }
        if mentions.is_empty() {
            remove_clauses.push("mentions");
        } else {
            let json = serde_json::to_string(mentions)
                .map_err(|e| RepoError::MissingField(format!("mentions: {e}")))?;
            values.insert(":mentions".to_string(), AttributeValue::S(json));
            set_clauses.push("mentions = :mentions".to_string());
        }

        let mut expr = format!("SET {}", set_clauses.join(", "));
        if !remove_clauses.is_empty() {
            expr.push_str(&format!(" REMOVE {}", remove_clauses.join(", ")));
        }

        let mut names = HashMap::new();
        names.insert("#content".to_string(), "content".to_string());

        self.db
            .update_item(&pk, sk, &expr, values, Some(names))
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

        for pass in 0..2 {
            let items = self.db
                .query(&pk, None)
                .await
                .map_err(|e| RepoError::Dynamo(e.to_string()))?;

            if items.is_empty() {
                break;
            }

            // First pass only: read the member set off the METADATA row and
            // tear down each member's reverse chat edge (issue #34). Doing it
            // before the rows are deleted keeps `member_ids` available; a
            // failed edge delete would only leave a stale edge, which
            // `list_user_chats` self-heals.
            if pass == 0 {
                let members = items
                    .iter()
                    .find(|it| {
                        it.get("SK").and_then(|v| v.as_s().ok()) == Some(&"METADATA".to_string())
                    })
                    .and_then(|meta| meta.get("member_ids"))
                    .and_then(|v| v.as_ss().ok())
                    .cloned()
                    .unwrap_or_default();
                for member in &members {
                    self.delete_chat_edge(member, thread_id).await?;
                }
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

    // ─── Chat membership edges (issue #34) ──────────────────────────
    //
    // A user's chat list was historically a full-table `Scan` filtering
    // `contains(member_ids, uid)` — cost O(all threads) per load. We now
    // maintain a reverse edge row per (member, chat):
    //   PK = USER#<uid>, SK = CHAT#<thread_id>
    // written on create / add-member and removed on remove-member /
    // delete-thread, so `list_user_chats` is a single-partition `Query`.

    /// Write (idempotent) the reverse membership edge for one member.
    async fn put_chat_edge(&self, user_id: &str, thread_id: &str) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(chat_edge_pk(user_id)));
        item.insert("SK".to_string(), AttributeValue::S(chat_edge_sk(thread_id)));
        item.insert(
            "thread_id".to_string(),
            AttributeValue::S(thread_id.to_string()),
        );
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Remove the reverse membership edge for one member (idempotent).
    async fn delete_chat_edge(&self, user_id: &str, thread_id: &str) -> Result<(), RepoError> {
        self.db
            .delete_item(&chat_edge_pk(user_id), &chat_edge_sk(thread_id))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// One-time migration: emit chat-membership edges for every existing
    /// chat/DM thread. Idempotent (edges are put-overwrites), so it is safe
    /// to re-run. Returns `(threads_scanned, edges_written)`. This is the
    /// only place that still Scans for chat membership — run once at the
    /// #34 cutover, before the new `list_user_chats` becomes authoritative,
    /// so existing chats don't briefly disappear from users' lists.
    pub async fn backfill_chat_edges(&self) -> Result<(usize, usize), RepoError> {
        let mut scanned = 0usize;
        let mut written = 0usize;
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let mut builder = self
                .db
                .inner()
                .scan()
                .table_name(self.db.table_name())
                .filter_expression("SK = :meta AND attribute_exists(member_ids)")
                .expression_attribute_values(":meta", AttributeValue::S("METADATA".to_string()));
            if let Some(start) = start_key.take() {
                builder = builder.set_exclusive_start_key(Some(start));
            }
            let result = builder
                .send()
                .await
                .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

            for item in result.items.unwrap_or_default() {
                let Some(thread_id) = item
                    .get("PK")
                    .and_then(|v| v.as_s().ok())
                    .and_then(|pk| pk.strip_prefix("THREAD#"))
                else {
                    continue;
                };
                scanned += 1;
                let members = item
                    .get("member_ids")
                    .and_then(|v| v.as_ss().ok())
                    .cloned()
                    .unwrap_or_default();
                for member in &members {
                    self.put_chat_edge(member, thread_id).await?;
                    written += 1;
                }
            }

            match result.last_evaluated_key {
                Some(key) => start_key = Some(key),
                None => break,
            }
        }
        Ok((scanned, written))
    }
}

/// Partition key for a user's chat-membership edges (issue #34).
fn chat_edge_pk(user_id: &str) -> String {
    format!("USER#{user_id}")
}

/// Sort key for the edge from a user to one chat thread (issue #34).
fn chat_edge_sk(thread_id: &str) -> String {
    format!("CHAT#{thread_id}")
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

/// True when a Query error means the target index (or its table) doesn't
/// exist — the one condition `list_threads_for_doc`'s dev scan fallback
/// is for (issue #11). DynamoDB reports a missing GSI on an existing
/// table as `ValidationException` with a message naming the index, and a
/// missing table as `ResourceNotFoundException`. Anything else —
/// throttling, auth, transient service errors — is a real failure that
/// must surface to the caller, not silently degrade into a table scan.
fn is_missing_index_error(code: Option<&str>, message: Option<&str>) -> bool {
    match code {
        // Missing table entirely (bare dev environment). If the table is
        // truly gone the fallback scan fails too, so this can't mask a
        // real outage — it just keeps the two absent-infra cases on the
        // same path.
        Some("ResourceNotFoundException") => true,
        // Missing GSI on an existing table. DynamoDB (and DDB Local)
        // phrase it "The table does not have the specified index: ..." —
        // other ValidationExceptions (bad key condition, reserved word)
        // are code bugs and must propagate.
        Some("ValidationException") => {
            // Query's smithy model has no dedicated ValidationException
            // variant (unlike ResourceNotFoundException) — it always lands
            // in Unhandled, so message-matching is the only mechanism the
            // SDK exposes for distinguishing "missing index" from other
            // validation failures.
            message.is_some_and(|m| m.contains("does not have the specified index"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::thread::{Mention, MentionType, MessagePart, PartStyle};

    fn thread_fixture() -> Thread {
        Thread {
            thread_id: "t1".to_string(),
            doc_id: "doc1".to_string(),
            thread_type: ThreadType::Chat,
            status: ThreadStatus::Open,
            created_by: "u1".to_string(),
            title: Some("Team chat".to_string()),
            member_ids: vec!["u1".to_string(), "u2".to_string()],
            block_id: Some("blk-1".to_string()),
            anchor_start: Some(3),
            anchor_end: Some(17),
            created_at: 100,
            updated_at: 200,
        }
    }

    /// Mimic `create_thread`'s column construction (no live table),
    /// including its conditional writes for the optional fields.
    fn thread_item(thread: &Thread) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(thread.pk()));
        item.insert("SK".to_string(), AttributeValue::S(thread.sk()));
        item.insert("doc_id".to_string(), AttributeValue::S(thread.doc_id.clone()));
        item.insert(
            "thread_type".to_string(),
            AttributeValue::S(
                serde_json::to_string(&thread.thread_type).unwrap().trim_matches('"').to_string(),
            ),
        );
        item.insert(
            "status".to_string(),
            AttributeValue::S(
                serde_json::to_string(&thread.status).unwrap().trim_matches('"').to_string(),
            ),
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
            item.insert("member_ids".to_string(), AttributeValue::Ss(thread.member_ids.clone()));
        }
        item.insert("created_at".to_string(), AttributeValue::N(thread.created_at.to_string()));
        item.insert("updated_at".to_string(), AttributeValue::N(thread.updated_at.to_string()));
        item
    }

    #[test]
    fn thread_round_trips_with_all_optional_fields() {
        let thread = thread_fixture();
        let back = thread_from_item(&thread_item(&thread), "t1").expect("from_item");
        assert_eq!(back.thread_id, "t1");
        assert_eq!(back.doc_id, thread.doc_id);
        assert_eq!(back.thread_type, thread.thread_type);
        assert_eq!(back.status, thread.status);
        assert_eq!(back.created_by, thread.created_by);
        assert_eq!(back.title, thread.title);
        assert_eq!(back.member_ids, thread.member_ids);
        assert_eq!(back.block_id, thread.block_id);
        assert_eq!(back.anchor_start, thread.anchor_start);
        assert_eq!(back.anchor_end, thread.anchor_end);
        assert_eq!(back.created_at, thread.created_at);
        assert_eq!(back.updated_at, thread.updated_at);
    }

    #[test]
    fn thread_minimal_row_decodes_optionals_as_absent() {
        // An inline-comment row written without title/members/anchors
        // (the create path omits them) must decode to None / empty,
        // not error.
        let mut thread = thread_fixture();
        thread.thread_type = ThreadType::Inline;
        thread.title = None;
        thread.member_ids = Vec::new();
        thread.block_id = None;
        thread.anchor_start = None;
        thread.anchor_end = None;
        let back = thread_from_item(&thread_item(&thread), "t1").expect("from_item");
        assert_eq!(back.title, None);
        assert!(back.member_ids.is_empty());
        assert_eq!(back.block_id, None);
        assert_eq!(back.anchor_start, None);
        assert_eq!(back.anchor_end, None);
    }

    #[test]
    fn thread_type_round_trips_for_every_variant() {
        // create_thread stores the serde tag ("inline" / "document" /
        // "chat" / "directMessage"); the decoder must accept each one.
        for tt in [
            ThreadType::Inline,
            ThreadType::Document,
            ThreadType::Chat,
            ThreadType::DirectMessage,
        ] {
            let mut thread = thread_fixture();
            thread.thread_type = tt.clone();
            let back = thread_from_item(&thread_item(&thread), "t1")
                .unwrap_or_else(|e| panic!("roundtrip failed for {tt:?}: {e}"));
            assert_eq!(back.thread_type, tt);
        }
    }

    #[test]
    fn thread_unknown_type_or_status_errors() {
        let mut item = thread_item(&thread_fixture());
        item.insert("thread_type".to_string(), AttributeValue::S("broadcast".to_string()));
        match thread_from_item(&item, "t1") {
            Err(RepoError::MissingField(msg)) => {
                assert!(msg.contains("thread_type"), "must name the field: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }

        let mut item = thread_item(&thread_fixture());
        item.insert("status".to_string(), AttributeValue::S("archived".to_string()));
        match thread_from_item(&item, "t1") {
            Err(RepoError::MissingField(msg)) => {
                assert!(msg.contains("status"), "must name the field: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    fn message_fixture() -> Message {
        Message {
            thread_id: "t1".to_string(),
            message_id: "m1".to_string(),
            user_id: "u1".to_string(),
            content: "hello".to_string(),
            created_at: 42,
            updated_at: None,
            parts: vec![MessagePart {
                style: PartStyle::Body,
                text: "hello".to_string(),
            }],
            mentions: vec![Mention {
                mention_type: MentionType::Person,
                id: "u2".to_string(),
                label: "Bob".to_string(),
            }],
            attachments: vec!["blob-key-1".to_string()],
        }
    }

    /// Mimic `add_message`'s column construction (no live table).
    fn message_item(msg: &Message) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(msg.pk()));
        item.insert("SK".to_string(), AttributeValue::S(msg.sk()));
        item.insert("user_id".to_string(), AttributeValue::S(msg.user_id.clone()));
        item.insert("content".to_string(), AttributeValue::S(msg.content.clone()));
        item.insert("message_id".to_string(), AttributeValue::S(msg.message_id.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(msg.created_at.to_string()));
        if !msg.parts.is_empty() {
            item.insert(
                "parts".to_string(),
                AttributeValue::S(serde_json::to_string(&msg.parts).unwrap()),
            );
        }
        if !msg.mentions.is_empty() {
            item.insert(
                "mentions".to_string(),
                AttributeValue::S(serde_json::to_string(&msg.mentions).unwrap()),
            );
        }
        if !msg.attachments.is_empty() {
            item.insert(
                "attachments".to_string(),
                AttributeValue::S(serde_json::to_string(&msg.attachments).unwrap()),
            );
        }
        item
    }

    #[test]
    fn message_round_trips_parts_mentions_attachments() {
        let msg = message_fixture();
        let back = message_from_item(&message_item(&msg), "t1").expect("from_item");
        assert_eq!(back.message_id, msg.message_id);
        assert_eq!(back.content, msg.content);
        assert_eq!(back.parts.len(), 1);
        assert_eq!(back.parts[0].style, PartStyle::Body);
        assert_eq!(back.parts[0].text, "hello");
        assert_eq!(back.mentions.len(), 1);
        assert_eq!(back.mentions[0].mention_type, MentionType::Person);
        assert_eq!(back.mentions[0].id, "u2");
        assert_eq!(back.attachments, vec!["blob-key-1".to_string()]);
        assert_eq!(back.updated_at, None);
    }

    #[test]
    fn message_without_rich_fields_decodes_empty() {
        // add_message omits parts/mentions/attachments when empty;
        // the decode must fill empty Vecs, not error.
        let mut msg = message_fixture();
        msg.parts = Vec::new();
        msg.mentions = Vec::new();
        msg.attachments = Vec::new();
        let back = message_from_item(&message_item(&msg), "t1").expect("from_item");
        assert!(back.parts.is_empty());
        assert!(back.mentions.is_empty());
        assert!(back.attachments.is_empty());
    }

    #[test]
    fn message_corrupt_parts_json_degrades_to_empty() {
        // Graceful-degrade posture: a corrupt `parts` blob must not
        // make the whole message unreadable — the plain `content`
        // column still renders. (The decoder logs and returns empty.)
        let mut item = message_item(&message_fixture());
        item.insert("parts".to_string(), AttributeValue::S("{not json".to_string()));
        let back = message_from_item(&item, "t1").expect("corrupt parts must not fail decode");
        assert!(back.parts.is_empty());
        assert_eq!(back.content, "hello", "content survives a corrupt parts blob");
    }

    #[test]
    fn message_missing_content_errors() {
        let mut item = message_item(&message_fixture());
        item.remove("content");
        match message_from_item(&item, "t1") {
            Err(RepoError::MissingField(f)) => assert_eq!(f, "content"),
            other => panic!("expected MissingField(content), got {other:?}"),
        }
    }

    #[test]
    fn reaction_round_trips_and_tolerates_missing_user_ids() {
        // Shape written by add_reaction (ADD user_ids + SET message_id,
        // emoji).
        let mut item = HashMap::new();
        item.insert("message_id".to_string(), AttributeValue::S("m1".to_string()));
        item.insert("emoji".to_string(), AttributeValue::S("👍".to_string()));
        item.insert(
            "user_ids".to_string(),
            AttributeValue::Ss(vec!["u1".to_string(), "u2".to_string()]),
        );
        let back = reaction_from_item(&item, "t1").expect("from_item");
        assert_eq!(back.thread_id, "t1");
        assert_eq!(back.message_id, "m1");
        assert_eq!(back.emoji, "👍");
        assert_eq!(back.user_ids, vec!["u1".to_string(), "u2".to_string()]);

        // The narrow race documented on list_reactions_for_thread: a
        // last-user removal can leave a row whose user_ids attribute
        // was dropped by DynamoDB. It must decode as empty, not error,
        // so callers can filter it out.
        item.remove("user_ids");
        let back = reaction_from_item(&item, "t1").expect("empty reaction row must decode");
        assert!(back.user_ids.is_empty());
    }

    #[test]
    fn read_receipt_round_trips() {
        // Shape written by upsert_read_receipt.
        let mut item = HashMap::new();
        item.insert("user_id".to_string(), AttributeValue::S("u1".to_string()));
        item.insert(
            "last_read_at".to_string(),
            AttributeValue::N("1700000000000000".to_string()),
        );
        let back = read_receipt_from_item(&item, "t1").expect("from_item");
        assert_eq!(back.thread_id, "t1");
        assert_eq!(back.user_id, "u1");
        assert_eq!(back.last_read_at, 1_700_000_000_000_000);
    }

    /// Issue #11: only genuinely-missing-index errors may trigger the
    /// scan fallback; throttling/auth/transient errors must propagate.
    #[test]
    fn missing_index_classifier_accepts_only_absent_index_conditions() {
        // Missing GSI on an existing table (real DynamoDB and DDB Local
        // both phrase it this way).
        assert!(is_missing_index_error(
            Some("ValidationException"),
            Some("The table does not have the specified index: GSI5-docid-updated"),
        ));
        // Missing table entirely (bare dev environment).
        assert!(is_missing_index_error(
            Some("ResourceNotFoundException"),
            Some("Requested resource not found"),
        ));

        // Real failures must NOT fall back to a scan.
        assert!(!is_missing_index_error(
            Some("ProvisionedThroughputExceededException"),
            Some("The level of configured provisioned throughput for the table was exceeded"),
        ));
        assert!(!is_missing_index_error(
            Some("AccessDeniedException"),
            Some("User is not authorized to perform: dynamodb:Query"),
        ));
        // A ValidationException about something other than a missing
        // index (e.g. a bad key condition) is a code bug, not a
        // missing-infra condition.
        assert!(!is_missing_index_error(
            Some("ValidationException"),
            Some("Invalid KeyConditionExpression: Attribute name is a reserved keyword"),
        ));
        assert!(!is_missing_index_error(None, None));
    }
}
