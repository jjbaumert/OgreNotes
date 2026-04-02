use serde::{Deserialize, Serialize};

use super::DocType;

/// Document metadata stored in DynamoDB.
/// PK: DOC#<doc_id>, SK: METADATA
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentMeta {
    pub doc_id: String,
    pub title: String,
    pub owner_id: String,
    /// The folder this document belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    pub doc_type: DocType,
    pub snapshot_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_s3_key: Option<String>,
    pub is_deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl DocumentMeta {
    pub fn pk(&self) -> String {
        format!("DOC#{}", self.doc_id)
    }

    pub fn sk() -> &'static str {
        "METADATA"
    }

    /// S3 key for the current snapshot.
    pub fn snapshot_key(&self) -> String {
        format!("docs/{}/snapshots/{}.bin", self.doc_id, self.snapshot_version)
    }
}

/// CRDT update log entry.
/// PK: DOC#<doc_id>, SK: UPDATE#<clock>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocUpdate {
    pub doc_id: String,
    pub clock: String,
    #[serde(with = "serde_bytes")]
    pub update_bytes: Vec<u8>,
    pub user_id: String,
    pub created_at: i64,
}

impl DocUpdate {
    pub fn pk(&self) -> String {
        format!("DOC#{}", self.doc_id)
    }

    pub fn sk(&self) -> String {
        format!("UPDATE#{}", self.clock)
    }
}

mod serde_bytes {
    use serde::{self, Deserialize, Deserializer, Serializer};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_common::id::new_id;
    use ogrenotes_common::time::now_usec;

    fn sample_doc() -> DocumentMeta {
        let now = now_usec();
        DocumentMeta {
            doc_id: new_id(),
            title: "Test Document".to_string(),
            owner_id: new_id(),
            folder_id: None,
            doc_type: DocType::Document,
            snapshot_version: 1,
            snapshot_s3_key: Some("docs/abc/snapshots/1.bin".to_string()),
            is_deleted: false,
            deleted_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn document_pk_format() {
        let doc = sample_doc();
        assert_eq!(doc.pk(), format!("DOC#{}", doc.doc_id));
    }

    #[test]
    fn document_sk_format() {
        assert_eq!(DocumentMeta::sk(), "METADATA");
    }

    #[test]
    fn document_snapshot_key() {
        let mut doc = sample_doc();
        doc.doc_id = "abc123".to_string();
        doc.snapshot_version = 5;
        assert_eq!(doc.snapshot_key(), "docs/abc123/snapshots/5.bin");
    }

    #[test]
    fn document_json_roundtrip() {
        let doc = sample_doc();
        let json = serde_json::to_string(&doc).unwrap();
        let back: DocumentMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn document_soft_delete_fields() {
        let mut doc = sample_doc();
        doc.is_deleted = true;
        doc.deleted_at = Some(now_usec());
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"is_deleted\":true"));
        assert!(json.contains("deleted_at"));
    }
}
