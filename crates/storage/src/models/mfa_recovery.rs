// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Single-use TOTP recovery codes (Phase 4 M-E3).
//!
//! Each enrollment mints `RECOVERY_CODE_COUNT` codes; each row stores
//! the bcrypt hash (never the plaintext). On a successful redemption,
//! the row is deleted so re-use is structurally impossible. On
//! re-enroll, all rows for the user are first deleted; otherwise
//! stale codes from an earlier enrollment would still verify against
//! the new secret.
//!
//! DynamoDB key pattern:
//!   PK = `USER#<user_id>`
//!   SK = `MFA_RECOVERY#<idx:02>`   (idx is 0..9; zero-pad keeps SK
//!                                   ordering stable for full-list scans)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MfaRecoveryCode {
    pub user_id: String,
    /// Stable index 0..9. Used in the SK; not exposed to the user.
    pub idx: u8,
    /// bcrypt hash of the original code. The plaintext is shown to
    /// the user exactly once at enroll-time and is otherwise
    /// unrecoverable.
    pub bcrypt_hash: String,
    pub created_at: i64,
}

impl MfaRecoveryCode {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk(&self) -> String {
        format!("MFA_RECOVERY#{:02}", self.idx)
    }

    pub fn sk_prefix() -> &'static str {
        "MFA_RECOVERY#"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(idx: u8) -> MfaRecoveryCode {
        MfaRecoveryCode {
            user_id: "alice".to_string(),
            idx,
            bcrypt_hash: "$2b$10$placeholder".to_string(),
            created_at: 0,
        }
    }

    #[test]
    fn sk_zero_pads_so_list_ordering_is_stable() {
        // Without the 02-wide pad, idx 9 sorts after idx 10 under
        // lexicographic compare. The list endpoint enumerates all 10
        // rows in idx order so the unverified-yet-displayed admin
        // page can show "code #1, code #2 …" cleanly.
        let a = fixture(9);
        let b = fixture(10);
        assert!(a.sk() < b.sk(), "single-digit must sort before double-digit");
    }

    #[test]
    fn pk_targets_owning_user() {
        assert_eq!(fixture(0).pk(), "USER#alice");
    }
}
