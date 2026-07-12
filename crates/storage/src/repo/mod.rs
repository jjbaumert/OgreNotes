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

    /// A guarded write (e.g. `attribute_exists(PK)`) found no existing row
    /// to update — the row was deleted concurrently between the caller's
    /// read and this write. Distinct from a plain "not found" read (which
    /// callers usually express as `Option::None`) because it signals a
    /// write that was refused, not absence discovered by a lookup.
    #[error("not found: {0}")]
    NotFound(String),

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

#[cfg(test)]
mod tests {
    use super::*;

    // Every repo decoder in this crate is built on these three
    // extractors, so their contract — Ok on the right AV kind,
    // MissingField naming the key on anything else — is the base
    // invariant of the whole read path.

    fn item_with(key: &str, value: AttributeValue) -> HashMap<String, AttributeValue> {
        HashMap::from([(key.to_string(), value)])
    }

    fn assert_missing_field(err: RepoError, expected_key: &str) {
        match err {
            RepoError::MissingField(k) => assert_eq!(k, expected_key),
            other => panic!("expected MissingField({expected_key}), got {other:?}"),
        }
    }

    #[test]
    fn get_s_returns_string_attribute() {
        let item = item_with("email", AttributeValue::S("a@b.c".to_string()));
        assert_eq!(get_s(&item, "email").unwrap(), "a@b.c");
    }

    #[test]
    fn get_s_missing_key_names_the_field() {
        let item = HashMap::new();
        assert_missing_field(get_s(&item, "email").unwrap_err(), "email");
    }

    #[test]
    fn get_s_wrong_attribute_kind_is_missing_field() {
        // An N where an S was expected must surface as MissingField,
        // not silently coerce — a row written with the wrong AV kind
        // is corrupt and should fail loud.
        let item = item_with("email", AttributeValue::N("42".to_string()));
        assert_missing_field(get_s(&item, "email").unwrap_err(), "email");
    }

    #[test]
    fn get_n_parses_positive_and_negative() {
        let item = item_with("ts", AttributeValue::N("1700000000000000".to_string()));
        assert_eq!(get_n(&item, "ts").unwrap(), 1_700_000_000_000_000);
        let item = item_with("ts", AttributeValue::N("-7".to_string()));
        assert_eq!(get_n(&item, "ts").unwrap(), -7);
    }

    #[test]
    fn get_n_missing_key_names_the_field() {
        let item = HashMap::new();
        assert_missing_field(get_n(&item, "created_at").unwrap_err(), "created_at");
    }

    #[test]
    fn get_n_rejects_string_attribute_and_unparseable_number() {
        // S-typed value where N expected.
        let item = item_with("ts", AttributeValue::S("42".to_string()));
        assert_missing_field(get_n(&item, "ts").unwrap_err(), "ts");
        // N attribute whose payload isn't an i64 (DDB allows decimals).
        let item = item_with("ts", AttributeValue::N("1.5".to_string()));
        assert_missing_field(get_n(&item, "ts").unwrap_err(), "ts");
        // Overflow past i64::MAX.
        let item = item_with("ts", AttributeValue::N("9223372036854775808".to_string()));
        assert_missing_field(get_n(&item, "ts").unwrap_err(), "ts");
    }

    #[test]
    fn get_n_u64_parses_full_range() {
        let item = item_with("v", AttributeValue::N("0".to_string()));
        assert_eq!(get_n_u64(&item, "v").unwrap(), 0);
        let item = item_with("v", AttributeValue::N(u64::MAX.to_string()));
        assert_eq!(get_n_u64(&item, "v").unwrap(), u64::MAX);
    }

    #[test]
    fn get_n_u64_rejects_negative() {
        // A negative value in a u64 slot (e.g. snapshot_version) is a
        // corrupt row; it must not wrap or default.
        let item = item_with("v", AttributeValue::N("-1".to_string()));
        assert_missing_field(get_n_u64(&item, "v").unwrap_err(), "v");
    }
}
