pub mod doc_repo;
pub mod folder_repo;
pub mod notification_repo;
pub mod session_repo;
pub mod snapshot_repo;
pub mod thread_repo;
pub mod user_repo;

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
