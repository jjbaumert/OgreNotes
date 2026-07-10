// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("bedrock error: {0}")]
    Bedrock(String),

    #[error("qdrant error: {0}")]
    Qdrant(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    /// The model returned a well-formed embedding of the wrong length
    /// (issue #15). Distinct from `Serialization` so triage can tell a
    /// model/config drift apart from a malformed response.
    #[error("embedding length {got} != configured dimensions {expected}")]
    DimensionMismatch { got: usize, expected: u32 },
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

    /// Issue #15: the mismatch message names both lengths so a log line
    /// alone is enough to spot a model/config drift.
    #[test]
    fn dimension_mismatch_display_names_both_lengths() {
        assert_eq!(
            EmbeddingError::DimensionMismatch { got: 3, expected: 512 }.to_string(),
            "embedding length 3 != configured dimensions 512"
        );
    }
}
