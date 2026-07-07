// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use serde::{Deserialize, Serialize};

use super::WorkspaceRole;

/// Workspace metadata.
/// PK: WORKSPACE#{workspace_id}, SK: METADATA
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    pub name: String,
    pub owner_id: String,
    /// Phase 4 M-E3: when true, members of this workspace must
    /// complete MFA enrollment before they can mint a session. The
    /// login handler reads this for the user's default workspace
    /// and flags `mfa_enrollment_required` on the TokenResponse if
    /// the user hasn't yet enrolled. `#[serde(default)]` keeps
    /// pre-M-E3 rows readable (treated as `false` = not required).
    #[serde(default)]
    pub mfa_required: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Workspace {
    pub fn pk(&self) -> String {
        format!("WORKSPACE#{}", self.workspace_id)
    }

    pub fn sk() -> &'static str {
        "METADATA"
    }
}

/// Workspace membership.
/// PK: WORKSPACE#{workspace_id}, SK: MEMBER#{user_id}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMember {
    pub workspace_id: String,
    pub user_id: String,
    pub role: WorkspaceRole,
    pub joined_at: i64,
}

impl WorkspaceMember {
    pub fn pk(&self) -> String {
        format!("WORKSPACE#{}", self.workspace_id)
    }

    pub fn sk(&self) -> String {
        format!("MEMBER#{}", self.user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_pk_format() {
        let ws = Workspace {
            workspace_id: "ws-1".to_string(),
            name: "Acme".to_string(),
            owner_id: "u-1".to_string(),
            mfa_required: false,
            created_at: 0,
            updated_at: 0,
        };
        assert_eq!(ws.pk(), "WORKSPACE#ws-1");
        assert_eq!(Workspace::sk(), "METADATA");
    }

    #[test]
    fn workspace_mfa_required_defaults_false_on_legacy_rows() {
        // Pre-M-E3 rows have no `mfa_required` field. Decode must
        // treat them as `false` (not required) — otherwise every
        // legacy workspace would silently turn into an MFA-required
        // one after the deploy.
        let legacy_json = serde_json::json!({
            "workspace_id": "ws-legacy",
            "name": "Acme",
            "owner_id": "u-1",
            "created_at": 0,
            "updated_at": 0,
        });
        let ws: Workspace = serde_json::from_value(legacy_json).expect("decode legacy row");
        assert!(!ws.mfa_required);
    }

    #[test]
    fn member_pk_sk_format() {
        let m = WorkspaceMember {
            workspace_id: "ws-1".to_string(),
            user_id: "u-2".to_string(),
            role: WorkspaceRole::Member,
            joined_at: 0,
        };
        assert_eq!(m.pk(), "WORKSPACE#ws-1");
        assert_eq!(m.sk(), "MEMBER#u-2");
    }
}
