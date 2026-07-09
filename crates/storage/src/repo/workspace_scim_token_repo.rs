// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for per-workspace SCIM bearer tokens (Phase 4 M-E5
//! piece A).
//!
//! Many rows per workspace; lookup by `(workspace_id, token_id)` is
//! the hot path (every SCIM request hits it) so the row is keyed as
//! `PK = WORKSPACE#<workspace_id>`, `SK = SCIM_TOKEN#<token_id>` and
//! a `get_item` is O(1). `list_for_workspace` issues a single
//! `begins_with` query for the admin-console workspace token page.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::workspace_scim_token::{BcryptHash, WorkspaceScimToken};

pub struct WorkspaceScimTokenRepo {
    db: DynamoClient,
}

impl WorkspaceScimTokenRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Upsert a token row. Used at issuance and (with the same
    /// shape) when toggling `disabled_at` via `set_disabled_at`.
    pub async fn put(&self, token: &WorkspaceScimToken) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(token.pk()));
        item.insert("SK".to_string(), AttributeValue::S(token.sk()));
        item.insert(
            "workspace_id".to_string(),
            AttributeValue::S(token.workspace_id.clone()),
        );
        item.insert(
            "token_id".to_string(),
            AttributeValue::S(token.token_id.clone()),
        );
        item.insert(
            "secret_hash".to_string(),
            AttributeValue::S(token.secret_hash.as_str().to_string()),
        );
        item.insert("name".to_string(), AttributeValue::S(token.name.clone()));
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(token.created_at.to_string()),
        );
        item.insert(
            "last_used_at".to_string(),
            AttributeValue::N(token.last_used_at.to_string()),
        );
        item.insert(
            "disabled_at".to_string(),
            AttributeValue::N(token.disabled_at.to_string()),
        );
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Look up a single token by `(workspace_id, token_id)`. Called
    /// on every SCIM request via the ScimAuth extractor.
    pub async fn get(
        &self,
        workspace_id: &str,
        token_id: &str,
    ) -> Result<Option<WorkspaceScimToken>, RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let sk = WorkspaceScimToken::sk_for(token_id);
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        match item {
            Some(it) => Ok(Some(from_item(&it)?)),
            None => Ok(None),
        }
    }

    /// All tokens for one workspace (active + disabled), for the
    /// admin-console list view. Disabled rows are returned so the UI
    /// can render history; the ScimAuth extractor uses `is_active`
    /// to gate runtime use.
    pub async fn list_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceScimToken>, RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let items = self
            .db
            .query(&pk, Some("SCIM_TOKEN#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        items.iter().map(from_item).collect()
    }

    /// Stamp `last_used_at` after a successful SCIM request. Fire-
    /// and-forget at call sites: a failure here must NOT reject the
    /// request the user is already authenticated for. Returns an
    /// `Err` so the caller can choose to log; the request itself
    /// must succeed regardless.
    pub async fn touch_last_used(
        &self,
        workspace_id: &str,
        token_id: &str,
        at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let sk = WorkspaceScimToken::sk_for(token_id);
        let mut values = HashMap::new();
        values.insert(":lu".to_string(), AttributeValue::N(at.to_string()));
        self.db
            .update_item(&pk, &sk, "SET last_used_at = :lu", values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Revoke a token by setting `disabled_at`. The row is kept so
    /// historical audit references to its `token_id` still resolve.
    /// `at` must be non-zero — `0` is the "active" sentinel, so
    /// passing it would silently re-enable a previously revoked
    /// token. The repo rejects that input to close the footgun.
    pub async fn set_disabled_at(
        &self,
        workspace_id: &str,
        token_id: &str,
        at: i64,
    ) -> Result<(), RepoError> {
        if at == 0 {
            return Err(RepoError::InvalidArgument(
                "set_disabled_at: `at` must be non-zero (0 is the active sentinel)".to_string(),
            ));
        }
        let pk = format!("WORKSPACE#{workspace_id}");
        let sk = WorkspaceScimToken::sk_for(token_id);
        let mut values = HashMap::new();
        values.insert(":d".to_string(), AttributeValue::N(at.to_string()));
        self.db
            .update_item(&pk, &sk, "SET disabled_at = :d", values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
}

fn from_item(item: &HashMap<String, AttributeValue>) -> Result<WorkspaceScimToken, RepoError> {
    Ok(WorkspaceScimToken {
        workspace_id: get_s(item, "workspace_id")?,
        token_id: get_s(item, "token_id")?,
        secret_hash: BcryptHash::new(get_s(item, "secret_hash")?),
        name: get_s(item, "name")?,
        created_at: get_n(item, "created_at")?,
        last_used_at: get_n(item, "last_used_at")?,
        disabled_at: get_n(item, "disabled_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_item_round_trips_put_shape() {
        let token = WorkspaceScimToken {
            workspace_id: "ws-1".to_string(),
            token_id: "tok-12345".to_string(),
            secret_hash: BcryptHash::new("$2b$12$abcdef".to_string()),
            name: "Okta connector".to_string(),
            created_at: 1_700_000_000_000_000,
            last_used_at: 0,
            disabled_at: 0,
        };
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(token.pk()));
        item.insert("SK".to_string(), AttributeValue::S(token.sk()));
        item.insert(
            "workspace_id".to_string(),
            AttributeValue::S(token.workspace_id.clone()),
        );
        item.insert(
            "token_id".to_string(),
            AttributeValue::S(token.token_id.clone()),
        );
        item.insert(
            "secret_hash".to_string(),
            AttributeValue::S(token.secret_hash.as_str().to_string()),
        );
        item.insert("name".to_string(), AttributeValue::S(token.name.clone()));
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(token.created_at.to_string()),
        );
        item.insert(
            "last_used_at".to_string(),
            AttributeValue::N(token.last_used_at.to_string()),
        );
        item.insert(
            "disabled_at".to_string(),
            AttributeValue::N(token.disabled_at.to_string()),
        );
        let back = from_item(&item).unwrap();
        assert_eq!(back, token);
    }

    #[test]
    fn from_item_missing_secret_hash_errors() {
        // A token row without its hash could never verify; it must
        // fail decode rather than come back with an empty hash.
        let mut item = HashMap::new();
        item.insert("workspace_id".to_string(), AttributeValue::S("ws-1".to_string()));
        item.insert("token_id".to_string(), AttributeValue::S("tok-1".to_string()));
        item.insert("name".to_string(), AttributeValue::S("n".to_string()));
        item.insert("created_at".to_string(), AttributeValue::N("1".to_string()));
        item.insert("last_used_at".to_string(), AttributeValue::N("0".to_string()));
        item.insert("disabled_at".to_string(), AttributeValue::N("0".to_string()));
        match from_item(&item) {
            Err(RepoError::MissingField(f)) => assert_eq!(f, "secret_hash"),
            other => panic!("expected MissingField(secret_hash), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_disabled_at_rejects_zero_before_any_io() {
        // `0` is the "active" sentinel: letting it through would
        // silently re-enable a revoked token. The guard fires before
        // any network call, so an offline client suffices.
        let conf = aws_sdk_dynamodb::Config::builder()
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .build();
        let repo = WorkspaceScimTokenRepo::new(crate::dynamo::DynamoClient::new(
            aws_sdk_dynamodb::Client::from_conf(conf),
            "test-table".to_string(),
        ));
        let err = repo
            .set_disabled_at("ws-1", "tok-1", 0)
            .await
            .expect_err("at=0 must be rejected");
        match err {
            RepoError::InvalidArgument(msg) => {
                assert!(msg.contains("non-zero"), "got: {msg}")
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }
}
