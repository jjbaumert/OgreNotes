// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Comment thread and message models.
//!
//! DynamoDB key patterns:
//! - Thread metadata: PK=`THREAD#<thread_id>`, SK=`METADATA`
//! - Messages:        PK=`THREAD#<thread_id>`, SK=`MSG#<timestamp>`

use serde::{Deserialize, Serialize};

/// Type of thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ThreadType {
    /// Inline comment anchored to a text selection.
    Inline,
    /// Document-level comment (conversation pane).
    Document,
    /// Group chat room.
    Chat,
    /// 1:1 direct message.
    #[serde(alias = "directmessage")]
    DirectMessage,
}

/// Status of a comment thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    Open,
    Resolved,
}

/// A comment thread attached to a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub thread_id: String,
    pub doc_id: String,
    pub thread_type: ThreadType,
    pub status: ThreadStatus,
    pub created_by: String,
    /// Chat room title (None for DMs and comments).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Participant user IDs (for chat/DM threads).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_ids: Vec<String>,
    /// For inline comments: the block ID this comment is anchored to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    /// For inline comments: start position in the document (legacy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_start: Option<u32>,
    /// For inline comments: end position in the document (legacy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_end: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Thread {
    pub fn pk(&self) -> String {
        format!("THREAD#{}", self.thread_id)
    }

    pub fn sk(&self) -> String {
        "METADATA".to_string()
    }
}

/// Style for a message part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PartStyle {
    Body,
    System,
    Monospace,
    Status,
}

/// A styled segment within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePart {
    pub style: PartStyle,
    pub text: String,
}

/// Type of mention target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MentionType {
    Person,
    Document,
    Chat,
}

/// An @mention within a message.
///
/// Serialized with camelCase field names so the API surface uses
/// `mentionType` rather than `mention_type`, matching the rest of the API's
/// camelCase convention. The model has never been populated with real
/// mentions prior to M3, so there is no stored `mention_type` JSON to be
/// backward-compatible with.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mention {
    pub mention_type: MentionType,
    pub id: String,
    pub label: String,
}

/// Emoji reaction on a message.
/// PK: THREAD#{thread_id}, SK: REACTION#{message_id}#{emoji}
#[derive(Debug, Clone)]
pub struct Reaction {
    pub thread_id: String,
    pub message_id: String,
    pub emoji: String,
    pub user_ids: Vec<String>,
}

impl Reaction {
    pub fn pk(&self) -> String {
        format!("THREAD#{}", self.thread_id)
    }
    pub fn sk(&self) -> String {
        format!("REACTION#{}#{}", self.message_id, self.emoji)
    }
}

/// Read receipt for a thread.
/// PK: THREAD#{thread_id}, SK: READ#{user_id}
#[derive(Debug, Clone)]
pub struct ReadReceipt {
    pub thread_id: String,
    pub user_id: String,
    pub last_read_at: i64,
}

impl ReadReceipt {
    pub fn pk(&self) -> String {
        format!("THREAD#{}", self.thread_id)
    }
    pub fn sk(&self) -> String {
        format!("READ#{}", self.user_id)
    }
}

/// A message within a comment thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub thread_id: String,
    pub message_id: String,
    pub user_id: String,
    pub content: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<MessagePart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<Mention>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
}

impl Message {
    pub fn pk(&self) -> String {
        format!("THREAD#{}", self.thread_id)
    }

    pub fn sk(&self) -> String {
        format!("MSG#{:020}#{}", self.created_at, self.message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_pk_format() {
        let thread = Thread {
            thread_id: "abc123".to_string(),
            doc_id: "doc1".to_string(),
            thread_type: ThreadType::Document,
            status: ThreadStatus::Open,
            created_by: "user1".to_string(),
            title: None,
            member_ids: Vec::new(),
            block_id: None,
            anchor_start: None,
            anchor_end: None,
            created_at: 1000000,
            updated_at: 1000000,
        };
        assert_eq!(thread.pk(), "THREAD#abc123");
        assert_eq!(thread.sk(), "METADATA");
    }

    #[test]
    fn message_sk_zero_padded() {
        let msg = Message {
            thread_id: "t1".to_string(),
            message_id: "m1".to_string(),
            user_id: "u1".to_string(),
            content: "hello".to_string(),
            created_at: 42,
            updated_at: None,
            parts: Vec::new(),
            mentions: Vec::new(),
            attachments: Vec::new(),
        };
        assert_eq!(msg.pk(), "THREAD#t1");
        // Zero-padded to 20 digits
        assert_eq!(msg.sk(), "MSG#00000000000000000042#m1");
    }

    #[test]
    fn message_sk_ordering() {
        // Verify lexicographic ordering matches numeric ordering
        let sk1 = Message {
            thread_id: "t".to_string(),
            message_id: "a".to_string(),
            user_id: "u".to_string(),
            content: "".to_string(),
            created_at: 100,
            updated_at: None,
            parts: Vec::new(),
            mentions: Vec::new(),
            attachments: Vec::new(),
        }
        .sk();
        let sk2 = Message {
            thread_id: "t".to_string(),
            message_id: "b".to_string(),
            user_id: "u".to_string(),
            content: "".to_string(),
            created_at: 1000,
            updated_at: None,
            parts: Vec::new(),
            mentions: Vec::new(),
            attachments: Vec::new(),
        }
        .sk();
        assert!(sk1 < sk2, "Earlier timestamp should sort first: {sk1} vs {sk2}");
    }

    #[test]
    fn thread_type_serialization() {
        assert_eq!(
            serde_json::to_string(&ThreadType::Chat).unwrap(),
            "\"chat\""
        );
        assert_eq!(
            serde_json::to_string(&ThreadType::DirectMessage).unwrap(),
            "\"directMessage\""
        );
        assert_eq!(
            serde_json::to_string(&ThreadType::Inline).unwrap(),
            "\"inline\""
        );
    }

    #[test]
    fn thread_type_deserialization() {
        let t: ThreadType = serde_json::from_str("\"chat\"").unwrap();
        assert_eq!(t, ThreadType::Chat);
        let t: ThreadType = serde_json::from_str("\"directMessage\"").unwrap();
        assert_eq!(t, ThreadType::DirectMessage);
    }

    #[test]
    fn thread_type_legacy_lowercase_deserialization() {
        // Backward compat: old "directmessage" format should still work via alias
        let t: ThreadType = serde_json::from_str("\"directmessage\"").unwrap();
        assert_eq!(t, ThreadType::DirectMessage);
    }

    #[test]
    fn reaction_pk_sk_format() {
        // SK embeds both the message and the emoji so one row exists
        // per (message, emoji) pair with a user-id set inside.
        let r = Reaction {
            thread_id: "t1".to_string(),
            message_id: "m1".to_string(),
            emoji: "👍".to_string(),
            user_ids: vec!["u1".to_string()],
        };
        assert_eq!(r.pk(), "THREAD#t1");
        assert_eq!(r.sk(), "REACTION#m1#👍");
    }

    #[test]
    fn read_receipt_pk_sk_format() {
        let rr = ReadReceipt {
            thread_id: "t1".to_string(),
            user_id: "u1".to_string(),
            last_read_at: 0,
        };
        assert_eq!(rr.pk(), "THREAD#t1");
        assert_eq!(rr.sk(), "READ#u1");
    }

    #[test]
    fn thread_status_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&ThreadStatus::Open).unwrap(), "\"open\"");
        assert_eq!(
            serde_json::to_string(&ThreadStatus::Resolved).unwrap(),
            "\"resolved\""
        );
    }

    #[test]
    fn mention_uses_camel_case_on_the_wire() {
        // The API surface promises `mentionType`, not `mention_type`
        // (see the type's doc comment).
        let m = Mention {
            mention_type: MentionType::Document,
            id: "doc1".to_string(),
            label: "Spec".to_string(),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"mentionType\":\"document\""), "got {json}");
        assert!(!json.contains("mention_type"), "snake_case leaked: {json}");
        let back: Mention = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mention_type, MentionType::Document);
        assert_eq!(back.id, "doc1");
    }

    #[test]
    fn part_style_serializes_lowercase() {
        // MessagePart blobs are stored as JSON strings on the MSG#
        // row; the lowercase tags are the stored wire shape.
        for (style, tag) in [
            (PartStyle::Body, "\"body\""),
            (PartStyle::System, "\"system\""),
            (PartStyle::Monospace, "\"monospace\""),
            (PartStyle::Status, "\"status\""),
        ] {
            assert_eq!(serde_json::to_string(&style).unwrap(), tag);
        }
    }
}
