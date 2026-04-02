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
