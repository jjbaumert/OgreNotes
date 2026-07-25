// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use serde::{Deserialize, Serialize};

/// Durable state of a Quip import run.
/// PK: IMPORT#<import_id>, SK: META
///
/// Deliberately has **no token field** — the Quip OAuth/API token for an
/// in-progress import lives only in the `TokenStore` (see Phase 0 Task 4),
/// which is a separate, more tightly-scoped secret store. This record is
/// the durable manifest an operator or the import worker can inspect
/// (status/phase/owner/scope); it must never be a place a leaked DynamoDB
/// read exposes a live credential.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportRecord {
    pub import_id: String,
    pub owner_id: String,
    pub status: ImportStatus,
    pub phase: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quip_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_folder_id: Option<String>,
    /// Root Quip folder/thread IDs the user scoped the import to. Stored
    /// sparsely (omitted when empty); legacy/pre-scoping rows decode to
    /// empty (import everything).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_roots: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ImportRecord {
    pub fn pk(&self) -> String {
        format!("IMPORT#{}", self.import_id)
    }

    pub fn sk() -> &'static str {
        "META"
    }
}

/// Lifecycle status of an import run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportStatus {
    /// User is picking Quip roots/target folder; nothing has run yet.
    Scoping,
    /// The import worker is actively fetching/converting content.
    Running,
    /// Paused pending the user confirming their Quip identity matches
    /// the OgreNotes account (e.g. ambiguous email match).
    AwaitingIdentityConfirm,
    /// The stored Quip token was rejected (expired/revoked) — the user
    /// must reconnect before the import can resume.
    TokenRejected,
    Succeeded,
    Failed,
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_common::id::new_id;
    use ogrenotes_common::time::now_usec;

    fn sample_record() -> ImportRecord {
        let now = now_usec();
        ImportRecord {
            import_id: new_id(),
            owner_id: new_id(),
            status: ImportStatus::Scoping,
            phase: 0,
            quip_user_id: Some("quip-user-1".to_string()),
            target_folder_id: Some("folder-1".to_string()),
            selected_roots: vec!["root-a".to_string(), "root-b".to_string()],
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn import_record_pk_sk_format() {
        let record = sample_record();
        assert_eq!(record.pk(), format!("IMPORT#{}", record.import_id));
        assert_eq!(ImportRecord::sk(), "META");
    }

    #[test]
    fn import_record_json_roundtrip() {
        let record = sample_record();
        let json = serde_json::to_string(&record).unwrap();
        let back: ImportRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn import_status_serializes_lowercase_no_separator() {
        // Matches the crate convention for multi-word `lowercase` enums
        // (e.g. `NotifEmailPref::MentionsOnly` -> "mentionsonly" in
        // models/mod.rs): squashed lowercase, no hyphen/underscore.
        let token = |s: ImportStatus| {
            serde_json::to_string(&s).unwrap().trim_matches('"').to_string()
        };
        assert_eq!(token(ImportStatus::Scoping), "scoping");
        assert_eq!(token(ImportStatus::Running), "running");
        assert_eq!(
            token(ImportStatus::AwaitingIdentityConfirm),
            "awaitingidentityconfirm"
        );
        assert_eq!(token(ImportStatus::TokenRejected), "tokenrejected");
        assert_eq!(token(ImportStatus::Succeeded), "succeeded");
        assert_eq!(token(ImportStatus::Failed), "failed");
        assert_eq!(token(ImportStatus::Cancelled), "cancelled");
    }

    // Compile-enforced contract: `ImportRecord` has no token/secret field.
    // This test exists to make that contract visible/searchable in the
    // test suite, not to catch a runtime bug — if a `token` field were
    // added to the struct, this file (and every other exhaustive
    // constructor of `ImportRecord` in this crate) would fail to compile
    // until it was accounted for.
    #[test]
    fn import_record_has_no_token_field() {
        let ImportRecord {
            import_id: _,
            owner_id: _,
            status: _,
            phase: _,
            quip_user_id: _,
            target_folder_id: _,
            selected_roots: _,
            created_at: _,
            updated_at: _,
        } = sample_record();
    }
}
