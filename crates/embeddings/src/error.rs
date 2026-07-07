// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("bedrock error: {0}")]
    Bedrock(String),

    #[error("qdrant error: {0}")]
    Qdrant(String),

    #[error("serialization error: {0}")]
    Serialization(String),
}
