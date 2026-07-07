// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

pub mod bedrock;
pub mod chunker;
pub mod error;
pub mod qdrant;

pub use error::EmbeddingError;
pub use qdrant::{PointMetadata, VectorFilter, VectorHit, VectorStore};

use crate::bedrock::BedrockEmbedder;
use crate::chunker::ChunkerConfig;

/// High-level pipeline that chunks text, embeds it, and stores vectors.
///
/// This is the main entry point held by AppState. Indexing hooks call
/// `index_document` and `delete_document`; the search endpoint calls `search`.
pub struct EmbeddingPipeline {
    embedder: BedrockEmbedder,
    store: VectorStore,
    chunker_config: ChunkerConfig,
}

impl EmbeddingPipeline {
    pub fn new(embedder: BedrockEmbedder, store: VectorStore) -> Self {
        Self {
            embedder,
            store,
            chunker_config: ChunkerConfig::default(),
        }
    }

    /// Index a document: chunk the body, embed each chunk, store in Qdrant.
    pub async fn index_document(
        &self,
        doc_id: &str,
        title: &str,
        body: &str,
        metadata: PointMetadata,
    ) -> Result<(), EmbeddingError> {
        let chunks = chunker::chunk_document(title, body, &self.chunker_config);
        if chunks.is_empty() {
            // Empty doc — remove any stale vectors
            self.store.delete_document(doc_id).await?;
            return Ok(());
        }

        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let vectors = self.embedder.embed_many(&texts).await?;
        self.store
            .upsert_document(doc_id, vectors, metadata)
            .await
    }

    /// Remove a document's vectors from Qdrant.
    pub async fn delete_document(&self, doc_id: &str) -> Result<(), EmbeddingError> {
        self.store.delete_document(doc_id).await
    }

    /// Semantic search: embed the query text, then search Qdrant for similar vectors.
    /// Returns doc_ids with scores, de-duplicated by doc_id.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        filter: Option<VectorFilter>,
    ) -> Result<Vec<VectorHit>, EmbeddingError> {
        let query_vector = self.embedder.embed(query).await?;
        self.store.search(query_vector, limit, filter).await
    }
}
