// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::import::{ImportRecord, ImportStatus};
use crate::models::import_inventory::{FolderRow, ThreadRow, ThreadState};
use crate::repo::{RepoError, get_n, get_s};

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

    /// Advance the import's phase counter on `META`.
    pub async fn set_phase(&self, import_id: &str, phase: u8) -> Result<(), RepoError> {
        let pk = format!("IMPORT#{import_id}");
        let mut values = HashMap::new();
        values.insert(":phase".to_string(), AttributeValue::N(phase.to_string()));
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(ogrenotes_common::time::now_usec().to_string()),
        );
        self.db
            .update_item(
                &pk,
                ImportRecord::sk(),
                "SET phase = :phase, updated_at = :updated_at",
                values,
                None,
            )
            .await
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
    /// doesn't need to wait out `stale_ms`.
    pub async fn clear_runner_claim(&self, import_id: &str) -> Result<(), RepoError> {
        let pk = format!("IMPORT#{import_id}");
        self.db
            .update_item(
                &pk,
                ImportRecord::sk(),
                "REMOVE runner_instance, runner_heartbeat_ms",
                HashMap::new(),
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
            state: ThreadState::Pending, ogre_doc_id: None };
        let item = thread_to_item(&t);
        assert!(!item.contains_key("token") && !item.contains_key("secret"));
        assert_eq!(thread_from_item(&item).expect("from_item"), t);
    }
}
