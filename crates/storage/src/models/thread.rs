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
        }
        .sk();
        let sk2 = Message {
            thread_id: "t".to_string(),
            message_id: "b".to_string(),
            user_id: "u".to_string(),
            content: "".to_string(),
            created_at: 1000,
            updated_at: None,
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
}
