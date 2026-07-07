// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for security audit-log events.
//!
//! Storage shape mirrors `admin_audit_repo` column-for-column so a
//! future consolidation into a single unified audit table is a
//! prefix rename, not a schema migration. The typed
//! `SecurityAuditAction` variant is decomposed into the `action` tag
//! column and a `detail` JSON-string column at write time; the read
//! path reassembles via `SecurityAuditAction::from_storage`.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::security_audit::{SecurityAudit, SecurityAuditAction};

pub struct SecurityAuditRepo {
    db: DynamoClient,
}

impl SecurityAuditRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Persist one audit row. Idempotent on `audit_id`: a re-write
    /// with the same PK+SK overwrites the existing item, which is
    /// safe because the row's content is fully determined by the
    /// action variant.
    pub async fn create(&self, audit: &SecurityAudit) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(audit.pk()));
        item.insert("SK".to_string(), AttributeValue::S(audit.sk()));
        item.insert("audit_id".to_string(), AttributeValue::S(audit.audit_id.clone()));
        item.insert(
            "target_user_id".to_string(),
            AttributeValue::S(audit.user_id.clone()),
        );
        item.insert(
            "actor_id".to_string(),
            AttributeValue::S(audit.actor_id.clone()),
        );
        item.insert(
            "action".to_string(),
            AttributeValue::S(audit.action.as_str().to_string()),
        );
        item.insert(
            "detail".to_string(),
            AttributeValue::S(audit.action.detail_json().to_string()),
        );
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(audit.created_at.to_string()),
        );

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List audit rows for a user, newest first. Mirrors
    /// `AdminAuditRepo::list_for_user` so a future admin-audit-viewer
    /// can fan out across both tables with one shared signature.
    pub async fn list_for_user(
        &self,
        user_id: &str,
        limit: usize,
    ) -> Result<Vec<SecurityAudit>, RepoError> {
        let pk = format!("USER#{user_id}");

        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(":prefix", AttributeValue::S("SEC_AUDIT#".to_string()))
            .scan_index_forward(false)
            .limit(limit as i32)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        items.iter().map(audit_from_item).collect()
    }

    /// Phase 4 M-E6 piece D — retention sweep for one user. Deletes
    /// every SecurityAudit row under PK=USER#<id> with
    /// `created_at < cutoff_usec`. Returns the count deleted.
    ///
    /// Two-step (list-then-delete) on purpose:
    ///   - DynamoDB has no `DELETE WHERE` operation; we have to read
    ///     keys back before issuing DeleteItem.
    ///   - The SK encodes `created_at` as a zero-padded 20-digit
    ///     prefix, so we *could* range-scan `SK < SEC_AUDIT#<cutoff>`
    ///     directly. We don't, because the read step also lets us
    ///     emit a tracing event per deleted row (forensic recoverability
    ///     if the worker accidentally over-deletes due to a bad config)
    ///     and bounds the per-tick batch size on the *deletes* not on
    ///     the SK range — a user with no eligible rows costs one Query
    ///     either way.
    ///
    /// `max_batch` caps how many rows this call will delete in one
    /// pass; the scheduler can call again next tick to drain a backlog
    /// gradually rather than blocking on a huge sweep.
    pub async fn delete_older_than_for_user(
        &self,
        user_id: &str,
        cutoff_usec: i64,
        max_batch: usize,
    ) -> Result<usize, RepoError> {
        let pk = format!("USER#{user_id}");
        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)")
            .expression_attribute_values(":pk", AttributeValue::S(pk.clone()))
            .expression_attribute_values(":prefix", AttributeValue::S("SEC_AUDIT#".to_string()))
            .scan_index_forward(true) // oldest first — we want to delete those
            .limit(max_batch as i32)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        let mut deleted = 0usize;
        for item in items {
            let created_at = get_n(&item, "created_at")?;
            if created_at >= cutoff_usec {
                // The oldest row still falls inside the retention
                // window; everything newer than it does too. No more
                // deletes possible this pass.
                break;
            }
            let sk = item
                .get("SK")
                .and_then(|v| v.as_s().ok())
                .ok_or_else(|| RepoError::MissingField("SK".to_string()))?;
            self.db
                .delete_item(&pk, sk)
                .await
                .map_err(|e| RepoError::Dynamo(e.to_string()))?;
            deleted += 1;
        }
        Ok(deleted)
    }
}

fn audit_from_item(item: &HashMap<String, AttributeValue>) -> Result<SecurityAudit, RepoError> {
    let action_tag = get_s(item, "action")?;
    let detail_str = item
        .get("detail")
        .and_then(|v| v.as_s().ok())
        .cloned()
        .unwrap_or_else(|| "{}".to_string());
    let detail: serde_json::Value = serde_json::from_str(&detail_str)
        .map_err(|e| RepoError::MissingField(format!("detail (invalid JSON): {e}")))?;
    let action = SecurityAuditAction::from_storage(&action_tag, &detail)
        .map_err(RepoError::MissingField)?;

    Ok(SecurityAudit {
        audit_id: get_s(item, "audit_id")?,
        user_id: get_s(item, "target_user_id")?,
        actor_id: get_s(item, "actor_id")?,
        action,
        created_at: get_n(item, "created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(action: SecurityAuditAction) -> SecurityAudit {
        SecurityAudit {
            audit_id: "aud1".to_string(),
            user_id: "alice".to_string(),
            actor_id: "alice".to_string(),
            action,
            created_at: 1_700_000_000_000_000,
        }
    }

    /// Mimic the `create` path's column construction without
    /// going through `DynamoClient::put_item`, then feed the
    /// resulting item through `audit_from_item`. This is the only
    /// way to exercise the storage decomposition in a unit test
    /// (the real put_item needs a live table).
    fn item_for(audit: &SecurityAudit) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(audit.pk()));
        item.insert("SK".to_string(), AttributeValue::S(audit.sk()));
        item.insert("audit_id".to_string(), AttributeValue::S(audit.audit_id.clone()));
        item.insert("target_user_id".to_string(), AttributeValue::S(audit.user_id.clone()));
        item.insert("actor_id".to_string(), AttributeValue::S(audit.actor_id.clone()));
        item.insert("action".to_string(), AttributeValue::S(audit.action.as_str().to_string()));
        item.insert("detail".to_string(), AttributeValue::S(audit.action.detail_json().to_string()));
        item.insert("created_at".to_string(), AttributeValue::N(audit.created_at.to_string()));
        item
    }

    #[test]
    fn round_trip_login_success() {
        let original = fixture(SecurityAuditAction::LoginSuccess);
        let item = item_for(&original);
        let back = audit_from_item(&item).expect("from_item");
        assert_eq!(back.audit_id, original.audit_id);
        assert_eq!(back.user_id, original.user_id);
        assert_eq!(back.action, original.action);
    }

    #[test]
    fn round_trip_login_failure_carries_reason() {
        let original = fixture(SecurityAuditAction::LoginFailure {
            reason: "bad_password".to_string(),
        });
        let back = audit_from_item(&item_for(&original)).expect("from_item");
        assert_eq!(back.action, original.action);
    }

    #[test]
    fn round_trip_share_revoked_carries_both_fields() {
        let original = fixture(SecurityAuditAction::ShareRevoked {
            doc_id: "doc1".to_string(),
            target: "bob@example.com".to_string(),
        });
        let back = audit_from_item(&item_for(&original)).expect("from_item");
        assert_eq!(back.action, original.action);
    }

    #[test]
    fn round_trip_covers_every_variant() {
        // The model-level roundtrip walks every variant through
        // `detail_json` + `from_storage`, but the repo path adds
        // serialize-to-string + AttributeValue wrapping + parse-back
        // — a layer that could regress independently (e.g. wrong
        // AttributeValue kind). Exercise every variant here too so
        // an MFA / SAML / SCIM writer in M-E3/4/5 doesn't discover
        // the column-decomposition bug only when it tries to read
        // its own writes.
        let cases = vec![
            SecurityAuditAction::MfaEnroll,
            SecurityAuditAction::MfaVerify { ok: true },
            SecurityAuditAction::MfaVerify { ok: false },
            SecurityAuditAction::MfaRecoveryUsed,
            SecurityAuditAction::MfaRecoveryFailed,
            SecurityAuditAction::MfaDisarm,
            SecurityAuditAction::SamlAssertionAccepted {
                workspace_id: "ws1".to_string(),
                name_id: "alice@example.com".to_string(),
            },
            SecurityAuditAction::ScimTokenUsed {
                token_id: "tok1".to_string(),
                op: "createUser".to_string(),
            },
            SecurityAuditAction::SessionRevoked {
                reason: "admin_disable".to_string(),
            },
            SecurityAuditAction::ShareGranted {
                doc_id: "doc1".to_string(),
                target: "bob".to_string(),
                level: "EDIT".to_string(),
            },
            SecurityAuditAction::ShareUpdated {
                doc_id: "doc1".to_string(),
                target: "bob".to_string(),
                level: "VIEW".to_string(),
            },
            SecurityAuditAction::ProfileUpdated {
                name_changed: true,
                avatar_changed: false,
            },
            SecurityAuditAction::SystemUserProvisioned,
            SecurityAuditAction::TemplateGalleryCreated {
                workspace_id: "ws1".to_string(),
                gallery_id: "gal1".to_string(),
            },
            SecurityAuditAction::TemplateGalleryUpdated {
                workspace_id: "ws1".to_string(),
                gallery_id: "gal1".to_string(),
            },
            SecurityAuditAction::TemplateGalleryDeleted {
                workspace_id: "ws1".to_string(),
                gallery_id: "gal1".to_string(),
            },
        ];
        for action in cases {
            let original = fixture(action.clone());
            let back = audit_from_item(&item_for(&original))
                .unwrap_or_else(|e| panic!("repo roundtrip failed for {action:?}: {e}"));
            assert_eq!(back.action, action, "repo roundtrip mismatch on {action:?}");
        }
    }

    #[test]
    fn round_trip_doc_deleted_preserves_hard_flag() {
        // The `hard: bool` distinguishes soft-delete-to-trash from
        // trash-cleanup purge. Losing this bit would silently
        // misclassify retention events.
        let soft = fixture(SecurityAuditAction::DocDeleted {
            doc_id: "doc1".to_string(),
            hard: false,
        });
        let hard = fixture(SecurityAuditAction::DocDeleted {
            doc_id: "doc1".to_string(),
            hard: true,
        });
        assert_eq!(audit_from_item(&item_for(&soft)).unwrap().action, soft.action);
        assert_eq!(audit_from_item(&item_for(&hard)).unwrap().action, hard.action);
    }

    #[test]
    fn audit_from_item_fails_when_action_missing() {
        let audit = fixture(SecurityAuditAction::LoginSuccess);
        let mut item = item_for(&audit);
        item.remove("action");
        let err = audit_from_item(&item).expect_err("missing action must error");
        match err {
            RepoError::MissingField(f) => assert_eq!(f, "action"),
            other => panic!("expected MissingField(action), got {other:?}"),
        }
    }

    #[test]
    fn audit_from_item_fails_when_detail_field_missing() {
        // The right tag with the wrong payload (data-loss event)
        // must surface — not silently default the variant payload.
        let audit = fixture(SecurityAuditAction::LoginFailure {
            reason: "x".to_string(),
        });
        let mut item = item_for(&audit);
        item.insert("detail".to_string(), AttributeValue::S("{}".to_string()));
        let err = audit_from_item(&item).expect_err("missing reason must error");
        match err {
            RepoError::MissingField(f) => assert!(f.contains("reason"), "got: {f}"),
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    #[test]
    fn audit_from_item_fails_on_corrupt_detail_json() {
        let audit = fixture(SecurityAuditAction::LoginSuccess);
        let mut item = item_for(&audit);
        item.insert("detail".to_string(), AttributeValue::S("{not json".to_string()));
        let err = audit_from_item(&item).expect_err("invalid JSON must error");
        match err {
            RepoError::MissingField(f) => assert!(f.contains("detail"), "got: {f}"),
            other => panic!("expected MissingField, got {other:?}"),
        }
    }
}
