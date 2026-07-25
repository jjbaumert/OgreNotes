// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::import::{ImportRecord, ImportStatus};
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
}
