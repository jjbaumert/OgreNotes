// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Admin audit-log model.
//!
//! Records privileged admin actions (disable/enable/promote/demote/
//! set-ask-enabled) for forensics and compliance. Separate from the
//! document `Activity` model because the audit row is keyed by the
//! *target user*, not a document.
//!
//! DynamoDB key pattern:
//!   PK = `USER#<target_user_id>`
//!   SK = `ADMIN_AUDIT#<created_at:020>#<audit_id>`
//!
//! The 20-digit zero-padded timestamp keeps SK ordering chronological
//! when listed via `scan_index_forward(false)`.

use serde::{Deserialize, Serialize};

/// The privileged action an admin took.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AdminAuditAction {
    Disable,
    Enable,
    Promote,
    Demote,
    /// #148 — three-state `ask_policy` flip. Replaces the
    /// pre-migration `SetAskEnabled` variant. Old audit rows
    /// still deserialize via the `serde(alias)` below.
    #[serde(alias = "setAskEnabled")]
    SetAskPolicy,
}

impl AdminAuditAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Enable => "enable",
            Self::Promote => "promote",
            Self::Demote => "demote",
            Self::SetAskPolicy => "setAskPolicy",
        }
    }
}

/// One record of an admin acting on a user account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminAudit {
    pub audit_id: String,
    pub target_user_id: String,
    pub actor_id: String,
    pub action: AdminAuditAction,
    /// Action-specific detail (JSON object). For SetAskEnabled this is
    /// `{"enabled": bool}`; otherwise it's `{}`.
    pub detail: String,
    pub created_at: i64,
}

impl AdminAudit {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.target_user_id)
    }

    pub fn sk(&self) -> String {
        format!("ADMIN_AUDIT#{:020}#{}", self.created_at, self.audit_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(action: AdminAuditAction, created_at: i64) -> AdminAudit {
        AdminAudit {
            audit_id: "aud1".to_string(),
            target_user_id: "victim".to_string(),
            actor_id: "admin".to_string(),
            action,
            detail: "{}".to_string(),
            created_at,
        }
    }

    #[test]
    fn pk_targets_the_user_being_acted_on() {
        assert_eq!(fixture(AdminAuditAction::Disable, 0).pk(), "USER#victim");
    }

    #[test]
    fn sk_zero_pads_for_chronological_ordering() {
        let a = fixture(AdminAuditAction::Disable, 100);
        let b = fixture(AdminAuditAction::Promote, 1_700_000_000_000_000);
        // Lexicographic comparison must match numeric ordering.
        assert!(a.sk() < b.sk(), "SK ordering must be chronological under string compare");
    }

    #[test]
    fn action_serializes_camel_case() {
        assert_eq!(serde_json::to_string(&AdminAuditAction::SetAskPolicy).unwrap(), "\"setAskPolicy\"");
        assert_eq!(serde_json::to_string(&AdminAuditAction::Disable).unwrap(), "\"disable\"");
    }

    #[test]
    fn action_deserialize_accepts_legacy_set_ask_enabled_alias() {
        // #148 — legacy audit rows written before the AskPolicy
        // migration serialized the action as "setAskEnabled".
        // The serde alias on `SetAskPolicy` accepts both the new
        // spelling and the old one for read compatibility.
        let a: AdminAuditAction = serde_json::from_str("\"setAskEnabled\"").unwrap();
        assert_eq!(a, AdminAuditAction::SetAskPolicy);
        let b: AdminAuditAction = serde_json::from_str("\"setAskPolicy\"").unwrap();
        assert_eq!(b, AdminAuditAction::SetAskPolicy);
    }
}
