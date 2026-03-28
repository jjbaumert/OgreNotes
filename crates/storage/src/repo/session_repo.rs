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

