// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

/// Errors shared across crates.
#[derive(Debug, thiserror::Error)]
pub enum CommonError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // The Display strings below are contract text: edge crates map
    // CommonError variants into HTTP responses and log lines, so a
    // wording change is an observable behavior change. Pin each one.

    #[test]
    fn display_not_found_includes_subject() {
        let e = CommonError::NotFound("doc-123".to_string());
        assert_eq!(e.to_string(), "not found: doc-123");
    }

    #[test]
    fn display_unauthorized_is_bare() {
        assert_eq!(CommonError::Unauthorized.to_string(), "unauthorized");
    }

    #[test]
    fn display_forbidden_is_bare() {
        assert_eq!(CommonError::Forbidden.to_string(), "forbidden");
    }

    #[test]
    fn display_invalid_input_includes_detail() {
        let e = CommonError::InvalidInput("title too long".to_string());
        assert_eq!(e.to_string(), "invalid input: title too long");
    }

    #[test]
    fn display_conflict_includes_detail() {
        let e = CommonError::Conflict("version mismatch".to_string());
        assert_eq!(e.to_string(), "conflict: version mismatch");
    }

    #[test]
    fn display_internal_includes_detail() {
        let e = CommonError::Internal("dynamo write failed".to_string());
        assert_eq!(e.to_string(), "internal error: dynamo write failed");
    }

    #[test]
    fn implements_std_error_with_no_source() {
        // thiserror derives std::error::Error; none of the variants wrap a
        // source error, so source() must be None (callers relying on a chain
        // would silently lose context if a wrapped variant were added without
        // #[source]).
        let e: &dyn std::error::Error = &CommonError::Unauthorized;
        assert!(e.source().is_none());
    }
}
