// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Activity feed model.
//!
//! DynamoDB key pattern:
//! PK = `DOC#<doc_id>`, SK = `ACTIVITY#<timestamp>#<activity_id>`

use serde::{Deserialize, Serialize};

/// Type of activity event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ActivityEventType {
    Edit,
    Comment,
    Share,
    Open,
    Restore,
    #[serde(alias = "resolveComment")]
    ResolveComment,
    /// Document was soft-deleted (moved to trash). Hard-deletes via
    /// `purge_document` and the future trash-cleanup worker do NOT
    /// emit an Activity row because `hard_delete` sweeps every row
    /// under PK=DOC#<doc_id>, including activity rows — the row
    /// would be orphaned-then-reaped in the same call. Hard-deletes
    /// are auditable via `SecurityAudit::DocDeleted { hard: true }`.
    Delete,
    /// Document was moved between folders. Detail carries
    /// `{ sourceFolderId?, destFolderId }`. Emitted by both the
    /// single-doc move path and the bulk-move endpoint (M-P7
    /// piece B).
    Move,
}

/// An activity event on a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub activity_id: String,
    pub doc_id: String,
    pub event_type: ActivityEventType,
    pub actor_id: String,
    /// Event-specific detail (JSON object).
    pub detail: String,
    pub created_at: i64,
}

impl Activity {
    pub fn pk(&self) -> String {
        format!("DOC#{}", self.doc_id)
    }

    pub fn sk(&self) -> String {
        format!("ACTIVITY#{:020}#{}", self.created_at, self.activity_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_pk_sk_format() {
        let a = Activity {
            activity_id: "act1".to_string(),
            doc_id: "doc1".to_string(),
            event_type: ActivityEventType::Edit,
            actor_id: "user1".to_string(),
            detail: "{}".to_string(),
            created_at: 42,
        };
        assert_eq!(a.pk(), "DOC#doc1");
        assert_eq!(a.sk(), "ACTIVITY#00000000000000000042#act1");
    }

    #[test]
    fn event_type_serialization() {
        assert_eq!(serde_json::to_string(&ActivityEventType::Edit).unwrap(), "\"edit\"");
        assert_eq!(serde_json::to_string(&ActivityEventType::Comment).unwrap(), "\"comment\"");
        assert_eq!(serde_json::to_string(&ActivityEventType::ResolveComment).unwrap(), "\"resolveComment\"");
        assert_eq!(serde_json::to_string(&ActivityEventType::Delete).unwrap(), "\"delete\"");
    }

    #[test]
    fn sk_ordering() {
        let sk1 = Activity {
            activity_id: "a".to_string(), doc_id: "d".to_string(),
            event_type: ActivityEventType::Edit, actor_id: "u".to_string(),
            detail: "{}".to_string(), created_at: 100,
        }.sk();
        let sk2 = Activity {
            activity_id: "b".to_string(), doc_id: "d".to_string(),
            event_type: ActivityEventType::Edit, actor_id: "u".to_string(),
            detail: "{}".to_string(), created_at: 200,
        }.sk();
        assert!(sk1 < sk2, "Earlier timestamp should sort first");
    }
}
