//! Notification model.
//!
//! DynamoDB key pattern:
//! PK = `USER#<user_id>`, SK = `NOTIF#<timestamp>#<notif_id>`

use serde::{Deserialize, Serialize};

/// Type of notification event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum NotifType {
    /// Someone shared a document/folder with the user.
    Shared,
    /// Someone mentioned the user in a comment or chat.
    Mentioned,
    /// Someone commented on a document the user owns/follows.
    Commented,
    /// Someone sent a chat message in a thread the user belongs to.
    #[serde(alias = "chatmessage")]
    ChatMessage,
    /// A document the user has access to was edited.
    #[serde(alias = "documentedited")]
    DocumentEdited,
}

/// A notification entry for a user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub notif_id: String,
    pub user_id: String,
    pub notif_type: NotifType,
    /// ID of the related document (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_id: Option<String>,
    /// ID of the related thread (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// User ID of the actor who triggered the notification.
    pub actor_id: String,
    /// Human-readable summary.
    pub message: String,
    /// Whether the user has read this notification.
    pub read: bool,
    pub created_at: i64,
}

impl Notification {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk(&self) -> String {
        format!("NOTIF#{:020}#{}", self.created_at, self.notif_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_pk_format() {
        let n = Notification {
            notif_id: "n1".to_string(),
            user_id: "user1".to_string(),
            notif_type: NotifType::Commented,
            doc_id: Some("doc1".to_string()),
            thread_id: None,
            actor_id: "actor1".to_string(),
            message: "someone commented".to_string(),
            read: false,
            created_at: 1234567890,
        };
        assert_eq!(n.pk(), "USER#user1");
        assert!(n.sk().starts_with("NOTIF#"));
    }

    #[test]
    fn notification_sk_zero_padded() {
        let n = Notification {
            notif_id: "x".to_string(),
            user_id: "u".to_string(),
            notif_type: NotifType::Shared,
            doc_id: None,
            thread_id: None,
            actor_id: "a".to_string(),
            message: "".to_string(),
            read: false,
            created_at: 42,
        };
        assert_eq!(n.sk(), "NOTIF#00000000000000000042#x");
    }

    #[test]
    fn notification_sk_ordering() {
        let sk1 = Notification {
            notif_id: "a".to_string(),
            user_id: "u".to_string(),
            notif_type: NotifType::Mentioned,
            doc_id: None,
            thread_id: None,
            actor_id: "a".to_string(),
            message: "".to_string(),
            read: false,
            created_at: 100,
        }
        .sk();
        let sk2 = Notification {
            notif_id: "b".to_string(),
            user_id: "u".to_string(),
            notif_type: NotifType::Mentioned,
            doc_id: None,
            thread_id: None,
            actor_id: "a".to_string(),
            message: "".to_string(),
            read: false,
            created_at: 200,
        }
        .sk();
        assert!(sk1 < sk2, "Earlier timestamp should sort first");
    }

    #[test]
    fn notif_type_serialization() {
        assert_eq!(serde_json::to_string(&NotifType::Shared).unwrap(), "\"shared\"");
        assert_eq!(serde_json::to_string(&NotifType::ChatMessage).unwrap(), "\"chatMessage\"");
        assert_eq!(serde_json::to_string(&NotifType::DocumentEdited).unwrap(), "\"documentEdited\"");
    }

    #[test]
    fn notif_type_deserialization() {
        let t: NotifType = serde_json::from_str("\"chatMessage\"").unwrap();
        assert_eq!(t, NotifType::ChatMessage);
        let t: NotifType = serde_json::from_str("\"documentEdited\"").unwrap();
        assert_eq!(t, NotifType::DocumentEdited);
    }

    #[test]
    fn notif_type_legacy_lowercase_deserialization() {
        // Backward compat via alias
        let t: NotifType = serde_json::from_str("\"chatmessage\"").unwrap();
        assert_eq!(t, NotifType::ChatMessage);
        let t: NotifType = serde_json::from_str("\"documentedited\"").unwrap();
        assert_eq!(t, NotifType::DocumentEdited);
    }
}
