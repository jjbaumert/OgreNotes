// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::import::{ImportRecord, ImportStatus};
use crate::models::import_inventory::{
    FolderRow, PendingLinkItem, SecMapRow, ThreadRow, ThreadState, UnresolvedRow,
    UNRESOLVED_CHUNK_LINKS,
};
use crate::repo::{RepoError, get_n, get_n_u64, get_s};

/// Repository for the Quip import manifest (`IMPORT#<id>` / `META`).
///
/// Deliberately narrow: this repo never reads or writes a token. The Quip
/// token lives in the `TokenStore` (Phase 0 Task 4); wiring the two
/// together happens at the endpoint/worker layer, not here.
pub struct ImportRepo {
    db: DynamoClient,
}

impl ImportRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Create a new import record. Fails if one already exists for this
    /// `import_id` (conditional on `attribute_not_exists(PK)`) — import IDs
    /// are generated fresh per run, so a conflict here means a caller bug
    /// (id reuse) or a concurrent double-create, not a legitimate update.
    pub async fn create(&self, record: &ImportRecord) -> Result<(), RepoError> {
        let mut item = import_to_item(record);
        item.insert("PK".to_string(), AttributeValue::S(record.pk()));
        item.insert(
            "SK".to_string(),
            AttributeValue::S(ImportRecord::sk().to_string()),
        );

        self.db
            .put_item_conditional(item, "attribute_not_exists(PK)")
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Fetch an import record by id.
    pub async fn get(&self, import_id: &str) -> Result<Option<ImportRecord>, RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let item = self
            .db
            .get_item(&pk, ImportRecord::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(import_from_item(&item)?)),
            None => Ok(None),
        }
    }

    /// Update just the status, bumping `updated_at` to now.
    pub async fn set_status(
        &self,
        import_id: &str,
        status: ImportStatus,
    ) -> Result<(), RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(
            ":status".to_string(),
            AttributeValue::S(status_to_str(status).to_string()),
        );
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(ogrenotes_common::time::now_usec().to_string()),
        );

        self.db
            .update_item(
                &pk,
                ImportRecord::sk(),
                "SET #status = :status, updated_at = :updated_at",
                values,
                Some(HashMap::from([("#status".to_string(), "status".to_string())])),
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Write a folder row discovered during inventory BFS. Folders are
    /// idempotent to re-write (unlike threads, they carry no progress
    /// state that a re-run could downgrade), so a plain `put_item`
    /// unconditionally upserts.
    pub async fn put_folder(&self, import_id: &str, f: &FolderRow) -> Result<(), RepoError> {
        let mut item = folder_to_item(f);
        item.insert("PK".to_string(), AttributeValue::S(format!("IMPORT#{import_id}")));
        item.insert("SK".to_string(), AttributeValue::S(f.sk()));
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Insert-if-absent: a re-run must never downgrade a thread that has
    /// advanced past `Pending` (Phase 2+). A conditional-check failure means
    /// the row already exists — treat as success, leave it as-is.
    pub async fn put_thread(&self, import_id: &str, t: &ThreadRow) -> Result<(), RepoError> {
        let mut item = thread_to_item(t);
        item.insert("PK".to_string(), AttributeValue::S(format!("IMPORT#{import_id}")));
        item.insert("SK".to_string(), AttributeValue::S(t.sk()));
        match self
            .db
            .put_item_conditional(item, "attribute_not_exists(SK)")
            .await
        {
            Ok(()) => Ok(()),
            Err(e) if is_conditional_check_failure(&e) => Ok(()),
            Err(e) => Err(RepoError::Dynamo(e.to_string())),
        }
    }

    /// List every folder row inventoried for this import.
    pub async fn list_folders(&self, import_id: &str) -> Result<Vec<FolderRow>, RepoError> {
        let items = self
            .db
            .query(&format!("IMPORT#{import_id}"), Some("FOLDER#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        items.iter().map(folder_from_item).collect()
    }

    /// List every thread row inventoried for this import.
    pub async fn list_threads(&self, import_id: &str) -> Result<Vec<ThreadRow>, RepoError> {
        let items = self
            .db
            .query(&format!("IMPORT#{import_id}"), Some("THREAD#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        items.iter().map(thread_from_item).collect()
    }

    /// `(total, done_past_pending)` — used by the worker/API to report
    /// inventory progress without materializing the full row set twice.
    pub async fn count_threads_by_state(&self, import_id: &str) -> Result<(usize, usize), RepoError> {
        let rows = self.list_threads(import_id).await?;
        let total = rows.len();
        let done = rows.iter().filter(|r| r.state != ThreadState::Pending).count();
        Ok((total, done))
    }

    /// Write one chunk of a thread's section-id → block-id map. Chunks are
    /// idempotent to re-write (same rationale as `put_folder`: a chunk
    /// carries no progress state a re-run could downgrade), so a plain
    /// `put_item` unconditionally upserts.
    pub async fn put_secmap(&self, import_id: &str, row: &SecMapRow) -> Result<(), RepoError> {
        let mut item = secmap_to_item(row);
        item.insert("PK".to_string(), AttributeValue::S(format!("IMPORT#{import_id}")));
        item.insert("SK".to_string(), AttributeValue::S(row.sk()));
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Read a thread's full section-id → block-id map, concatenating all
    /// `SECMAP#<thread>#<chunk>` rows in numeric chunk order. Must not
    /// rely on the SK's lexicographic order — `#10` sorts before `#2` as
    /// strings — so chunks are sorted by the parsed `chunk` field.
    pub async fn get_secmap(
        &self,
        import_id: &str,
        quip_thread_id: &str,
    ) -> Result<Vec<(String, String)>, RepoError> {
        let items = self
            .db
            .query(&format!("IMPORT#{import_id}"), Some(&format!("SECMAP#{quip_thread_id}#")))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        let mut rows: Vec<SecMapRow> = items.iter().map(secmap_from_item).collect::<Result<_, _>>()?;
        rows.sort_by_key(|r| r.chunk);
        Ok(rows.into_iter().flat_map(|r| r.entries).collect())
    }

    /// Write the set of cross-thread links discovered in one source
    /// thread that couldn't be resolved yet. Plain upsert, same rationale
    /// as `put_secmap` — the caller passes the complete current set.
    ///
    /// Split across `UNRESOLVED#<thread>#<chunk>` rows of at most
    /// [`UNRESOLVED_CHUNK_LINKS`] links each so a link-dense source thread
    /// (a Quip index/directory page) can't blow DynamoDB's 400 KB item cap
    /// — which would surface to the content pass as a transient error and
    /// cost it a retry. Chunking lives here rather than in the caller
    /// (unlike `SECMAP#`) so the row shape stays an invisible storage
    /// detail; `list_unresolved` reassembles it.
    ///
    /// Overwrite semantics: each chunk is upserted independently, so
    /// re-writing a *shorter* link set for the same thread would leave the
    /// surplus tail chunks behind. The only caller re-derives the set from
    /// the same staged HTML, so a rewrite is byte-identical; a future
    /// caller that can genuinely shrink a set must delete the tail.
    pub async fn put_unresolved(&self, import_id: &str, row: &UnresolvedRow) -> Result<(), RepoError> {
        // `[].chunks(n)` yields nothing; an empty set still gets one row so
        // "written but link-free" is distinguishable from "never written".
        let batches: Vec<&[PendingLinkItem]> = if row.links.is_empty() {
            vec![&[]]
        } else {
            row.links.chunks(UNRESOLVED_CHUNK_LINKS).collect()
        };
        for (chunk, links) in batches.into_iter().enumerate() {
            let chunk = u32::try_from(chunk)
                .map_err(|_| RepoError::MissingField("unresolved chunk overflow".to_string()))?;
            let part = UnresolvedRow {
                source_quip_thread_id: row.source_quip_thread_id.clone(),
                owner_id: row.owner_id.clone(),
                links: links.to_vec(),
            };
            let mut item = unresolved_to_item(&part);
            item.insert("chunk".to_string(), AttributeValue::N(chunk.to_string()));
            item.insert("PK".to_string(), AttributeValue::S(format!("IMPORT#{import_id}")));
            item.insert("SK".to_string(), AttributeValue::S(row.sk(chunk)));
            self.db
                .put_item(item)
                .await
                .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        }
        Ok(())
    }

    /// List every unresolved-link row recorded for this import, one entry
    /// per source thread with its chunks concatenated back into a single
    /// link list. Ordering is the write order: chunks are merged by their
    /// parsed numeric `chunk` (never the SK's lexicographic order, where
    /// `#10` precedes `#2`), exactly as `get_secmap` does.
    pub async fn list_unresolved(&self, import_id: &str) -> Result<Vec<UnresolvedRow>, RepoError> {
        let items = self
            .db
            .query(&format!("IMPORT#{import_id}"), Some("UNRESOLVED#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        let mut parts: Vec<(u32, UnresolvedRow)> = items
            .iter()
            .map(|i| Ok((unresolved_chunk_from_item(i)?, unresolved_from_item(i)?)))
            .collect::<Result<_, RepoError>>()?;
        parts.sort_by(|(a_chunk, a), (b_chunk, b)| {
            a.source_quip_thread_id
                .cmp(&b.source_quip_thread_id)
                .then(a_chunk.cmp(b_chunk))
        });
        let mut out: Vec<UnresolvedRow> = Vec::new();
        for (_, part) in parts {
            match out.last_mut() {
                Some(last) if last.source_quip_thread_id == part.source_quip_thread_id => {
                    last.links.extend(part.links);
                }
                _ => out.push(part),
            }
        }
        Ok(out)
    }

    /// Reserve this thread's future `ogre_doc_id` *before* any document is
    /// created under it, and hand back the id that is actually reserved.
    ///
    /// This is what makes the per-thread import idempotent. Steps after
    /// `DocRepo::create` (section map, unresolved links, the `ContentDone`
    /// checkpoint) can each fail transiently; the queue then retries a
    /// thread that is still `Pending`, and if it minted a *fresh* id every
    /// time, one DynamoDB throttle would deterministically leave the user
    /// with two copies of the same document (up to four across the retry
    /// budget). Reserving first means the retry re-uses the same id and
    /// reconciles with the document it already created.
    ///
    /// Conditional on `attribute_not_exists(ogre_doc_id)`, so a rare
    /// double-claim converges: the loser reads the winner's id back rather
    /// than overwriting it. Returns `candidate` when the reservation won.
    pub async fn reserve_thread_doc_id(
        &self,
        import_id: &str,
        quip_thread_id: &str,
        candidate: &str,
    ) -> Result<String, RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let sk = format!("THREAD#{quip_thread_id}");
        let mut values = HashMap::new();
        values.insert(":doc".to_string(), AttributeValue::S(candidate.to_string()));
        let reserved = self
            .db
            .update_item_conditional(
                &pk,
                &sk,
                "SET ogre_doc_id = :doc",
                "attribute_not_exists(ogre_doc_id)",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        if reserved {
            return Ok(candidate.to_string());
        }
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?
            .ok_or_else(|| RepoError::MissingField(format!("thread row {sk}")))?;
        get_s(&item, "ogre_doc_id")
    }

    /// Advance a thread's `THREAD#` row to `ContentDone` and stamp the
    /// resulting `ogre_doc_id` / `content_s3_key`. Plain `update_item` —
    /// unlike `put_thread`'s insert-if-absent, the row already exists
    /// from Phase 1's inventory, and this is a forward-only checkpoint a
    /// re-run can safely repeat (idempotent: same final state either way).
    pub async fn set_thread_content_done(
        &self,
        import_id: &str,
        quip_thread_id: &str,
        ogre_doc_id: &str,
        content_s3_key: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(
            ":state".to_string(),
            AttributeValue::S(thread_state_to_str(ThreadState::ContentDone).to_string()),
        );
        values.insert(":doc".to_string(), AttributeValue::S(ogre_doc_id.to_string()));
        values.insert(":key".to_string(), AttributeValue::S(content_s3_key.to_string()));
        self.db
            .update_item(
                &pk,
                &format!("THREAD#{quip_thread_id}"),
                "SET #state = :state, ogre_doc_id = :doc, content_s3_key = :key",
                values,
                Some(HashMap::from([("#state".to_string(), "state".to_string())])),
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Mark a thread `Skipped` (e.g. unsupported thread type, or a 403
    /// after Task 4 decides the thread is inaccessible) with a
    /// human-readable `reason`. Plain `update_item`, same idempotency
    /// rationale as `set_thread_content_done`.
    pub async fn set_thread_skipped(
        &self,
        import_id: &str,
        quip_thread_id: &str,
        reason: &str,
    ) -> Result<(), RepoError> {
        self.set_thread_disposition(import_id, quip_thread_id, ThreadState::Skipped, reason)
            .await
    }

    /// Mark a thread `Failed` (the content pass retried it and gave up)
    /// with a human-readable `reason`. Same shape as `set_thread_skipped`
    /// — the two states differ in *why* the pass stopped trying, not in
    /// how the row is written.
    pub async fn set_thread_failed(
        &self,
        import_id: &str,
        quip_thread_id: &str,
        reason: &str,
    ) -> Result<(), RepoError> {
        self.set_thread_disposition(import_id, quip_thread_id, ThreadState::Failed, reason)
            .await
    }

    /// Shared body for `set_thread_skipped` / `set_thread_failed`: set
    /// `state` and `reason` on an existing `THREAD#` row. Plain
    /// `update_item`, same idempotency rationale as
    /// `set_thread_content_done` — the row already exists from Phase 1's
    /// inventory, and this is a forward-only checkpoint a re-run can
    /// safely repeat.
    async fn set_thread_disposition(
        &self,
        import_id: &str,
        quip_thread_id: &str,
        state: ThreadState,
        reason: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(
            ":state".to_string(),
            AttributeValue::S(thread_state_to_str(state).to_string()),
        );
        values.insert(":reason".to_string(), AttributeValue::S(reason.to_string()));
        self.db
            .update_item(
                &pk,
                &format!("THREAD#{quip_thread_id}"),
                "SET #state = :state, reason = :reason",
                values,
                Some(HashMap::from([("#state".to_string(), "state".to_string())])),
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Atomically increment the per-thread attempt counter and return the
    /// new count. Task 4 reads this to decide when a thread has failed
    /// deterministically enough times (`N`) to be marked `Failed` and
    /// skipped rather than retried forever.
    ///
    /// Uses a raw `ADD attempts :inc` (mirrors `ThreadRepo::add_reaction`'s
    /// pattern for reaching through `DynamoClient::inner()`) rather than a
    /// read-modify-write: `update_item`/`update_item_conditional` on
    /// `DynamoClient` don't expose `ReturnValues`, so this goes straight to
    /// the SDK builder with `ReturnValue::UpdatedNew` to read the
    /// post-increment value back in the same round trip — atomic even
    /// under concurrent callers, unlike a get-then-set.
    pub async fn bump_thread_attempts(
        &self,
        import_id: &str,
        quip_thread_id: &str,
    ) -> Result<u32, RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let sk = format!("THREAD#{quip_thread_id}");
        let mut values = HashMap::new();
        values.insert(":inc".to_string(), AttributeValue::N("1".to_string()));

        let result = self
            .db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(sk))
            .update_expression("ADD attempts :inc")
            .set_expression_attribute_values(Some(values))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::UpdatedNew)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        result
            .attributes
            .as_ref()
            .and_then(|a| a.get("attempts"))
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<u32>().ok())
            .ok_or_else(|| RepoError::MissingField("attempts".to_string()))
    }

    /// Record the total thread count discovered by inventory BFS, on `META`.
    pub async fn set_inventory_total(&self, import_id: &str, total: usize) -> Result<(), RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(
            ":total".to_string(),
            AttributeValue::N(total.to_string()),
        );
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(ogrenotes_common::time::now_usec().to_string()),
        );
        self.db
            .update_item(
                &pk,
                ImportRecord::sk(),
                "SET inventory_total = :total, updated_at = :updated_at",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Advance the import's phase counter on `META`. **Forward-only**: a
    /// write that would move `phase` backwards (or leave it unchanged) is a
    /// no-op, not an error.
    ///
    /// The handler re-runs inventory from scratch on every retry, reaper
    /// redelivery, or manual replay, and writes `phase = 1` when it finishes —
    /// so an unconditional `SET` would regress a phase-2 import to 1 every
    /// time it was replayed. Nothing reads `phase` as a *decreasing* signal, so
    /// clamping it here removes the whole class rather than asking each caller
    /// to check first.
    pub async fn set_phase(&self, import_id: &str, phase: u8) -> Result<(), RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(":phase".to_string(), AttributeValue::N(phase.to_string()));
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(ogrenotes_common::time::now_usec().to_string()),
        );
        // A condition failure means the import is already at or past this
        // phase — the desired end state either way.
        self.db
            .update_item_conditional(
                &pk,
                ImportRecord::sk(),
                "SET phase = :phase, updated_at = :updated_at",
                "attribute_not_exists(phase) OR phase < :phase",
                values,
                None,
            )
            .await
            .map(|_| ())
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Record the user's chosen scope (selected roots + target folder) on
    /// `META`. Consumed by Task 4's scoping endpoint.
    pub async fn set_scope(
        &self,
        import_id: &str,
        roots: &[String],
        target: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(
            ":roots".to_string(),
            AttributeValue::L(roots.iter().cloned().map(AttributeValue::S).collect()),
        );
        values.insert(
            ":target".to_string(),
            AttributeValue::S(target.to_string()),
        );
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(ogrenotes_common::time::now_usec().to_string()),
        );
        self.db
            .update_item(
                &pk,
                ImportRecord::sk(),
                "SET selected_roots = :roots, target_folder_id = :target, updated_at = :updated_at",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Acquire the inventory lease. Succeeds if no claim exists or the
    /// existing claim's heartbeat is older than `stale_ms`. Uses a
    /// conditional update so two workers cannot both acquire.
    pub async fn claim_runner(
        &self,
        import_id: &str,
        instance_id: &str,
        now_ms: i64,
        stale_ms: i64,
    ) -> Result<bool, RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(":inst".to_string(), AttributeValue::S(instance_id.to_string()));
        values.insert(":now".to_string(), AttributeValue::N(now_ms.to_string()));
        values.insert(
            ":stale".to_string(),
            AttributeValue::N((now_ms - stale_ms).to_string()),
        );
        // condition: no claim, OR same instance, OR heartbeat older than cutoff.
        let cond = "attribute_not_exists(runner_instance) OR runner_instance = :inst OR runner_heartbeat_ms < :stale";
        self.db
            .update_item_conditional(
                &pk,
                ImportRecord::sk(),
                "SET runner_instance = :inst, runner_heartbeat_ms = :now",
                cond,
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Best-effort heartbeat refresh for the held lease. If the condition
    /// fails (we've lost the lease to a takeover), that's not this call's
    /// problem to report — the next `claim_runner` attempt will surface it.
    pub async fn heartbeat_runner(
        &self,
        import_id: &str,
        instance_id: &str,
        now_ms: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(":inst".to_string(), AttributeValue::S(instance_id.to_string()));
        values.insert(":now".to_string(), AttributeValue::N(now_ms.to_string()));
        self.db
            .update_item_conditional(
                &pk,
                ImportRecord::sk(),
                "SET runner_heartbeat_ms = :now",
                "runner_instance = :inst",
                values,
                None,
            )
            .await
            .map(|_| ())
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Release the lease (e.g. worker exiting cleanly) so the next claim
    /// doesn't need to wait out `stale_ms`. **Only clears a lease this
    /// instance still holds**; returns `false` when the lease has since been
    /// taken over by someone else.
    ///
    /// The owner check is load-bearing, not defensive. A runner whose lease
    /// went stale mid-pass (one slow thread, a paused container) can be
    /// legitimately superseded by a redelivered run, and would then reach its
    /// own exit path and — unconditionally — wipe the *new* holder's lease,
    /// admitting a third concurrent runner. Only clear what you still own.
    pub async fn clear_runner_claim(
        &self,
        import_id: &str,
        instance_id: &str,
    ) -> Result<bool, RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(":inst".to_string(), AttributeValue::S(instance_id.to_string()));
        self.db
            .update_item_conditional(
                &pk,
                ImportRecord::sk(),
                "REMOVE runner_instance, runner_heartbeat_ms",
                "runner_instance = :inst",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
}

/// The `DynamoClient::put_item_conditional` wrapper hands back the
/// top-level `aws_sdk_dynamodb::Error`, not the per-operation error type —
/// so match the variant rather than reach for the operation-level
/// `is_conditional_check_failed_exception()` predicate (which only exists
/// on `PutItemError` et al., not the aggregated `Error` enum). Mirrors the
/// idiom at `doc_repo.rs`'s `record_open`.
fn is_conditional_check_failure(e: &aws_sdk_dynamodb::Error) -> bool {
    matches!(e, aws_sdk_dynamodb::Error::ConditionalCheckFailedException(_))
}

fn status_to_str(status: ImportStatus) -> &'static str {
    match status {
        ImportStatus::Scoping => "scoping",
        ImportStatus::Running => "running",
        ImportStatus::AwaitingIdentityConfirm => "awaitingidentityconfirm",
        ImportStatus::TokenRejected => "tokenrejected",
        ImportStatus::Succeeded => "succeeded",
        ImportStatus::Failed => "failed",
        ImportStatus::Cancelled => "cancelled",
    }
}

fn status_from_item(item: &HashMap<String, AttributeValue>) -> Result<ImportStatus, RepoError> {
    let raw = get_s(item, "status")?;
    serde_json::from_str(&format!("\"{raw}\""))
        .map_err(|e| RepoError::MissingField(format!("status: {e}")))
}

fn import_to_item(record: &ImportRecord) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert(
        "import_id".to_string(),
        AttributeValue::S(record.import_id.clone()),
    );
    item.insert(
        "owner_id".to_string(),
        AttributeValue::S(record.owner_id.clone()),
    );
    item.insert(
        "status".to_string(),
        AttributeValue::S(status_to_str(record.status).to_string()),
    );
    item.insert("phase".to_string(), AttributeValue::N(record.phase.to_string()));
    if let Some(ref quip_user_id) = record.quip_user_id {
        item.insert(
            "quip_user_id".to_string(),
            AttributeValue::S(quip_user_id.clone()),
        );
    }
    if let Some(ref target_folder_id) = record.target_folder_id {
        item.insert(
            "target_folder_id".to_string(),
            AttributeValue::S(target_folder_id.clone()),
        );
    }
    if !record.selected_roots.is_empty() {
        item.insert(
            "selected_roots".to_string(),
            AttributeValue::L(
                record
                    .selected_roots
                    .iter()
                    .cloned()
                    .map(AttributeValue::S)
                    .collect(),
            ),
        );
    }
    item.insert(
        "created_at".to_string(),
        AttributeValue::N(record.created_at.to_string()),
    );
    item.insert(
        "updated_at".to_string(),
        AttributeValue::N(record.updated_at.to_string()),
    );
    item
}

fn import_from_item(item: &HashMap<String, AttributeValue>) -> Result<ImportRecord, RepoError> {
    Ok(ImportRecord {
        import_id: get_s(item, "import_id")?,
        owner_id: get_s(item, "owner_id")?,
        status: status_from_item(item)?,
        phase: item
            .get("phase")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<u8>().ok())
            .ok_or_else(|| RepoError::MissingField("phase".to_string()))?,
        quip_user_id: item.get("quip_user_id").and_then(|v| v.as_s().ok()).cloned(),
        target_folder_id: item
            .get("target_folder_id")
            .and_then(|v| v.as_s().ok())
            .cloned(),
        selected_roots: item
            .get("selected_roots")
            .and_then(|v| v.as_l().ok())
            .map(|l| l.iter().filter_map(|av| av.as_s().ok().cloned()).collect())
            .unwrap_or_default(),
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
}

fn folder_to_item(f: &FolderRow) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert(
        "quip_folder_id".to_string(),
        AttributeValue::S(f.quip_folder_id.clone()),
    );
    item.insert("owner_id".to_string(), AttributeValue::S(f.owner_id.clone()));
    item.insert("title".to_string(), AttributeValue::S(f.title.clone()));
    if let Some(ref parent_quip_id) = f.parent_quip_id {
        item.insert(
            "parent_quip_id".to_string(),
            AttributeValue::S(parent_quip_id.clone()),
        );
    }
    if let Some(ref ogre_folder_id) = f.ogre_folder_id {
        item.insert(
            "ogre_folder_id".to_string(),
            AttributeValue::S(ogre_folder_id.clone()),
        );
    }
    item
}

fn folder_from_item(item: &HashMap<String, AttributeValue>) -> Result<FolderRow, RepoError> {
    Ok(FolderRow {
        quip_folder_id: get_s(item, "quip_folder_id")?,
        owner_id: get_s(item, "owner_id")?,
        title: get_s(item, "title")?,
        parent_quip_id: item.get("parent_quip_id").and_then(|v| v.as_s().ok()).cloned(),
        ogre_folder_id: item.get("ogre_folder_id").and_then(|v| v.as_s().ok()).cloned(),
    })
}

fn thread_state_to_str(s: ThreadState) -> &'static str {
    match s {
        ThreadState::Pending => "pending",
        ThreadState::ContentDone => "contentdone",
        ThreadState::CommentsDone => "commentsdone",
        ThreadState::Skipped => "skipped",
        ThreadState::Failed => "failed",
    }
}

fn thread_state_from_item(item: &HashMap<String, AttributeValue>) -> Result<ThreadState, RepoError> {
    let raw = get_s(item, "state")?;
    serde_json::from_str(&format!("\"{raw}\""))
        .map_err(|e| RepoError::MissingField(format!("state: {e}")))
}

fn thread_to_item(t: &ThreadRow) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert(
        "quip_thread_id".to_string(),
        AttributeValue::S(t.quip_thread_id.clone()),
    );
    item.insert("owner_id".to_string(), AttributeValue::S(t.owner_id.clone()));
    item.insert("title".to_string(), AttributeValue::S(t.title.clone()));
    item.insert(
        "thread_type".to_string(),
        AttributeValue::S(t.thread_type.clone()),
    );
    item.insert(
        "updated_usec".to_string(),
        AttributeValue::N(t.updated_usec.to_string()),
    );
    item.insert(
        "member_folders".to_string(),
        AttributeValue::L(
            t.member_folders
                .iter()
                .cloned()
                .map(AttributeValue::S)
                .collect(),
        ),
    );
    item.insert(
        "first_folder".to_string(),
        AttributeValue::S(t.first_folder.clone()),
    );
    item.insert(
        "state".to_string(),
        AttributeValue::S(thread_state_to_str(t.state).to_string()),
    );
    if let Some(ref ogre_doc_id) = t.ogre_doc_id {
        item.insert("ogre_doc_id".to_string(), AttributeValue::S(ogre_doc_id.clone()));
    }
    if let Some(ref reason) = t.reason {
        item.insert("reason".to_string(), AttributeValue::S(reason.clone()));
    }
    item.insert("attempts".to_string(), AttributeValue::N(t.attempts.to_string()));
    item
}

fn thread_from_item(item: &HashMap<String, AttributeValue>) -> Result<ThreadRow, RepoError> {
    Ok(ThreadRow {
        quip_thread_id: get_s(item, "quip_thread_id")?,
        owner_id: get_s(item, "owner_id")?,
        title: get_s(item, "title")?,
        thread_type: get_s(item, "thread_type")?,
        updated_usec: get_n(item, "updated_usec")?,
        member_folders: item
            .get("member_folders")
            .and_then(|v| v.as_l().ok())
            .map(|l| l.iter().filter_map(|av| av.as_s().ok().cloned()).collect())
            .unwrap_or_default(),
        first_folder: get_s(item, "first_folder")?,
        state: thread_state_from_item(item)?,
        ogre_doc_id: item.get("ogre_doc_id").and_then(|v| v.as_s().ok()).cloned(),
        reason: item.get("reason").and_then(|v| v.as_s().ok()).cloned(),
        // Rows written before `attempts` existed have no such attribute —
        // decode as 0, the correct "never attempted under the new
        // counter" value, rather than erroring on a mid-flight import.
        attempts: item
            .get("attempts")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0),
    })
}

fn secmap_to_item(r: &SecMapRow) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert(
        "quip_thread_id".to_string(),
        AttributeValue::S(r.quip_thread_id.clone()),
    );
    item.insert("chunk".to_string(), AttributeValue::N(r.chunk.to_string()));
    item.insert("owner_id".to_string(), AttributeValue::S(r.owner_id.clone()));
    item.insert(
        "entries".to_string(),
        AttributeValue::L(
            r.entries
                .iter()
                .map(|(section, block)| {
                    AttributeValue::M(HashMap::from([
                        ("quip_section_id".to_string(), AttributeValue::S(section.clone())),
                        ("ogre_block_id".to_string(), AttributeValue::S(block.clone())),
                    ]))
                })
                .collect(),
        ),
    );
    item
}

fn secmap_from_item(item: &HashMap<String, AttributeValue>) -> Result<SecMapRow, RepoError> {
    let chunk = get_n_u64(item, "chunk")?;
    let chunk = u32::try_from(chunk).map_err(|_| RepoError::MissingField("chunk".to_string()))?;
    let entries = item
        .get("entries")
        .and_then(|v| v.as_l().ok())
        .map(|l| {
            l.iter()
                .filter_map(|av| av.as_m().ok())
                .map(|m| {
                    let section = get_s(m, "quip_section_id")?;
                    let block = get_s(m, "ogre_block_id")?;
                    Ok((section, block))
                })
                .collect::<Result<Vec<_>, RepoError>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(SecMapRow {
        quip_thread_id: get_s(item, "quip_thread_id")?,
        chunk,
        owner_id: get_s(item, "owner_id")?,
        entries,
    })
}

fn unresolved_to_item(r: &UnresolvedRow) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert(
        "source_quip_thread_id".to_string(),
        AttributeValue::S(r.source_quip_thread_id.clone()),
    );
    item.insert("owner_id".to_string(), AttributeValue::S(r.owner_id.clone()));
    item.insert(
        "links".to_string(),
        AttributeValue::L(
            r.links
                .iter()
                .map(|link| {
                    let mut m = HashMap::new();
                    m.insert(
                        "source_block_id".to_string(),
                        AttributeValue::S(link.source_block_id.clone()),
                    );
                    m.insert(
                        "target_quip_thread_id".to_string(),
                        AttributeValue::S(link.target_quip_thread_id.clone()),
                    );
                    if let Some(ref target_quip_section_id) = link.target_quip_section_id {
                        m.insert(
                            "target_quip_section_id".to_string(),
                            AttributeValue::S(target_quip_section_id.clone()),
                        );
                    }
                    AttributeValue::M(m)
                })
                .collect(),
        ),
    );
    item
}

/// Chunk index of one persisted `UNRESOLVED#` row. Rows written before the
/// attribute existed (and the single-chunk case generally) read as 0, which
/// is the correct merge position for a row that was never split.
fn unresolved_chunk_from_item(item: &HashMap<String, AttributeValue>) -> Result<u32, RepoError> {
    if !item.contains_key("chunk") {
        return Ok(0);
    }
    let chunk = get_n_u64(item, "chunk")?;
    u32::try_from(chunk).map_err(|_| RepoError::MissingField("chunk".to_string()))
}

fn unresolved_from_item(item: &HashMap<String, AttributeValue>) -> Result<UnresolvedRow, RepoError> {
    let links = item
        .get("links")
        .and_then(|v| v.as_l().ok())
        .map(|l| {
            l.iter()
                .filter_map(|av| av.as_m().ok())
                .map(|m| {
                    Ok(PendingLinkItem {
                        source_block_id: get_s(m, "source_block_id")?,
                        target_quip_thread_id: get_s(m, "target_quip_thread_id")?,
                        target_quip_section_id: m
                            .get("target_quip_section_id")
                            .and_then(|v| v.as_s().ok())
                            .cloned(),
                    })
                })
                .collect::<Result<Vec<_>, RepoError>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(UnresolvedRow {
        source_quip_thread_id: get_s(item, "source_quip_thread_id")?,
        owner_id: get_s(item, "owner_id")?,
        links,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_fixture() -> ImportRecord {
        ImportRecord {
            import_id: "imp1".to_string(),
            owner_id: "u1".to_string(),
            status: ImportStatus::Scoping,
            phase: 0,
            quip_user_id: Some("quip-u1".to_string()),
            target_folder_id: Some("f1".to_string()),
            selected_roots: vec!["root-a".to_string(), "root-b".to_string()],
            created_at: 100,
            updated_at: 200,
        }
    }

    #[test]
    fn import_round_trips_through_item() {
        let record = record_fixture();
        let back = import_from_item(&import_to_item(&record)).expect("from_item");
        assert_eq!(back, record);
    }

    #[test]
    fn import_optionals_and_roots_are_sparse_when_absent() {
        let mut record = record_fixture();
        record.quip_user_id = None;
        record.target_folder_id = None;
        record.selected_roots = Vec::new();
        let item = import_to_item(&record);
        assert!(!item.contains_key("quip_user_id"));
        assert!(!item.contains_key("target_folder_id"));
        assert!(!item.contains_key("selected_roots"));

        let back = import_from_item(&item).expect("from_item");
        assert_eq!(back.quip_user_id, None);
        assert_eq!(back.target_folder_id, None);
        assert!(back.selected_roots.is_empty());
    }

    #[test]
    fn import_status_round_trips_for_every_variant() {
        for status in [
            ImportStatus::Scoping,
            ImportStatus::Running,
            ImportStatus::AwaitingIdentityConfirm,
            ImportStatus::TokenRejected,
            ImportStatus::Succeeded,
            ImportStatus::Failed,
            ImportStatus::Cancelled,
        ] {
            let mut record = record_fixture();
            record.status = status;
            let back = import_from_item(&import_to_item(&record)).expect("from_item");
            assert_eq!(back.status, status, "round-trip failed for {status:?}");
        }
    }

    #[test]
    fn import_unknown_status_errors() {
        let mut item = import_to_item(&record_fixture());
        item.insert("status".to_string(), AttributeValue::S("bogus".to_string()));
        match import_from_item(&item) {
            Err(RepoError::MissingField(msg)) => {
                assert!(msg.contains("status"), "must name the field: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    #[test]
    fn import_item_has_no_token_field() {
        // Compile-enforced by the model (no `token` field on
        // `ImportRecord`), pinned here at the storage boundary too: the
        // hand-built item this repo writes to DynamoDB must never carry
        // a `token`/`secret` column, since that's the attack surface a
        // leaked table read or backup would expose.
        let item = import_to_item(&record_fixture());
        assert!(!item.contains_key("token"));
        assert!(!item.contains_key("secret"));
    }

    // Pure item (de)serialization round-trips — no live Dynamo needed.
    #[test]
    fn folder_row_item_round_trips() {
        let f = FolderRow { quip_folder_id: "qf1".into(), owner_id: "u1".into(),
            title: "Root".into(), parent_quip_id: Some("qp".into()),
            ogre_folder_id: Some("of1".into()) };
        let back = folder_from_item(&folder_to_item(&f)).expect("from_item");
        assert_eq!(back, f);
    }

    #[test]
    fn folder_row_item_round_trips_and_has_no_token() {
        // Mirrors thread_row_item_round_trips_and_has_no_token: the task's
        // no-token guard names both FolderRow and ThreadRow mappers.
        let f = FolderRow { quip_folder_id: "qf1".into(), owner_id: "u1".into(),
            title: "Root".into(), parent_quip_id: Some("qp".into()),
            ogre_folder_id: Some("of1".into()) };
        let item = folder_to_item(&f);
        assert!(!item.contains_key("token") && !item.contains_key("secret"));
        assert_eq!(folder_from_item(&item).expect("from_item"), f);
    }

    #[test]
    fn thread_row_item_round_trips_and_has_no_token() {
        let t = ThreadRow { quip_thread_id: "qt1".into(), owner_id: "u1".into(),
            title: "Doc".into(), thread_type: "document".into(), updated_usec: 42,
            member_folders: vec!["qf1".into(), "qf2".into()], first_folder: "qf1".into(),
            state: ThreadState::Pending, ogre_doc_id: None, reason: None, attempts: 0 };
        let item = thread_to_item(&t);
        assert!(!item.contains_key("token") && !item.contains_key("secret"));
        assert_eq!(thread_from_item(&item).expect("from_item"), t);
    }

    fn thread_fixture(state: ThreadState, reason: Option<&str>, attempts: u32) -> ThreadRow {
        ThreadRow {
            quip_thread_id: "qt1".into(),
            owner_id: "u1".into(),
            title: "Doc".into(),
            thread_type: "document".into(),
            updated_usec: 42,
            member_folders: vec!["qf1".into()],
            first_folder: "qf1".into(),
            state,
            ogre_doc_id: None,
            reason: reason.map(str::to_string),
            attempts,
        }
    }

    /// `Failed` and `Skipped` round-trip with and without a `reason`, and
    /// carry no token/secret — same guard as the plain-`Pending` case
    /// above, extended to the new variant and field (Task 2 of the
    /// open-failures remediation plan).
    #[test]
    fn thread_row_failed_and_skipped_round_trip_with_and_without_reason() {
        for state in [ThreadState::Failed, ThreadState::Skipped] {
            for reason in [Some("403 forbidden"), None] {
                let t = thread_fixture(state, reason, 2);
                let item = thread_to_item(&t);
                assert!(!item.contains_key("token") && !item.contains_key("secret"));
                assert_eq!(
                    item.contains_key("reason"),
                    reason.is_some(),
                    "reason must be sparse-omitted when None (state={state:?})"
                );
                assert_eq!(thread_from_item(&item).expect("from_item"), t, "state={state:?} reason={reason:?}");
            }
        }
    }

    /// `attempts` round-trips when present, and a row that never went
    /// through the new write path (attribute absent entirely) decodes as
    /// `0` rather than erroring — the load-bearing backward-compat case:
    /// an import mid-flight across the deploy that adds this field.
    #[test]
    fn thread_row_attempts_present_and_backward_compat_absent() {
        let t = thread_fixture(ThreadState::Pending, None, 5);
        let item = thread_to_item(&t);
        assert_eq!(item.get("attempts").and_then(|v| v.as_n().ok()), Some(&"5".to_string()));
        assert_eq!(thread_from_item(&item).expect("from_item"), t);

        // Simulate a pre-Task-2 row: no `attempts`, no `reason` attribute
        // at all (not even sparse-omitted-and-present-as-null — genuinely
        // absent, as a row written by the old `thread_to_item` would be).
        let mut legacy_item = item.clone();
        legacy_item.remove("attempts");
        legacy_item.remove("reason");
        let decoded = thread_from_item(&legacy_item).expect("from_item on legacy row");
        assert_eq!(decoded.attempts, 0);
        assert_eq!(decoded.reason, None);
    }

    /// Backward-compat decode of a hand-built item map that never had
    /// `reason`/`attempts` at all (the exact shape of a `THREAD#` row
    /// written before this change) — must decode cleanly, not error.
    #[test]
    fn thread_row_decodes_pre_task2_item_without_reason_or_attempts() {
        let mut item = HashMap::new();
        item.insert("quip_thread_id".to_string(), AttributeValue::S("qt1".into()));
        item.insert("owner_id".to_string(), AttributeValue::S("u1".into()));
        item.insert("title".to_string(), AttributeValue::S("Doc".into()));
        item.insert("thread_type".to_string(), AttributeValue::S("document".into()));
        item.insert("updated_usec".to_string(), AttributeValue::N("42".into()));
        item.insert(
            "member_folders".to_string(),
            AttributeValue::L(vec![AttributeValue::S("qf1".into())]),
        );
        item.insert("first_folder".to_string(), AttributeValue::S("qf1".into()));
        item.insert("state".to_string(), AttributeValue::S("contentdone".into()));
        item.insert("ogre_doc_id".to_string(), AttributeValue::S("doc-1".into()));
        // Deliberately no "reason", no "attempts" — this is what every
        // THREAD# row written before Task 2 looks like.

        let decoded = thread_from_item(&item).expect("must decode a pre-Task-2 row without error");
        assert_eq!(decoded.state, ThreadState::ContentDone);
        assert_eq!(decoded.ogre_doc_id.as_deref(), Some("doc-1"));
        assert_eq!(decoded.reason, None);
        assert_eq!(decoded.attempts, 0);
    }

    #[test]
    fn secmap_row_round_trips_and_has_no_token() {
        let r = SecMapRow {
            quip_thread_id: "t1".into(),
            chunk: 0,
            owner_id: "u1".into(),
            entries: vec![("s1".into(), "b1".into()), ("s2".into(), "b2".into())],
        };
        let item = secmap_to_item(&r);
        assert!(!item.contains_key("token") && !item.contains_key("secret"));
        assert_eq!(secmap_from_item(&item).expect("from_item"), r);
    }

    #[test]
    fn unresolved_row_round_trips_with_optional_section() {
        let r = UnresolvedRow {
            source_quip_thread_id: "t1".into(),
            owner_id: "u1".into(),
            links: vec![
                PendingLinkItem {
                    source_block_id: "b1".into(),
                    target_quip_thread_id: "t2".into(),
                    target_quip_section_id: Some("s9".into()),
                },
                PendingLinkItem {
                    source_block_id: "b2".into(),
                    target_quip_thread_id: "t3".into(),
                    target_quip_section_id: None,
                },
            ],
        };
        assert_eq!(unresolved_from_item(&unresolved_to_item(&r)).expect("from_item"), r);
    }

    #[test]
    fn unresolved_row_has_no_token() {
        let r = UnresolvedRow {
            source_quip_thread_id: "t1".into(),
            owner_id: "u1".into(),
            links: vec![PendingLinkItem {
                source_block_id: "b1".into(),
                target_quip_thread_id: "t2".into(),
                target_quip_section_id: None,
            }],
        };
        let item = unresolved_to_item(&r);
        assert!(!item.contains_key("token") && !item.contains_key("secret"));
    }

    #[test]
    fn secmap_row_sk_formats() {
        let r = SecMapRow { quip_thread_id: "t1".into(), chunk: 3, owner_id: "u1".into(), entries: vec![] };
        assert_eq!(r.sk(), "SECMAP#t1#3");
    }

    /// `UNRESOLVED#` is chunked (I4) exactly like `SECMAP#`, so its SK
    /// carries the chunk index. Pinned because Phase 2b inherits this shape.
    #[test]
    fn unresolved_row_sk_formats_with_chunk() {
        let r = UnresolvedRow { source_quip_thread_id: "t1".into(), owner_id: "u1".into(), links: vec![] };
        assert_eq!(r.sk(0), "UNRESOLVED#t1#0");
        assert_eq!(r.sk(12), "UNRESOLVED#t1#12");
    }

    /// A row written before the `chunk` attribute existed (and any
    /// single-chunk row) must merge at position 0, not error out.
    #[test]
    fn unresolved_chunk_defaults_to_zero_when_absent() {
        let r = UnresolvedRow { source_quip_thread_id: "t1".into(), owner_id: "u1".into(), links: vec![] };
        let item = unresolved_to_item(&r);
        assert!(!item.contains_key("chunk"));
        assert_eq!(unresolved_chunk_from_item(&item).expect("chunk"), 0);

        let mut item = unresolved_to_item(&r);
        item.insert("chunk".to_string(), AttributeValue::N("7".to_string()));
        assert_eq!(unresolved_chunk_from_item(&item).expect("chunk"), 7);
    }
}
