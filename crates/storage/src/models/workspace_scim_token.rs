// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Workspace-scoped SCIM 2.0 bearer-token rows (Phase 4 M-E5).
//!
//! A workspace admin issues one or more SCIM tokens that an IdP's
//! SCIM provisioning agent uses to call `/scim/v2/Users` and
//! `/scim/v2/Groups`. Stored per-workspace:
//!
//!   PK = `WORKSPACE#<workspace_id>`
//!   SK = `SCIM_TOKEN#<token_id>`
//!
//! ## Token format on the wire
//!
//! The token the admin copies is `<token_id>.<secret>` where:
//!   - `token_id` is a public 16-char nanoid that identifies the
//!     row (the SCIM extractor uses it to look up the DDB row in
//!     O(1) — no scan).
//!   - `secret` is 32 bytes of cryptographic randomness, base64-
//!     url-no-pad encoded (~43 chars). Only the bcrypt hash of the
//!     secret lives in DDB; we cannot recover the secret after
//!     issuance.
//!
//! Splitting the public identifier from the secret follows the same
//! pattern as GitHub PATs, Stripe API keys, etc. The alternative —
//! "secret only, bcrypt-match across all rows for that workspace"
//! — would force a Query+iterate on every request and break under
//! any non-trivial token count.
//!
//! ## Revocation
//!
//! Disabling a token sets `disabled_at` to a non-zero usec timestamp;
//! the row stays in DDB so audit logs that reference its `token_id`
//! still resolve. The extractor rejects any token with
//! `disabled_at > 0`.

use serde::{Deserialize, Serialize};

/// Length cap on the admin-visible label. Long enough for "Okta SCIM
/// connector for Acme" type labels; short enough to keep the row
/// modest in CloudWatch dumps.
pub const MAX_TOKEN_NAME_LEN: usize = 128;

/// Opaque wrapper around a bcrypt-hashed secret. Prevents the
/// "stored plaintext where a hash was expected" bug class — passing
/// a raw secret string at the `WorkspaceScimToken.secret_hash`
/// field is a compile error rather than a silent corruption.
///
/// `serde(transparent)` keeps the on-the-wire and on-disk shapes
/// identical to a plain `String`, so the JSON round-trip and
/// DynamoDB encoding are unaffected. The constructor does NOT
/// validate that the input is in bcrypt format — that's the
/// caller's responsibility (typically `bcrypt::hash` output). The
/// guarantee is structural, not cryptographic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct BcryptHash(String);

impl BcryptHash {
    /// Wrap a bcrypt hash produced by `bcrypt::hash(...)`.
    pub fn new(hash: String) -> Self {
        Self(hash)
    }

    /// Borrow the hash as a string slice — used by `bcrypt::verify`
    /// and by the repo's DynamoDB encode/decode.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceScimToken {
    pub workspace_id: String,
    /// 16-char nanoid. Public — surfaced in the admin UI and in
    /// every SCIM audit row so admins can correlate a request to
    /// the token that issued it.
    pub token_id: String,
    /// bcrypt hash of the secret portion. The plaintext is shown
    /// once at creation and never again. Typed `BcryptHash` so a
    /// caller cannot accidentally store the raw secret here.
    pub secret_hash: BcryptHash,
    /// Admin-set label. Free-text, capped at `MAX_TOKEN_NAME_LEN`.
    pub name: String,
    pub created_at: i64,
    /// Set on every successful SCIM request via the token. Lets the
    /// admin see "this token was last used 3 days ago" and judge
    /// whether to revoke unused entries.
    pub last_used_at: i64,
    /// `0` = active; non-zero = disabled at that usec. Once disabled
    /// the row is preserved so historical audit references resolve.
    pub disabled_at: i64,
}

impl WorkspaceScimToken {
    pub fn pk(&self) -> String {
        format!("WORKSPACE#{}", self.workspace_id)
    }

    pub fn sk(&self) -> String {
        Self::sk_for(&self.token_id)
    }

    pub fn sk_for(token_id: &str) -> String {
        format!("SCIM_TOKEN#{token_id}")
    }

    /// True iff the token is currently active. Used by the SCIM
    /// extractor to gate every request.
    pub fn is_active(&self) -> bool {
        self.disabled_at == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> WorkspaceScimToken {
        WorkspaceScimToken {
            workspace_id: "ws-1".to_string(),
            token_id: "tok-aaaa-bbbb-cccc".to_string(),
            secret_hash: BcryptHash::new("$2b$12$abcdef".to_string()),
            name: "Okta connector".to_string(),
            created_at: 1_700_000_000_000_000,
            last_used_at: 0,
            disabled_at: 0,
        }
    }

    #[test]
    fn pk_sk_format() {
        let t = fixture();
        assert_eq!(t.pk(), "WORKSPACE#ws-1");
        assert_eq!(t.sk(), "SCIM_TOKEN#tok-aaaa-bbbb-cccc");
        assert_eq!(
            WorkspaceScimToken::sk_for("xyz"),
            "SCIM_TOKEN#xyz"
        );
    }

    #[test]
    fn is_active_reflects_disabled_at() {
        let mut t = fixture();
        assert!(t.is_active(), "fresh token must be active");
        t.disabled_at = 1_700_000_000_000_000;
        assert!(!t.is_active(), "disabled token must NOT be active");
    }

    #[test]
    fn json_roundtrip() {
        let t = fixture();
        let json = serde_json::to_string(&t).unwrap();
        let back: WorkspaceScimToken = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }
}
