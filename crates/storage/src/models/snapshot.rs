//! Document snapshot model for edit history.
//!
//! DynamoDB key pattern:
//! PK = `DOC#<doc_id>`, SK = `SNAPSHOT#<version>`

use serde::{Deserialize, Serialize};

/// A point-in-time snapshot of a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocSnapshot {
    pub doc_id: String,
    pub version: u64,
    pub s3_key: String,
    pub size_bytes: u64,
    pub user_id: String,
    pub created_at: i64,
}

impl DocSnapshot {
    pub fn pk(&self) -> String {
        format!("DOC#{}", self.doc_id)
    }

    pub fn sk(&self) -> String {
        format!("SNAPSHOT#{:020}", self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_pk_sk_format() {
        let s = DocSnapshot {
            doc_id: "doc1".to_string(),
            version: 5,
            s3_key: "docs/doc1/snapshots/5.bin".to_string(),
            size_bytes: 1024,
            user_id: "user1".to_string(),
            created_at: 1000000,
        };
        assert_eq!(s.pk(), "DOC#doc1");
        assert_eq!(s.sk(), "SNAPSHOT#00000000000000000005");
    }

    #[test]
    fn snapshot_sk_ordering() {
        let s1 = DocSnapshot {
            doc_id: "d".to_string(),
            version: 1,
            s3_key: "".to_string(),
            size_bytes: 0,
            user_id: "u".to_string(),
            created_at: 0,
        };
        let s2 = DocSnapshot {
            doc_id: "d".to_string(),
            version: 100,
            s3_key: "".to_string(),
            size_bytes: 0,
            user_id: "u".to_string(),
            created_at: 0,
        };
        assert!(s1.sk() < s2.sk(), "Lower version should sort first");
    }
}
