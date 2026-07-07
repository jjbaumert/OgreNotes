// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for MFA recovery codes (Phase 4 M-E3).
//!
//! Storage pattern is one DynamoDB row per code, keyed under the
//! owning user's PK. The plaintext is never stored — only the bcrypt
//! hash. A successful redemption deletes the row, so re-use is
//! structurally impossible. Re-enrollment deletes all rows first so
//! stale codes from a prior enrollment don't survive a re-mint of
//! the TOTP secret.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::mfa_recovery::MfaRecoveryCode;

pub struct MfaRecoveryRepo {
    db: DynamoClient,
}

impl MfaRecoveryRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Construct + insert a single recovery row from its raw
    /// ingredients. Lets L4 callers stay out of the `MfaRecoveryCode`
    /// type — the repo owns the model construction, the handler just
    /// supplies the bcrypt hash it computed.
    pub async fn put_hashed(
        &self,
        user_id: &str,
        idx: usize,
        bcrypt_hash: &str,
        created_at: i64,
    ) -> Result<(), RepoError> {
        let row = MfaRecoveryCode {
            user_id: user_id.to_string(),
            idx: idx as u8,
            bcrypt_hash: bcrypt_hash.to_string(),
            created_at,
        };
        self.put(&row).await
    }

    /// Insert one row. Called in a loop at enroll-time (always 10
    /// codes; a transactWrite-of-10 isn't worth the API ceiling
    /// budget for a once-per-user operation).
    pub async fn put(&self, code: &MfaRecoveryCode) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(code.pk()));
        item.insert("SK".to_string(), AttributeValue::S(code.sk()));
        item.insert("user_id".to_string(), AttributeValue::S(code.user_id.clone()));
        item.insert("idx".to_string(), AttributeValue::N(code.idx.to_string()));
        item.insert(
            "bcrypt_hash".to_string(),
            AttributeValue::S(code.bcrypt_hash.clone()),
        );
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(code.created_at.to_string()),
        );
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Fetch every recovery row for a user. Used by the redemption
    /// flow: the caller bcrypt-verifies each row against the presented
    /// plaintext and consumes the match. Bounded by
    /// `RECOVERY_CODE_COUNT` so there's no pagination concern.
    pub async fn list_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<MfaRecoveryCode>, RepoError> {
        let pk = format!("USER#{user_id}");
        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(
                ":prefix",
                AttributeValue::S(MfaRecoveryCode::sk_prefix().to_string()),
            )
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        result
            .items
            .unwrap_or_default()
            .iter()
            .map(from_item)
            .collect()
    }

    /// Delete a single row after successful consumption.
    pub async fn delete(&self, user_id: &str, idx: u8) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let sk = format!("MFA_RECOVERY#{:02}", idx);
        self.db
            .delete_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Wipe all recovery rows for a user. Called at disarm time and
    /// before a re-enrollment writes its fresh batch.
    pub async fn delete_all_for_user(&self, user_id: &str) -> Result<(), RepoError> {
        let rows = self.list_for_user(user_id).await?;
        for row in rows {
            self.delete(user_id, row.idx).await?;
        }
        Ok(())
    }
}

fn from_item(item: &HashMap<String, AttributeValue>) -> Result<MfaRecoveryCode, RepoError> {
    let idx_str = get_s(item, "idx").or_else(|_| {
        item.get("idx")
            .and_then(|v| v.as_n().ok())
            .cloned()
            .ok_or_else(|| RepoError::MissingField("idx".to_string()))
    })?;
    let idx: u8 = idx_str
        .parse()
        .map_err(|_| RepoError::MissingField(format!("idx (unparseable: {idx_str})")))?;
    Ok(MfaRecoveryCode {
        user_id: get_s(item, "user_id")?,
        idx,
        bcrypt_hash: get_s(item, "bcrypt_hash")?,
        created_at: get_n(item, "created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(idx: u8) -> MfaRecoveryCode {
        MfaRecoveryCode {
            user_id: "alice".to_string(),
            idx,
            bcrypt_hash: "$2b$10$placeholder".to_string(),
            created_at: 1_700_000_000_000_000,
        }
    }

    /// The serialize side of the repo (no live DDB needed). Confirms
    /// `from_item` is the inverse of the column construction in
    /// `put`. The full integration round-trip lands in test_mfa.rs
    /// alongside the endpoint tests.
    #[test]
    fn from_item_round_trips_a_put_shape() {
        let code = fixture(7);
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(code.pk()));
        item.insert("SK".to_string(), AttributeValue::S(code.sk()));
        item.insert("user_id".to_string(), AttributeValue::S(code.user_id.clone()));
        item.insert("idx".to_string(), AttributeValue::N(code.idx.to_string()));
        item.insert(
            "bcrypt_hash".to_string(),
            AttributeValue::S(code.bcrypt_hash.clone()),
        );
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(code.created_at.to_string()),
        );
        let back = from_item(&item).expect("from_item");
        assert_eq!(back.user_id, code.user_id);
        assert_eq!(back.idx, code.idx);
        assert_eq!(back.bcrypt_hash, code.bcrypt_hash);
        assert_eq!(back.created_at, code.created_at);
    }
}
