// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Notification model.
//!
//! DynamoDB key pattern:
//! PK = `USER#<user_id>`, SK = `NOTIF#<timestamp>#<notif_id>`

use serde::{Deserialize, Serialize};

use super::NotifLevel;

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
    /// Someone opened a document the user shared.
    #[serde(alias = "documentopened")]
    DocumentOpened,
    /// Someone reached a document via its view-only link and requested
    /// edit access; the document owner is notified.
    #[serde(alias = "requestaccess")]
    RequestAccess,
}

/// Per-thread notification preference.
/// PK: USER#{user_id}, SK: NOTIF_PREF#{thread_id}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifPref {
    pub user_id: String,
    pub thread_id: String,
    pub level: NotifLevel,
}

impl NotifPref {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk(&self) -> String {
        format!("NOTIF_PREF#{}", self.thread_id)
    }
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
    /// Human-readable summary (the action, e.g. "replied to your comment").
    pub message: String,
    /// Truncated preview of the comment/reply text that triggered this
    /// notification, so the recipient can tell threads apart at a glance.
    /// `None` for notification types that carry no body text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Anchor block of the related comment thread (document `data-block-id`
    /// or a `cell-…` spreadsheet cell id). Lets the client deep-link to the
    /// exact block/cell and open its comment dialog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
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
            preview: None,
            block_id: None,
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
            preview: None,
            block_id: None,
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
            preview: None,
            block_id: None,
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
            preview: None,
            block_id: None,
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
        assert_eq!(serde_json::to_string(&NotifType::DocumentOpened).unwrap(), "\"documentOpened\"");
    }

    #[test]
    fn notif_type_deserialization() {
        let t: NotifType = serde_json::from_str("\"chatMessage\"").unwrap();
        assert_eq!(t, NotifType::ChatMessage);
        let t: NotifType = serde_json::from_str("\"documentEdited\"").unwrap();
        assert_eq!(t, NotifType::DocumentEdited);
        let t: NotifType = serde_json::from_str("\"documentOpened\"").unwrap();
        assert_eq!(t, NotifType::DocumentOpened);
    }

    #[test]
    fn notif_type_legacy_lowercase_deserialization() {
        // Backward compat via alias
        let t: NotifType = serde_json::from_str("\"chatmessage\"").unwrap();
        assert_eq!(t, NotifType::ChatMessage);
        let t: NotifType = serde_json::from_str("\"documentedited\"").unwrap();
        assert_eq!(t, NotifType::DocumentEdited);
        let t: NotifType = serde_json::from_str("\"documentopened\"").unwrap();
        assert_eq!(t, NotifType::DocumentOpened);
    }

    #[test]
    fn notif_pref_pk_sk_format() {
        // "NOTIF_PREF#…" shares only "NOTIF" with the "NOTIF#" query
        // prefix — the sixth character ('_' vs '#') keeps pref rows
        // out of every begins_with(SK, "NOTIF#") query. Load-bearing
        // for delete_all/mark_all_read: preferences must survive a
        // clear-all of the notification list.
        let p = NotifPref {
            user_id: "u1".to_string(),
            thread_id: "t1".to_string(),
            level: NotifLevel::Mute,
        };
        assert_eq!(p.pk(), "USER#u1");
        assert_eq!(p.sk(), "NOTIF_PREF#t1");
        assert!(
            !p.sk().starts_with("NOTIF#"),
            "a NOTIF_PREF row must not match the NOTIF# query prefix"
        );
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn notif_at(created_at: i64, id: &str) -> Notification {
        Notification {
            notif_id: id.to_string(),
            user_id: "u".to_string(),
            notif_type: NotifType::Shared,
            doc_id: None,
            thread_id: None,
            actor_id: "a".to_string(),
            message: String::new(),
            preview: None,
            block_id: None,
            read: false,
            created_at,
        }
    }

    proptest! {
        /// The 20-digit zero-pad makes lexicographic SK order equal
        /// numeric timestamp order for every representable non-negative
        /// timestamp (i64::MAX < 10^20, so the pad never overflows).
        /// Newest-first listing sorts on this — a violation reorders a
        /// user's notification feed.
        #[test]
        fn sk_lexicographic_order_matches_timestamp_order(
            a in 0..i64::MAX,
            b in 0..i64::MAX,
        ) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assume!(lo != hi);
            // Fix the id so ordering is decided purely by timestamp.
            prop_assert!(notif_at(lo, "x").sk() < notif_at(hi, "x").sk());
        }
    }
}
