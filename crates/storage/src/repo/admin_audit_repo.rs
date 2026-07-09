// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for admin audit-log events.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::admin_audit::{AdminAudit, AdminAuditAction};

/// #49: actor-centric forensic index. Hash on the admin who performed
/// the action, range on `created_at`, so "every action admin Y took
/// since T" is a bounded query instead of a full-table scan. Sparse —
/// only AdminAudit rows set `actor_id_gsi`, so the index contains
/// exactly the admin-action rows. The literal must match the
/// `index_name` declared in `setup_dev` and `scripts/aws-test-deploy.sh`.
const ADMIN_AUDIT_ACTOR_INDEX: &str = "GSI8-actor-created";

pub struct AdminAuditRepo {
    db: DynamoClient,
}

impl AdminAuditRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Persist one audit row. Idempotent on `audit_id`: if a row with the
    /// same PK+SK already exists DynamoDB will overwrite it, which is
    /// fine — the audit row's content is fully determined by the action.
    pub async fn create(&self, audit: &AdminAudit) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(audit.pk()));
        item.insert("SK".to_string(), AttributeValue::S(audit.sk()));
        item.insert("audit_id".to_string(), AttributeValue::S(audit.audit_id.clone()));
        item.insert(
            "target_user_id".to_string(),
            AttributeValue::S(audit.target_user_id.clone()),
        );
        item.insert("actor_id".to_string(), AttributeValue::S(audit.actor_id.clone()));
        // #49: hash key for the actor-centric forensic index (GSI8).
        // Sparse — present only on audit rows, so the index never picks
        // up unrelated items that happen to carry `created_at`.
        item.insert(
            "actor_id_gsi".to_string(),
            AttributeValue::S(audit.actor_id.clone()),
        );
        item.insert(
            "action".to_string(),
            AttributeValue::S(audit.action.as_str().to_string()),
        );
        item.insert("detail".to_string(), AttributeValue::S(audit.detail.clone()));
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(audit.created_at.to_string()),
        );

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List audit rows for a target user, newest first.
    pub async fn list_for_user(
        &self,
        target_user_id: &str,
        limit: usize,
    ) -> Result<Vec<AdminAudit>, RepoError> {
        let pk = format!("USER#{target_user_id}");

        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(":prefix", AttributeValue::S("ADMIN_AUDIT#".to_string()))
            .scan_index_forward(false)
            .limit(limit as i32)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        items.iter().map(audit_from_item).collect()
    }

    /// List audit rows for an *actor* (the admin who performed the
    /// action), newest first, bounded to actions at or after `since`
    /// (same epoch units as `AdminAudit::created_at`). Backs the
    /// incident-response question "what else did this — possibly
    /// compromised — admin do?", which the target-keyed PK cannot answer
    /// without a table scan. Joins the sparse `GSI8-actor-created` index
    /// (#49).
    pub async fn list_by_actor(
        &self,
        actor_id: &str,
        since: i64,
        limit: usize,
    ) -> Result<Vec<AdminAudit>, RepoError> {
        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .index_name(ADMIN_AUDIT_ACTOR_INDEX)
            .key_condition_expression("actor_id_gsi = :actor AND created_at >= :since")
            .expression_attribute_values(":actor", AttributeValue::S(actor_id.to_string()))
            .expression_attribute_values(":since", AttributeValue::N(since.to_string()))
            .scan_index_forward(false)
            .limit(limit as i32)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        items.iter().map(audit_from_item).collect()
    }
}

fn audit_from_item(item: &HashMap<String, AttributeValue>) -> Result<AdminAudit, RepoError> {
    let action_str = get_s(item, "action")?;
    let action = match action_str.as_str() {
        "disable" => AdminAuditAction::Disable,
        "enable" => AdminAuditAction::Enable,
        "promote" => AdminAuditAction::Promote,
        "demote" => AdminAuditAction::Demote,
        // #148 — new preferred string; keep legacy alias so
        // rows written before the migration continue to
        // parse.
        "setAskPolicy" => AdminAuditAction::SetAskPolicy,
        "setAskEnabled" => AdminAuditAction::SetAskPolicy,
        other => {
            return Err(RepoError::MissingField(format!(
                "unknown admin audit action: {other}"
            )))
        }
    };

    Ok(AdminAudit {
        audit_id: get_s(item, "audit_id")?,
        target_user_id: get_s(item, "target_user_id")?,
        actor_id: get_s(item, "actor_id")?,
        action,
        detail: item
            .get("detail")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        created_at: get_n(item, "created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(action: AdminAuditAction) -> AdminAudit {
        AdminAudit {
            audit_id: "aud1".to_string(),
            target_user_id: "victim".to_string(),
            actor_id: "admin".to_string(),
            action,
            detail: "{}".to_string(),
            created_at: 1_700_000_000_000_000,
        }
    }

    /// Mimic `create`'s column construction without a live table,
    /// so `audit_from_item` can be exercised as its inverse. Mirrors
    /// the `item_for` idiom in security_audit_repo's tests.
    fn item_for(audit: &AdminAudit) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(audit.pk()));
        item.insert("SK".to_string(), AttributeValue::S(audit.sk()));
        item.insert("audit_id".to_string(), AttributeValue::S(audit.audit_id.clone()));
        item.insert(
            "target_user_id".to_string(),
            AttributeValue::S(audit.target_user_id.clone()),
        );
        item.insert("actor_id".to_string(), AttributeValue::S(audit.actor_id.clone()));
        item.insert(
            "actor_id_gsi".to_string(),
            AttributeValue::S(audit.actor_id.clone()),
        );
        item.insert(
            "action".to_string(),
            AttributeValue::S(audit.action.as_str().to_string()),
        );
        item.insert("detail".to_string(), AttributeValue::S(audit.detail.clone()));
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(audit.created_at.to_string()),
        );
        item
    }

    #[test]
    fn round_trip_covers_every_action_variant() {
        // The write path stores `action.as_str()`; the read path
        // hand-matches those strings. If a new variant lands in
        // as_str() without a matching arm here, its rows become
        // unreadable — this test catches the drift for all current
        // variants.
        let cases = [
            AdminAuditAction::Disable,
            AdminAuditAction::Enable,
            AdminAuditAction::Promote,
            AdminAuditAction::Demote,
            AdminAuditAction::SetAskPolicy,
        ];
        for action in cases {
            let original = fixture(action.clone());
            let back = audit_from_item(&item_for(&original))
                .unwrap_or_else(|e| panic!("roundtrip failed for {action:?}: {e}"));
            assert_eq!(back.action, action, "roundtrip mismatch on {action:?}");
            assert_eq!(back.audit_id, original.audit_id);
            assert_eq!(back.target_user_id, original.target_user_id);
            assert_eq!(back.actor_id, original.actor_id);
            assert_eq!(back.created_at, original.created_at);
        }
    }

    #[test]
    fn legacy_set_ask_enabled_action_string_still_parses() {
        // #148 — rows written before the AskPolicy migration carry
        // "setAskEnabled" in the action column. They must decode to
        // SetAskPolicy forever; there is no migration step.
        let mut item = item_for(&fixture(AdminAuditAction::SetAskPolicy));
        item.insert(
            "action".to_string(),
            AttributeValue::S("setAskEnabled".to_string()),
        );
        let back = audit_from_item(&item).expect("legacy action string must parse");
        assert_eq!(back.action, AdminAuditAction::SetAskPolicy);
    }

    #[test]
    fn unknown_action_string_fails_naming_the_tag() {
        // A row written by a newer binary (or corrupted) must fail
        // loud with the offending tag, not decode to a default.
        let mut item = item_for(&fixture(AdminAuditAction::Disable));
        item.insert(
            "action".to_string(),
            AttributeValue::S("obliterate".to_string()),
        );
        let err = audit_from_item(&item).expect_err("unknown action must error");
        match err {
            RepoError::MissingField(msg) => {
                assert!(msg.contains("obliterate"), "error must name the tag: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    #[test]
    fn missing_detail_defaults_to_empty_string() {
        // `detail` is the only lenient column: absence reads as ""
        // rather than an error, so a hand-written or partial row
        // still lists in the admin console.
        let mut item = item_for(&fixture(AdminAuditAction::Promote));
        item.remove("detail");
        let back = audit_from_item(&item).expect("missing detail is tolerated");
        assert_eq!(back.detail, "");
    }

    #[test]
    fn missing_required_columns_error() {
        for required in ["audit_id", "target_user_id", "actor_id", "created_at"] {
            let mut item = item_for(&fixture(AdminAuditAction::Enable));
            item.remove(required);
            let result = audit_from_item(&item);
            match result {
                Err(RepoError::MissingField(f)) => assert_eq!(f, required),
                other => panic!("missing {required} must error, got {other:?}"),
            }
        }
    }
}
