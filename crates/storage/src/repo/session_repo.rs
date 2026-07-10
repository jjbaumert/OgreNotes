// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::session::Session;
use crate::repo::{RepoError, get_s, get_n};

/// Repository for session operations.
pub struct SessionRepo {
    db: DynamoClient,
}

impl SessionRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Create a new session.
    pub async fn create(&self, session: &Session) -> Result<(), RepoError> {
        let mut item = session_to_item(session);
        item.insert("PK".to_string(), AttributeValue::S(session.pk()));
        item.insert("SK".to_string(), AttributeValue::S(session.sk()));

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get a session by user_id and session_id.
    pub async fn get(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<Session>, RepoError> {
        let pk = format!("USER#{user_id}");
        let sk = format!("SESSION#{session_id}");
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(session_from_item(&item)?)),
            None => Ok(None),
        }
    }

    /// Delete a specific session.
    pub async fn delete(&self, user_id: &str, session_id: &str) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let sk = format!("SESSION#{session_id}");
        self.db
            .delete_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Delete all sessions for a user (e.g., on refresh token reuse detection).
    pub async fn delete_all_for_user(&self, user_id: &str) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let items = self
            .db
            .query(&pk, Some("SESSION#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        for item in &items {
            if let Some(sk) = item.get("SK").and_then(|v| v.as_s().ok()) {
                self.db
                    .delete_item(&pk, sk)
                    .await
                    .map_err(|e| RepoError::Dynamo(e.to_string()))?;
            }
        }

        Ok(())
    }

    /// Update the refresh token hash (for rotation).
    pub async fn update_refresh_token(
        &self,
        user_id: &str,
        session_id: &str,
        new_hash: &str,
        new_expires_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let sk = format!("SESSION#{session_id}");
        let mut values = HashMap::new();
        values.insert(
            ":hash".to_string(),
            AttributeValue::S(new_hash.to_string()),
        );
        values.insert(
            ":expires_at".to_string(),
            AttributeValue::N(new_expires_at.to_string()),
        );

        self.db
            .update_item(
                &pk,
                &sk,
                "SET refresh_token_hash = :hash, expires_at = :expires_at",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
}

fn session_to_item(session: &Session) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert(
        "user_id".to_string(),
        AttributeValue::S(session.user_id.clone()),
    );
    item.insert(
        "session_id".to_string(),
        AttributeValue::S(session.session_id.clone()),
    );
    item.insert(
        "refresh_token_hash".to_string(),
        AttributeValue::S(session.refresh_token_hash.clone()),
    );
    item.insert(
        "expires_at".to_string(),
        AttributeValue::N(session.expires_at.to_string()),
    );
    if let Some(ref info) = session.device_info {
        item.insert("device_info".to_string(), AttributeValue::S(info.clone()));
    }
    item.insert(
        "created_at".to_string(),
        AttributeValue::N(session.created_at.to_string()),
    );
    item
}

fn session_from_item(item: &HashMap<String, AttributeValue>) -> Result<Session, RepoError> {
    Ok(Session {
        user_id: get_s(item, "user_id")?,
        session_id: get_s(item, "session_id")?,
        refresh_token_hash: get_s(item, "refresh_token_hash")?,
        expires_at: get_n(item, "expires_at")?,
        device_info: item.get("device_info").and_then(|v| v.as_s().ok()).cloned(),
        created_at: get_n(item, "created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(device_info: Option<String>) -> Session {
        Session {
            user_id: "u1".to_string(),
            session_id: "s1".to_string(),
            refresh_token_hash: "sha256:abc".to_string(),
            expires_at: 1_700_000_000_000_000,
            device_info,
            created_at: 1_699_999_999_000_000,
        }
    }

    #[test]
    fn to_item_from_item_round_trips_with_device_info() {
        let session = fixture(Some("Chrome/Linux".to_string()));
        let back = session_from_item(&session_to_item(&session)).expect("from_item");
        assert_eq!(back, session);
    }

    #[test]
    fn device_info_is_sparse_and_decodes_as_none() {
        // `device_info: None` must not write an attribute, and a row
        // without one must decode back to None — the shape every
        // pre-device-tracking session row has.
        let session = fixture(None);
        let item = session_to_item(&session);
        assert!(
            !item.contains_key("device_info"),
            "None device_info must not write an attribute"
        );
        let back = session_from_item(&item).expect("from_item");
        assert_eq!(back, session);
    }

    #[test]
    fn missing_refresh_token_hash_errors() {
        // The token hash is the session's whole reason to exist; a row
        // without it must fail decode, not come back with a default
        // that could never verify (or worse, always "verify").
        let mut item = session_to_item(&fixture(None));
        item.remove("refresh_token_hash");
        match session_from_item(&item) {
            Err(RepoError::MissingField(f)) => assert_eq!(f, "refresh_token_hash"),
            other => panic!("expected MissingField(refresh_token_hash), got {other:?}"),
        }
    }

    #[test]
    fn missing_expires_at_errors() {
        // A session that can't report its expiry must not decode —
        // is_expired() on a defaulted 0 would read as always-expired
        // (fail-safe) but hides the data-loss; surface it instead.
        let mut item = session_to_item(&fixture(None));
        item.remove("expires_at");
        match session_from_item(&item) {
            Err(RepoError::MissingField(f)) => assert_eq!(f, "expires_at"),
            other => panic!("expected MissingField(expires_at), got {other:?}"),
        }
    }
}

