// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::import::{ImportRecord, ImportStatus};
use crate::models::import_inventory::{
    FolderRow, PendingLinkItem, ReportNote, ReportRow, SecMapRow, ThreadRow, ThreadState,
    UnresolvedRow, UNRESOLVED_CHUNK_LINKS,
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

    /// Write a folder row discovered during inventory BFS.
    ///
    /// Upserts the fields the *inventory walk* owns — title and parentage —
    /// and deliberately does **not** touch [`OGRE_FOLDER_ID_ATTR`], which is
    /// [`ImportRepo::record_ogre_folder`]'s alone. That split is the whole
    /// reason this is an `update_item` and not the `put_item` it used to be
    /// (#236).
    ///
    /// The failure it prevents: the inventory BFS re-runs on every job
    /// attempt and always offers `ogre_folder_id: None`, because the walk has
    /// no OgreNotes folder to offer. A whole-item put therefore *erased* the
    /// mirrored-tree idempotency key on the second run, so the mirroring pass
    /// read "not created yet" and built a second tree — every run, forever.
    /// Folders were once described here as carrying "no progress state that a
    /// re-run could downgrade"; `ogre_folder_id` is exactly such state, and it
    /// is now the only field on the row that a re-run must not touch.
    pub async fn put_folder(&self, import_id: &str, f: &FolderRow) -> Result<(), RepoError> {
        let (expression, names, values) = folder_inventory_update(f);
        self.db
            .update_item(
                &format!("IMPORT#{import_id}"),
                &f.sk(),
                &expression,
                values,
                Some(names),
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Record the OgreNotes folder mirroring one Quip folder, and return the
    /// id that is durably recorded — which is **not** necessarily `candidate`.
    ///
    /// This is [`ImportRepo::record_import_folder`]'s contract, per folder,
    /// and for the same reason: it is the idempotency key of a create that a
    /// crashed, reaped, or re-started import will attempt again. Present →
    /// the caller adopts what is already there; absent → the caller's
    /// freshly-created folder is recorded under a conditional write, and a
    /// concurrent loser reads the winner's id back and leaves its own folder
    /// as a harmless empty orphan (owned by the user, linked under its
    /// parent, so listable and deletable — never a wedge).
    ///
    /// `attribute_exists(quip_folder_id)` is load-bearing and not belt-and-
    /// braces: DynamoDB satisfies `attribute_not_exists(ogre_folder_id)`
    /// against a *missing item*, so without it a mis-keyed call would create
    /// a row carrying only the keys and this id — which `folder_from_item`
    /// cannot decode, permanently poisoning `list_folders` for that import.
    pub async fn record_ogre_folder(
        &self,
        import_id: &str,
        quip_folder_id: &str,
        candidate: &str,
    ) -> Result<String, RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let sk = format!("FOLDER#{quip_folder_id}");
        let mut names = HashMap::new();
        names.insert("#ogre".to_string(), OGRE_FOLDER_ID_ATTR.to_string());
        names.insert("#quip".to_string(), QUIP_FOLDER_ID_ATTR.to_string());
        let mut values = HashMap::new();
        values.insert(":fid".to_string(), AttributeValue::S(candidate.to_string()));
        let recorded = self
            .db
            .update_item_conditional(
                &pk,
                &sk,
                "SET #ogre = :fid",
                "attribute_exists(#quip) AND attribute_not_exists(#ogre)",
                values,
                Some(names),
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        if recorded {
            return Ok(candidate.to_string());
        }
        // Lost the race (or this is a re-run): read back whatever is durable.
        // A row that is missing entirely lands here too, and must surface as
        // an error rather than as a silent "no mapping" — see the condition
        // above.
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?
            .ok_or_else(|| {
                RepoError::MissingField(format!("import {import_id} has no {sk} row"))
            })?;
        folder_from_item(&item)?
            .ogre_folder_id
            .ok_or_else(|| RepoError::MissingField(format!("{sk} ogre_folder_id")))
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
    ///
    /// Also the lookup for a Quip **comment anchor** since #194 F-10 — see
    /// [`SecMapRow`] for why the two share one map. A Phase-4 caller holding
    /// an `annotationid` from Quip's comment API finds its block here and
    /// needs nothing else from this repo.
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

    /// Read this import's accumulating `REPORT` row.
    ///
    /// `Ok(None)` when the import has nothing to report yet — the row is
    /// written lazily on the first counter or note, so its absence is the
    /// normal state of a clean import, not an error.
    ///
    /// **Strongly consistent**, unlike every other read in this repo.
    /// `DynamoClient::get_item` never sets `ConsistentRead`, and the
    /// default eventually-consistent read can serve a replica that has not
    /// yet seen the last write — which would make
    /// [`mutate_report`](Self::mutate_report)'s read-modify-write lose
    /// increments *with no concurrency at all*: two `bump_report_counter`
    /// calls milliseconds apart from the same single runner are enough. So
    /// this reaches through `DynamoClient::inner()` for a raw builder, the
    /// same escape hatch [`bump_thread_attempts`](Self::bump_thread_attempts)
    /// uses.
    ///
    /// The consistency is on *this* method rather than only on the RMW
    /// path deliberately: a second, quietly eventually-consistent reader
    /// would be a loaded gun for the next person to build a mutation on
    /// top of. One read path, correct by construction. The cost is one
    /// extra RCU per read of a single small item — read once per counter
    /// bump or note and once per wizard poll, which is nothing against the
    /// per-thread Quip API calls happening alongside it.
    pub async fn get_report(&self, import_id: &str) -> Result<Option<ReportRow>, RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let result = self
            .db
            .inner()
            .get_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(ReportRow::sk().to_string()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;
        result.item.as_ref().map(report_from_item).transpose()
    }

    /// Add `by` to the named counter on the `REPORT` row, creating the row
    /// (and the counter) if needed. Counters are the totals the report
    /// quotes; unlike `notes` they are unbounded in value and bounded only
    /// by the number of distinct keys the callers use, which is a small
    /// compile-time set.
    ///
    /// **Callers must treat a failure here as advisory.** The report
    /// describes an import; it must never be able to *stop* one. A
    /// corrupt or unwritable report row that propagated into the content
    /// pass's control flow would halt a migration over its own bookkeeping
    /// — the failure mode this whole row exists to remove. Log and carry
    /// on. (Same for [`append_report_note`](Self::append_report_note).)
    pub async fn bump_report_counter(
        &self,
        import_id: &str,
        owner_id: &str,
        key: &str,
        by: u64,
    ) -> Result<(), RepoError> {
        self.mutate_report(import_id, owner_id, |row| row.bump_counter(key, by))
            .await
    }

    /// Append one named loss/fallback to the `REPORT` row, creating the row
    /// if needed.
    ///
    /// Silently budgeted, per `kind`, at
    /// [`REPORT_MAX_NOTES_PER_KIND`](crate::models::import_inventory::REPORT_MAX_NOTES_PER_KIND)
    /// and globally at
    /// [`REPORT_MAX_NOTES`](crate::models::import_inventory::REPORT_MAX_NOTES):
    /// over budget the note is dropped and only `notes_dropped` advances,
    /// so a pathological import (every thread inaccessible) can't grow this
    /// item past DynamoDB's 400 KB cap and lose the whole report, and a
    /// high-frequency kind can't spend the slots a rarer one needs.
    /// Callers that need the true total must also
    /// [`bump_report_counter`](Self::bump_report_counter) — the note list is
    /// a sample, the counters are the tally. Failures here are advisory,
    /// exactly as for `bump_report_counter`.
    pub async fn append_report_note(
        &self,
        import_id: &str,
        owner_id: &str,
        note: ReportNote,
    ) -> Result<(), RepoError> {
        self.mutate_report(import_id, owner_id, |row| row.push_note(note))
            .await
    }

    /// Shared read-modify-write body for the `REPORT` mutators.
    ///
    /// **This is a plain read-modify-write, and it is only safe because of
    /// the `runner_claim` lease.** The content pass is single-writer per
    /// import — `claim_runner`/`heartbeat_runner` admit one runner at a time
    /// — so no second writer can interleave between the read and the write.
    /// A future caller that writes the report from *outside* that lease
    /// (an API handler, a second worker pass, a parallel per-thread task)
    /// turns this into a lost-update bug: the loser's counters and notes
    /// vanish silently. If that day comes, the fix is to make each mutation
    /// atomic on the server — `ADD` for the counters (mirroring
    /// [`bump_thread_attempts`](Self::bump_thread_attempts)) and a
    /// `list_append` guarded by `size(notes) < :cap` for the notes — not to
    /// add a lock here.
    ///
    /// Two caveats worth knowing even today. First, lease takeover after a
    /// stale heartbeat can briefly overlap two runners (see
    /// `clear_runner_claim`'s owner check — the superseded runner is still
    /// executing and nothing revokes its ability to write; there is no
    /// fencing token). What a lost update costs in that window is **not**
    /// one line: this writes the whole item, so the loser's *entire*
    /// mutation is dropped — every note **and every counter increment**
    /// the other writer committed between our read and our put. The
    /// counters are the worse half. They are what "…and 9 800 more" is
    /// computed from, so a lost increment makes the report silently
    /// **under-report** the losses — the exact failure this row exists to
    /// prevent, arriving quietly and looking like good news. It is still
    /// never document data: the report is derived from the `THREAD#` rows,
    /// which are written by their own conditional/forward-only paths.
    ///
    /// Second, and for the same reason, a mutation must never be built
    /// from a stale in-memory `ReportRow` — always go through these
    /// methods, which re-read (consistently) first.
    async fn mutate_report(
        &self,
        import_id: &str,
        owner_id: &str,
        mutate: impl FnOnce(&mut ReportRow),
    ) -> Result<(), RepoError> {
        let mut row = self
            .get_report(import_id)
            .await?
            .unwrap_or_else(|| ReportRow::new(owner_id));
        mutate(&mut row);

        let mut item = report_to_item(&row);
        item.insert("PK".to_string(), AttributeValue::S(format!("IMPORT#{import_id}")));
        item.insert("SK".to_string(), AttributeValue::S(ReportRow::sk().to_string()));
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
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

    /// Record the dedicated per-import destination folder id on `META`, if one
    /// is not already recorded, and return whichever id is now durably recorded.
    ///
    /// This is the idempotency point for the "one folder per import"
    /// containment (issue #170). The write is conditional on
    /// `attribute_not_exists(import_folder_id)`, so the FIRST `start` to reach
    /// here wins and its `candidate` becomes the import's folder; every later
    /// `start` — a double-click, or a job the queue redelivered — fails the
    /// condition and reads the winner's id back instead of recording a second
    /// folder. Mirrors [`reserve_thread_doc_id`](Self::reserve_thread_doc_id)'s
    /// reserve-or-read-back shape.
    ///
    /// Returns `candidate` when THIS call won the reservation (the caller then
    /// knows its freshly-created folder is the durable one), or the
    /// pre-existing id when it lost (the caller's own candidate, if it created
    /// one, is a harmless empty orphan and the winner's folder is used).
    pub async fn record_import_folder(
        &self,
        import_id: &str,
        candidate: &str,
    ) -> Result<String, RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(":fid".to_string(), AttributeValue::S(candidate.to_string()));
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(ogrenotes_common::time::now_usec().to_string()),
        );
        let recorded = self
            .db
            .update_item_conditional(
                &pk,
                ImportRecord::sk(),
                "SET import_folder_id = :fid, updated_at = :updated_at",
                "attribute_not_exists(import_folder_id)",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        if recorded {
            return Ok(candidate.to_string());
        }
        // Lost the race (or this is a re-start): a folder is already recorded.
        // Read the winner's id back so the caller files documents into the one
        // durable import folder rather than its own.
        let record = self
            .get(import_id)
            .await?
            .ok_or_else(|| RepoError::MissingField(format!("import {import_id} META")))?;
        record
            .import_folder_id
            .ok_or_else(|| RepoError::MissingField("import_folder_id".to_string()))
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
    if let Some(ref import_folder_id) = record.import_folder_id {
        item.insert(
            "import_folder_id".to_string(),
            AttributeValue::S(import_folder_id.clone()),
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
        // Absent on imports written before the per-import folder existed →
        // `None`, which the `start` handler reads as "not yet created".
        import_folder_id: item
            .get("import_folder_id")
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

/// Attribute names on a `FOLDER#` row, named once so the writer, the reader
/// and the two update expressions cannot drift apart. A drifted name here is
/// silent: an unknown attribute reads as absent, i.e. "this folder was never
/// mirrored", which is exactly the state that makes the mirroring pass build
/// a duplicate tree.
const QUIP_FOLDER_ID_ATTR: &str = "quip_folder_id";
const PARENT_QUIP_ID_ATTR: &str = "parent_quip_id";
/// The mirrored-tree idempotency key. Written by
/// [`ImportRepo::record_ogre_folder`] and by nothing else — in particular
/// **not** by [`ImportRepo::put_folder`], which the inventory re-runs.
const OGRE_FOLDER_ID_ATTR: &str = "ogre_folder_id";

fn folder_to_item(f: &FolderRow) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert(
        QUIP_FOLDER_ID_ATTR.to_string(),
        AttributeValue::S(f.quip_folder_id.clone()),
    );
    item.insert("owner_id".to_string(), AttributeValue::S(f.owner_id.clone()));
    item.insert("title".to_string(), AttributeValue::S(f.title.clone()));
    if let Some(ref parent_quip_id) = f.parent_quip_id {
        item.insert(
            PARENT_QUIP_ID_ATTR.to_string(),
            AttributeValue::S(parent_quip_id.clone()),
        );
    }
    if let Some(ref ogre_folder_id) = f.ogre_folder_id {
        item.insert(
            OGRE_FOLDER_ID_ATTR.to_string(),
            AttributeValue::S(ogre_folder_id.clone()),
        );
    }
    item
}

/// The `(expression, names, values)` [`ImportRepo::put_folder`] writes:
/// every attribute [`folder_to_item`] produces **except**
/// [`OGRE_FOLDER_ID_ATTR`], plus a `REMOVE` for a parent that has gone away
/// so a re-rooted folder cannot keep a stale pointer.
///
/// Derived from `folder_to_item` rather than re-listing the attributes, so
/// the writer and the reader stay one mapping. Split out from the async
/// method purely so the exclusion can be asserted by a unit test — it is the
/// property the whole idempotency design rests on.
fn folder_inventory_update(
    f: &FolderRow,
) -> (String, HashMap<String, String>, HashMap<String, AttributeValue>) {
    let mut names = HashMap::new();
    let mut values = HashMap::new();
    let mut sets: Vec<String> = Vec::new();
    // Sorted first, so an arbitrary `HashMap` iteration order cannot make two
    // runs writing the same row issue textually different updates.
    let mut attrs: Vec<(String, AttributeValue)> = folder_to_item(f)
        .into_iter()
        .filter(|(attr, _)| attr != OGRE_FOLDER_ID_ATTR)
        .collect();
    attrs.sort_by(|a, b| a.0.cmp(&b.0));
    for (n, (attr, value)) in attrs.into_iter().enumerate() {
        // Every name is aliased: DynamoDB's reserved-word list is long and
        // grows, and a row this code cannot write is a stuck import.
        let (name, value_key) = (format!("#a{n}"), format!(":v{n}"));
        sets.push(format!("{name} = {value_key}"));
        names.insert(name, attr);
        values.insert(value_key, value);
    }
    let mut expression = format!("SET {}", sets.join(", "));
    if f.parent_quip_id.is_none() {
        names.insert("#parent".to_string(), PARENT_QUIP_ID_ATTR.to_string());
        expression.push_str(" REMOVE #parent");
    }
    (expression, names, values)
}

fn folder_from_item(item: &HashMap<String, AttributeValue>) -> Result<FolderRow, RepoError> {
    Ok(FolderRow {
        quip_folder_id: get_s(item, QUIP_FOLDER_ID_ATTR)?,
        owner_id: get_s(item, "owner_id")?,
        title: get_s(item, "title")?,
        parent_quip_id: item.get(PARENT_QUIP_ID_ATTR).and_then(|v| v.as_s().ok()).cloned(),
        ogre_folder_id: item.get(OGRE_FOLDER_ID_ATTR).and_then(|v| v.as_s().ok()).cloned(),
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

fn report_to_item(r: &ReportRow) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert("owner_id".to_string(), AttributeValue::S(r.owner_id.clone()));
    // Counters/notes are sparse-omitted when empty, same convention as
    // `selected_roots` on `META` — an import with nothing to report writes
    // the smallest possible row.
    if !r.counters.is_empty() {
        item.insert(
            "counters".to_string(),
            AttributeValue::M(
                r.counters
                    .iter()
                    .map(|(k, v)| (k.clone(), AttributeValue::N(v.to_string())))
                    .collect(),
            ),
        );
    }
    if !r.notes.is_empty() {
        item.insert(
            "notes".to_string(),
            AttributeValue::L(
                r.notes
                    .iter()
                    .map(|n| {
                        AttributeValue::M(HashMap::from([
                            (
                                "quip_thread_id".to_string(),
                                AttributeValue::S(n.quip_thread_id.clone()),
                            ),
                            ("kind".to_string(), AttributeValue::S(n.kind.clone())),
                            ("detail".to_string(), AttributeValue::S(n.detail.clone())),
                        ]))
                    })
                    .collect(),
            ),
        );
    }
    if r.notes_dropped > 0 {
        item.insert(
            "notes_dropped".to_string(),
            AttributeValue::N(r.notes_dropped.to_string()),
        );
    }
    item
}

fn report_from_item(item: &HashMap<String, AttributeValue>) -> Result<ReportRow, RepoError> {
    let counters = item
        .get("counters")
        .and_then(|v| v.as_m().ok())
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let n = v
                        .as_n()
                        .ok()
                        .and_then(|n| n.parse::<u64>().ok())
                        .ok_or_else(|| RepoError::MissingField(format!("counters.{k}")))?;
                    Ok((k.clone(), n))
                })
                .collect::<Result<_, RepoError>>()
        })
        .transpose()?
        .unwrap_or_default();
    let notes = item
        .get("notes")
        .and_then(|v| v.as_l().ok())
        .map(|l| {
            l.iter()
                .filter_map(|av| av.as_m().ok())
                .map(|m| {
                    Ok(ReportNote {
                        quip_thread_id: get_s(m, "quip_thread_id")?,
                        kind: get_s(m, "kind")?,
                        detail: get_s(m, "detail")?,
                    })
                })
                .collect::<Result<Vec<_>, RepoError>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(ReportRow {
        owner_id: get_s(item, "owner_id")?,
        counters,
        notes,
        // Absent means nothing was ever dropped — the list is complete.
        notes_dropped: item
            .get("notes_dropped")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime::client::http::test_util::StaticReplayClient;
    use std::collections::BTreeSet;

    fn record_fixture() -> ImportRecord {
        ImportRecord {
            import_id: "imp1".to_string(),
            owner_id: "u1".to_string(),
            status: ImportStatus::Scoping,
            phase: 0,
            quip_user_id: Some("quip-u1".to_string()),
            target_folder_id: Some("f1".to_string()),
            import_folder_id: Some("imp-folder-1".to_string()),
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
        record.import_folder_id = None;
        record.selected_roots = Vec::new();
        let item = import_to_item(&record);
        assert!(!item.contains_key("quip_user_id"));
        assert!(!item.contains_key("target_folder_id"));
        assert!(!item.contains_key("import_folder_id"));
        assert!(!item.contains_key("selected_roots"));

        let back = import_from_item(&item).expect("from_item");
        assert_eq!(back.quip_user_id, None);
        assert_eq!(back.target_folder_id, None);
        assert_eq!(back.import_folder_id, None);
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

    /// The exclusion the mirrored-tree idempotency key rests on (#236): the
    /// update `put_folder` issues must never mention `ogre_folder_id`, even
    /// when handed a row that carries one. The inventory re-runs on every job
    /// attempt with `ogre_folder_id: None`, and a write that reached the
    /// attribute at all would clear it and make the mirroring pass build a
    /// second tree.
    #[test]
    fn put_folders_update_never_reaches_the_mirrored_folder_id() {
        let f = FolderRow {
            quip_folder_id: "qf1".into(),
            owner_id: "u1".into(),
            title: "Root".into(),
            parent_quip_id: Some("qp".into()),
            ogre_folder_id: Some("of1".into()),
        };
        let (expression, names, values) = folder_inventory_update(&f);
        assert!(
            !names.values().any(|attr| attr == OGRE_FOLDER_ID_ATTR),
            "no alias may point at the idempotency key: {names:?}",
        );
        assert!(
            !expression.contains(OGRE_FOLDER_ID_ATTR),
            "nor may the expression name it directly: {expression}",
        );
        assert!(
            !values.values().any(|v| v.as_s().is_ok_and(|s| s == "of1")),
            "nor may its value ride along: {values:?}",
        );
        // ...while the inventory-owned fields all do get written.
        let written: BTreeSet<&str> = names.values().map(String::as_str).collect();
        assert!(written.contains(QUIP_FOLDER_ID_ATTR), "{written:?}");
        assert!(written.contains(PARENT_QUIP_ID_ATTR), "{written:?}");
        assert!(written.contains("title"), "{written:?}");
        assert!(written.contains("owner_id"), "{written:?}");
    }

    /// A folder that is no longer anyone's child must lose the attribute,
    /// not keep pointing at the parent a previous run recorded.
    #[test]
    fn a_row_without_a_parent_removes_the_parent_attribute() {
        let f = FolderRow {
            quip_folder_id: "qf1".into(),
            owner_id: "u1".into(),
            title: "Root".into(),
            parent_quip_id: None,
            ogre_folder_id: None,
        };
        let (expression, names, _) = folder_inventory_update(&f);
        assert!(expression.contains("REMOVE"), "{expression}");
        assert!(
            names.values().any(|attr| attr == PARENT_QUIP_ID_ATTR),
            "{names:?}",
        );
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

    fn report_fixture() -> ReportRow {
        let mut r = ReportRow::new("u1");
        r.bump_counter("threads_imported", 7);
        r.bump_counter("threads_skipped", 2);
        r.push_note(ReportNote {
            quip_thread_id: "qt1".into(),
            kind: "skipped".into(),
            detail: "403 forbidden".into(),
        });
        r.push_note(ReportNote {
            quip_thread_id: "qt2".into(),
            kind: "image_dropped".into(),
            detail: "blob too large".into(),
        });
        r
    }

    #[test]
    fn report_row_round_trips_and_has_no_token() {
        let r = report_fixture();
        let item = report_to_item(&r);
        // Same guard as every other manifest row: no durable row in the
        // import partition may carry the Quip credential.
        assert!(!item.contains_key("token") && !item.contains_key("secret"));
        assert_eq!(report_from_item(&item).expect("from_item"), r);
    }

    /// A report with nothing in it writes the smallest possible row (no
    /// `counters`/`notes`/`notes_dropped` attributes) and reads back as an
    /// empty report rather than erroring — the shape every import has
    /// before its first loss.
    #[test]
    fn report_row_empty_is_sparse_and_decodes_clean() {
        let r = ReportRow::new("u1");
        let item = report_to_item(&r);
        assert!(!item.contains_key("counters"));
        assert!(!item.contains_key("notes"));
        assert!(
            !item.contains_key("notes_dropped"),
            "an untruncated report must not claim a truncation"
        );
        let back = report_from_item(&item).expect("from_item");
        assert_eq!(back, r);
        assert!(back.counters.is_empty() && back.notes.is_empty());
        assert_eq!(back.notes_dropped, 0);
    }

    /// The truncation marker must survive the wire, not just live in
    /// memory: a reader that only ever sees the persisted row is the one
    /// that has to say "and N more".
    #[test]
    fn report_row_truncation_marker_survives_the_item_round_trip() {
        use crate::models::import_inventory::REPORT_MAX_NOTES_PER_KIND;

        let mut r = ReportRow::new("u1");
        for i in 0..REPORT_MAX_NOTES_PER_KIND + 3 {
            r.push_note(ReportNote {
                quip_thread_id: format!("qt{i}"),
                kind: "failed".into(),
                detail: "boom".into(),
            });
            r.bump_counter("threads_failed", 1);
        }
        let back = report_from_item(&report_to_item(&r)).expect("from_item");
        assert_eq!(back.notes.len(), REPORT_MAX_NOTES_PER_KIND);
        assert_eq!(back.notes_dropped, 3);
        assert_eq!(
            back.counters["threads_failed"],
            (REPORT_MAX_NOTES_PER_KIND + 3) as u64
        );
    }

    /// The per-kind budget has to survive the wire too: a reader that only
    /// ever sees the persisted row must still find the rare kind's notes
    /// among the noisy kind's.
    #[test]
    fn report_row_keeps_every_kind_across_the_item_round_trip() {
        use crate::models::import_inventory::REPORT_MAX_NOTES_PER_KIND;

        let mut r = ReportRow::new("u1");
        for i in 0..REPORT_MAX_NOTES_PER_KIND * 4 {
            r.push_note(ReportNote {
                quip_thread_id: format!("qt{i}"),
                kind: "image_dropped".into(),
                detail: "blob 403".into(),
            });
        }
        r.push_note(ReportNote {
            quip_thread_id: "qt-late".into(),
            kind: "skipped".into(),
            detail: "403 forbidden".into(),
        });

        let back = report_from_item(&report_to_item(&r)).expect("from_item");
        assert_eq!(
            back.notes.iter().filter(|n| n.kind == "image_dropped").count(),
            REPORT_MAX_NOTES_PER_KIND
        );
        assert_eq!(
            back.notes.iter().filter(|n| n.kind == "skipped").count(),
            1,
            "the late, rare kind must still be on the persisted row"
        );
    }

    /// A corrupt counter value must surface as a named `MissingField`
    /// rather than silently decoding as zero — a report that under-counts
    /// is worse than one that fails loudly.
    #[test]
    fn report_row_rejects_a_non_numeric_counter() {
        let mut item = report_to_item(&report_fixture());
        item.insert(
            "counters".to_string(),
            AttributeValue::M(HashMap::from([(
                "threads_imported".to_string(),
                AttributeValue::S("lots".to_string()),
            )])),
        );
        match report_from_item(&item) {
            Err(RepoError::MissingField(msg)) => {
                assert!(msg.contains("threads_imported"), "must name the counter: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    /// A `DynamoClient` whose HTTP layer replays canned responses and
    /// records the requests the SDK actually emitted. Lets a test assert on
    /// request *shape* — the only way to pin `ConsistentRead`, since
    /// DynamoDB Local serves every read from one store and so behaves
    /// identically with and without it.
    fn replaying_repo(responses: Vec<&str>) -> (ImportRepo, StaticReplayClient) {
        use aws_smithy_runtime::client::http::test_util::ReplayEvent;
        use aws_smithy_types::body::SdkBody;

        let replay = StaticReplayClient::new(
            responses
                .into_iter()
                .map(|body| {
                    ReplayEvent::new(
                        http::Request::builder()
                            .uri("http://localhost/")
                            .body(SdkBody::from("{}"))
                            .unwrap(),
                        http::Response::builder()
                            .status(200)
                            .body(SdkBody::from(body))
                            .unwrap(),
                    )
                })
                .collect(),
        );
        let conf = aws_sdk_dynamodb::config::Builder::new()
            .endpoint_url("http://localhost:9999")
            .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
            .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
                "k", "s", None, None, "test",
            ))
            .http_client(replay.clone())
            .behavior_version_latest()
            .build();
        let repo = ImportRepo::new(crate::dynamo::DynamoClient::new(
            aws_sdk_dynamodb::Client::from_conf(conf),
            "test-table".to_string(),
        ));
        (repo, replay)
    }

    /// The report's read-modify-write is only correct if its read is
    /// **strongly consistent**. `DynamoClient::get_item` never sets
    /// `ConsistentRead`, and under the default eventually-consistent read
    /// two `bump_report_counter` calls milliseconds apart *from the same
    /// single runner* can lose an increment: the second read may be served
    /// by a replica that hasn't seen the first write. That is a lost update
    /// with **no concurrency at all** — not the lease-overlap case — and a
    /// lost counter increment makes the report silently under-report the
    /// losses it exists to name.
    ///
    /// DynamoDB Local cannot express the difference (it serves every read
    /// from one store), so the live integration tests pass either way. This
    /// asserts the request the SDK actually puts on the wire instead.
    #[tokio::test]
    async fn the_report_read_modify_write_reads_consistently() {
        let (repo, replay) = replaying_repo(vec![
            // The existing row the RMW reads back...
            r#"{"Item":{"owner_id":{"S":"u1"},"counters":{"M":{"threads_failed":{"N":"4"}}}}}"#,
            // ...and the PutItem response.
            "{}",
        ]);

        repo.bump_report_counter("imp1", "u1", "threads_failed", 1)
            .await
            .expect("bump_report_counter");

        // Everything is read off the captured requests inline: they are
        // smithy's own `Request` type, which this crate has no direct
        // dependency on to name in a helper's signature.
        let reqs: Vec<_> = replay.actual_requests().collect();
        assert_eq!(reqs.len(), 2, "an RMW is exactly one read then one write");
        let read_target = reqs[0].headers().get("x-amz-target").unwrap_or_default();
        let read_body =
            String::from_utf8(reqs[0].body().bytes().expect("in-memory body").to_vec())
                .expect("utf-8 body");
        let write_target = reqs[1].headers().get("x-amz-target").unwrap_or_default();
        let write_body =
            String::from_utf8(reqs[1].body().bytes().expect("in-memory body").to_vec())
                .expect("utf-8 body");

        assert!(
            read_target.ends_with("GetItem"),
            "first call must be the read: {read_target}"
        );
        assert!(
            read_body.contains(r#""ConsistentRead":true"#),
            "the RMW's read must be strongly consistent, else an increment can \
             be lost with no concurrency at all: {read_body}",
        );

        // ...and the write really did merge what the read returned, rather
        // than starting from an empty row.
        assert!(
            write_target.ends_with("PutItem"),
            "second call must be the write: {write_target}"
        );
        assert!(
            write_body.contains(r#""threads_failed":{"N":"5"}"#),
            "the write must carry 4 + 1: {write_body}",
        );
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
