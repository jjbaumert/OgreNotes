// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::snapshot::DocSnapshot;
use crate::repo::{get_n, get_n_u64, get_s, RepoError};

pub struct SnapshotRepo {
    db: DynamoClient,
}

impl SnapshotRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Write a snapshot entry to DynamoDB.
    pub async fn create(&self, snapshot: &DocSnapshot) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(snapshot.pk()));
        item.insert("SK".to_string(), AttributeValue::S(snapshot.sk()));
        item.insert("doc_id".to_string(), AttributeValue::S(snapshot.doc_id.clone()));
        item.insert("version".to_string(), AttributeValue::N(snapshot.version.to_string()));
        item.insert("s3_key".to_string(), AttributeValue::S(snapshot.s3_key.clone()));
        item.insert("size_bytes".to_string(), AttributeValue::N(snapshot.size_bytes.to_string()));
        item.insert("user_id".to_string(), AttributeValue::S(snapshot.user_id.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(snapshot.created_at.to_string()));

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List all snapshots for a document, ordered by version (ascending).
    pub async fn list(&self, doc_id: &str) -> Result<Vec<DocSnapshot>, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let items = self
            .db
            .query(&pk, Some("SNAPSHOT#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items.iter().map(|item| snapshot_from_item(item)).collect()
    }

    /// Get a specific snapshot by version number.
    pub async fn get(&self, doc_id: &str, version: u64) -> Result<Option<DocSnapshot>, RepoError> {
        let pk = format!("DOC#{doc_id}");
        let sk = format!("SNAPSHOT#{:020}", version);
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        match item {
            Some(item) => Ok(Some(snapshot_from_item(&item)?)),
            None => Ok(None),
        }
    }
}

fn snapshot_from_item(item: &HashMap<String, AttributeValue>) -> Result<DocSnapshot, RepoError> {
    Ok(DocSnapshot {
        doc_id: get_s(item, "doc_id")?,
        version: get_n_u64(item, "version")?,
        s3_key: get_s(item, "s3_key")?,
        size_bytes: get_n_u64(item, "size_bytes")?,
        user_id: get_s(item, "user_id")?,
        created_at: get_n(item, "created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> DocSnapshot {
        DocSnapshot {
            doc_id: "doc1".to_string(),
            version: 5,
            s3_key: "docs/doc1/snapshots/5.bin".to_string(),
            size_bytes: 1024,
            user_id: "u1".to_string(),
            created_at: 1_700_000_000_000_000,
        }
    }

    /// Mimic `create`'s column construction (no live table).
    fn item_for(snap: &DocSnapshot) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(snap.pk()));
        item.insert("SK".to_string(), AttributeValue::S(snap.sk()));
        item.insert("doc_id".to_string(), AttributeValue::S(snap.doc_id.clone()));
        item.insert("version".to_string(), AttributeValue::N(snap.version.to_string()));
        item.insert("s3_key".to_string(), AttributeValue::S(snap.s3_key.clone()));
        item.insert("size_bytes".to_string(), AttributeValue::N(snap.size_bytes.to_string()));
        item.insert("user_id".to_string(), AttributeValue::S(snap.user_id.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(snap.created_at.to_string()));
        item
    }

    #[test]
    fn from_item_round_trips_create_shape() {
        let snap = fixture();
        let back = snapshot_from_item(&item_for(&snap)).expect("from_item");
        assert_eq!(back.doc_id, snap.doc_id);
        assert_eq!(back.version, snap.version);
        assert_eq!(back.s3_key, snap.s3_key);
        assert_eq!(back.size_bytes, snap.size_bytes);
        assert_eq!(back.user_id, snap.user_id);
        assert_eq!(back.created_at, snap.created_at);
    }

    #[test]
    fn missing_s3_key_errors() {
        // The row's whole purpose is pointing at the blob; a row
        // without the pointer must fail decode, not come back with an
        // empty key that get_object would treat as a real path.
        let mut item = item_for(&fixture());
        item.remove("s3_key");
        match snapshot_from_item(&item) {
            Err(RepoError::MissingField(f)) => assert_eq!(f, "s3_key"),
            other => panic!("expected MissingField(s3_key), got {other:?}"),
        }
    }
}
