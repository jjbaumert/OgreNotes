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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_are_stable() {
        // These strings surface in API error responses and CloudWatch
        // logs; pin them so log-based triage queries keep matching.
        assert_eq!(
            EmbeddingError::Bedrock("throttled".into()).to_string(),
            "bedrock error: throttled"
        );
        assert_eq!(
            EmbeddingError::Qdrant("connection refused".into()).to_string(),
            "qdrant error: connection refused"
        );
        assert_eq!(
            EmbeddingError::Serialization("bad json".into()).to_string(),
            "serialization error: bad json"
        );
    }
}
