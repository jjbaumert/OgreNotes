// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

pub mod activity_repo;
pub mod admin_audit_repo;
pub mod doc_repo;
pub mod folder_repo;
pub mod mfa_recovery_repo;
pub mod notification_repo;
pub mod security_audit_repo;
pub mod session_repo;
pub mod snapshot_repo;
pub mod template_gallery_repo;
pub mod thread_repo;
pub mod user_repo;
pub mod workspace_repo;
pub mod workspace_saml_config_repo;
pub mod workspace_scim_token_repo;

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

/// Errors from repository operations.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("DynamoDB error: {0}")]
    Dynamo(String),

    #[error("missing field: {0}")]
    MissingField(String),

    #[error("S3 error: {0}")]
    S3(String),

    /// Caller passed a semantically invalid argument that the repo
    /// refuses to write because it would corrupt downstream
    /// semantics (e.g. passing 0 to a "set disabled timestamp"
    /// method would silently re-enable a revoked row).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// A bounded read would have materialized more bytes than the
    /// caller's cap allows (#91). Currently fires from
    /// `get_pending_updates` when an accumulated UPDATE# tail
    /// exceeds the per-request memory budget. The caller's
    /// recovery path is to surface a 503 with the cap in the
    /// message — failing one doc is the correct trade-off vs an
    /// OOM cascade taking the whole task down.
    #[error("response exceeded cap on {what}: {actual} bytes > {cap}-byte cap")]
    TooLarge {
        /// Short label identifying what blew the cap (e.g.
        /// `"pending updates for doc-X"`). Surfaced in the
        /// 503 body so operators can correlate.
        what: String,
        /// Bytes seen before bailing. The function bails the
        /// instant the running total exceeds `cap`, so this is
        /// at least `cap + 1` rather than the full count of
        /// what would have been loaded.
        actual: usize,
        /// The cap that was tripped.
        cap: usize,
    },
}

/// Extract a string attribute from a DynamoDB item.
pub(crate) fn get_s(
    item: &HashMap<String, AttributeValue>,
    key: &str,
) -> Result<String, RepoError> {
    item.get(key)
        .and_then(|v| v.as_s().ok())
        .cloned()
        .ok_or_else(|| RepoError::MissingField(key.to_string()))
}

/// Extract a numeric i64 attribute from a DynamoDB item.
pub(crate) fn get_n(
    item: &HashMap<String, AttributeValue>,
    key: &str,
) -> Result<i64, RepoError> {
    item.get(key)
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse::<i64>().ok())
        .ok_or_else(|| RepoError::MissingField(key.to_string()))
}

/// Extract a numeric u64 attribute from a DynamoDB item.
pub(crate) fn get_n_u64(
    item: &HashMap<String, AttributeValue>,
    key: &str,
) -> Result<u64, RepoError> {
    item.get(key)
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(|| RepoError::MissingField(key.to_string()))
}
