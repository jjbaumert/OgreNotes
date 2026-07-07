// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::collections::HashMap;

use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, DeletePointsBuilder, Distance, Filter, PointStruct,
    SearchPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder,
    value::Kind,
};
use qdrant_client::Qdrant;
use uuid::Uuid;

use crate::error::EmbeddingError;

/// Deterministic namespace UUID for generating point IDs.
const OGRE_NS: Uuid = Uuid::from_bytes([
    0x6f, 0x67, 0x72, 0x65, 0x6e, 0x6f, 0x74, 0x65,
    0x73, 0x2d, 0x65, 0x6d, 0x62, 0x65, 0x64, 0x73,
]);

/// Qdrant vector store client.
pub struct VectorStore {
    client: Qdrant,
    collection: String,
}

/// A result from vector similarity search.
pub struct VectorHit {
    pub doc_id: String,
    pub score: f32,
    pub title: String,
    pub doc_type: String,
    pub owner_id: String,
    pub updated_at: i64,
}

/// Metadata stored with each point in Qdrant.
pub struct PointMetadata {
    pub doc_id: String,
    pub owner_id: String,
    pub doc_type: String,
    pub folder_id: Option<String>,
    pub workspace_id: Option<String>,
    pub title: String,
    pub updated_at: i64,
}

/// Filters for vector search.
pub struct VectorFilter {
    pub doc_type: Option<String>,
    pub owner_id: Option<String>,
    pub folder_id: Option<String>,
}

impl VectorStore {
    /// Connect to Qdrant and ensure the collection exists.
    pub async fn new(
        url: &str,
        collection: &str,
        dimensions: u32,
    ) -> Result<Self, EmbeddingError> {
        let client = Qdrant::from_url(url)
            .build()
            .map_err(|e| EmbeddingError::Qdrant(e.to_string()))?;

        let exists = client
            .collection_exists(collection)
            .await
            .map_err(|e| EmbeddingError::Qdrant(e.to_string()))?;

        if !exists {
            client
                .create_collection(
                    CreateCollectionBuilder::new(collection)
                        .vectors_config(VectorParamsBuilder::new(
                            dimensions as u64,
                            Distance::Cosine,
                        )),
                )
                .await
                .map_err(|e| EmbeddingError::Qdrant(e.to_string()))?;
            tracing::info!(collection, "created Qdrant collection");
        }

        Ok(Self {
            client,
            collection: collection.to_string(),
        })
    }

    /// Upsert all chunk vectors for a document.
    /// Deletes existing points for this doc_id first, then inserts new ones.
    pub async fn upsert_document(
        &self,
        doc_id: &str,
        vectors: Vec<Vec<f32>>,
        metadata: PointMetadata,
    ) -> Result<(), EmbeddingError> {
        // Delete existing points for this doc
        self.delete_document(doc_id).await?;

        let mut points = Vec::with_capacity(vectors.len());
        for (i, vector) in vectors.into_iter().enumerate() {
            let point_id = Uuid::new_v5(&OGRE_NS, format!("{doc_id}:{i}").as_bytes());
            let mut payload = HashMap::new();
            payload.insert("doc_id".to_string(), qdrant_value_str(&metadata.doc_id));
            payload.insert("chunk_index".to_string(), qdrant_value_int(i as i64));
            payload.insert("owner_id".to_string(), qdrant_value_str(&metadata.owner_id));
            payload.insert("doc_type".to_string(), qdrant_value_str(&metadata.doc_type));
            payload.insert("title".to_string(), qdrant_value_str(&metadata.title));
            payload.insert(
                "updated_at".to_string(),
                qdrant_value_int(metadata.updated_at),
            );
            if let Some(ref fid) = metadata.folder_id {
                payload.insert("folder_id".to_string(), qdrant_value_str(fid));
            }
            if let Some(ref wid) = metadata.workspace_id {
                payload.insert("workspace_id".to_string(), qdrant_value_str(wid));
            }

            points.push(PointStruct::new(
                point_id.to_string(),
                vector,
                payload,
            ));
        }

        if !points.is_empty() {
            self.client
                .upsert_points(UpsertPointsBuilder::new(&self.collection, points))
                .await
                .map_err(|e| EmbeddingError::Qdrant(e.to_string()))?;
        }

        Ok(())
    }

    /// Delete all points belonging to a document.
    pub async fn delete_document(&self, doc_id: &str) -> Result<(), EmbeddingError> {
        let filter = Filter::must([Condition::matches("doc_id", doc_id.to_string())]);
        self.client
            .delete_points(
                DeletePointsBuilder::new(&self.collection).points(filter),
            )
            .await
            .map_err(|e| EmbeddingError::Qdrant(e.to_string()))?;
        Ok(())
    }

    /// Search for similar vectors. Returns results de-duplicated by doc_id
    /// (highest score per document).
    pub async fn search(
        &self,
        query_vector: Vec<f32>,
        limit: usize,
        filter: Option<VectorFilter>,
    ) -> Result<Vec<VectorHit>, EmbeddingError> {
        let mut builder = SearchPointsBuilder::new(&self.collection, query_vector, limit as u64)
            .with_payload(true);

        if let Some(ref f) = filter {
            let mut conditions = Vec::new();
            if let Some(ref dt) = f.doc_type {
                conditions.push(Condition::matches("doc_type", dt.clone()));
            }
            if let Some(ref oid) = f.owner_id {
                conditions.push(Condition::matches("owner_id", oid.clone()));
            }
            if let Some(ref fid) = f.folder_id {
                conditions.push(Condition::matches("folder_id", fid.clone()));
            }
            if !conditions.is_empty() {
                builder = builder.filter(Filter::must(conditions));
            }
        }

        let results = self
            .client
            .search_points(builder)
            .await
            .map_err(|e| EmbeddingError::Qdrant(e.to_string()))?;

        // De-duplicate by doc_id, keeping highest score
        let mut best: HashMap<String, VectorHit> = HashMap::new();
        for point in results.result {
            let payload = &point.payload;
            let doc_id = payload_str(payload, "doc_id");
            let score = point.score;

            if let Some(existing) = best.get(&doc_id) {
                if score <= existing.score {
                    continue;
                }
            }

            best.insert(
                doc_id.clone(),
                VectorHit {
                    doc_id,
                    score,
                    title: payload_str(payload, "title"),
                    doc_type: payload_str(payload, "doc_type"),
                    owner_id: payload_str(payload, "owner_id"),
                    updated_at: payload_int(payload, "updated_at"),
                },
            );
        }

        let mut hits: Vec<VectorHit> = best.into_values().collect();
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(hits)
    }
}

fn qdrant_value_str(s: &str) -> qdrant_client::qdrant::Value {
    qdrant_client::qdrant::Value {
        kind: Some(Kind::StringValue(s.to_string())),
    }
}

fn qdrant_value_int(n: i64) -> qdrant_client::qdrant::Value {
    qdrant_client::qdrant::Value {
        kind: Some(Kind::IntegerValue(n)),
    }
}

fn payload_str(
    payload: &HashMap<String, qdrant_client::qdrant::Value>,
    key: &str,
) -> String {
    payload
        .get(key)
        .and_then(|v| match &v.kind {
            Some(Kind::StringValue(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn payload_int(
    payload: &HashMap<String, qdrant_client::qdrant::Value>,
    key: &str,
) -> i64 {
    payload
        .get(key)
        .and_then(|v| match &v.kind {
            Some(Kind::IntegerValue(n)) => Some(*n),
            _ => None,
        })
        .unwrap_or(0)
}
