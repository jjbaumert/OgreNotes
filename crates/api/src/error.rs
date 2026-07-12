// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use ogrenotes_common::metrics::{counter, MetricKey};

/// API error type that maps to HTTP responses.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    /// 403 Forbidden with a custom user-visible message. Use when the
    /// generic "you don't have access to this resource" wording would
    /// hide actionable information (e.g. "admin must enable AI assistant
    /// for your account").
    #[error("forbidden: {0}")]
    ForbiddenMsg(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),

    /// 429 Too Many Requests with a `Retry-After` header. Used by the
    /// per-user and load-shedding quotas in `routes/ask.rs`. The
    /// retry_after value is the number of whole seconds until the
    /// caller's bucket window rolls.
    #[error("too many requests: {message}")]
    TooManyRequests {
        message: String,
        retry_after_secs: u64,
    },

    /// 503 Service Unavailable with a `Retry-After` header. Used by
    /// the global daily-cap circuit breaker in `routes/ask.rs`.
    /// Distinct from `ServiceUnavailable(String)` (which is for
    /// configuration-absent cases like a missing API key) because
    /// callers should see when a retry might succeed.
    #[error("service overloaded: {message}")]
    Overloaded {
        message: String,
        retry_after_secs: u64,
    },
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // The two retry-bearing variants need a Retry-After header, so
        // they bypass the simple status+body construction below and
        // build their own Response.
        if let ApiError::TooManyRequests {
            message,
            retry_after_secs,
        }
        | ApiError::Overloaded {
            message,
            retry_after_secs,
        } = &self
        {
            let (status, code) = match self {
                ApiError::TooManyRequests { .. } => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
                ApiError::Overloaded { .. } => (StatusCode::SERVICE_UNAVAILABLE, "overloaded"),
                _ => unreachable!(),
            };
            counter::inc(MetricKey::new("api.errors_total", &[("code", code)]));
            let retry_after = axum::http::HeaderValue::from_str(&retry_after_secs.to_string())
                .unwrap_or_else(|_| axum::http::HeaderValue::from_static("60"));
            return (
                status,
                [(axum::http::header::RETRY_AFTER, retry_after)],
                Json(ErrorBody {
                    error: code,
                    message: message.clone(),
                }),
            )
                .into_response();
        }

        let (status, error_code, message) = match &self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, "not_found", msg.clone()),
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Authentication required".to_string(),
            ),
            ApiError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "You don't have access to this resource".to_string(),
            ),
            ApiError::ForbiddenMsg(msg) => (StatusCode::FORBIDDEN, "forbidden", msg.clone()),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg.clone()),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, "conflict", msg.clone()),
            ApiError::Internal(msg) => {
                tracing::error!(event_type = "internal_error", error = %msg, "internal server error");
                counter::inc(MetricKey::new("api.errors.internal_total", &[]));
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Something went wrong".to_string(),
                )
            }
            ApiError::ServiceUnavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                msg.clone(),
            ),
            // Handled above.
            ApiError::TooManyRequests { .. } | ApiError::Overloaded { .. } => unreachable!(),
        };

        // Auth rejections (401 / 403) are client-driven and dominate in
        // volume at scale — expired JWTs, browsers probing cookies, unshared
        // links. Keep them out of `api.errors_total` so that metric stays a
        // real server-error signal; track them separately.
        match status.as_u16() {
            401 | 403 => {
                counter::inc(MetricKey::new(
                    "api.auth_rejected_total",
                    &[("code", error_code)],
                ));
            }
            _ => {
                counter::inc(MetricKey::new(
                    "api.errors_total",
                    &[("code", error_code)],
                ));
            }
        }

        (status, Json(ErrorBody { error: error_code, message })).into_response()
    }
}

impl From<ogrenotes_storage::repo::RepoError> for ApiError {
    fn from(err: ogrenotes_storage::repo::RepoError) -> Self {
        match &err {
            ogrenotes_storage::repo::RepoError::MissingField(_) => {
                ApiError::Internal(err.to_string())
            }
            ogrenotes_storage::repo::RepoError::Dynamo(msg) => {
                if msg.contains("ConditionalCheckFailed") {
                    ApiError::Conflict("Resource already exists".to_string())
                } else {
                    ApiError::Internal(err.to_string())
                }
            }
            ogrenotes_storage::repo::RepoError::S3(_) => ApiError::Internal(err.to_string()),
            // A guarded write found nothing to update — the row was
            // deleted concurrently between the caller's read and this
            // write (e.g. `ThreadRepo::update_message` racing a message
            // delete). Surface as a clean 404 rather than a 500/409: the
            // resource genuinely isn't there anymore.
            ogrenotes_storage::repo::RepoError::NotFound(msg) => ApiError::NotFound(msg.clone()),
            // InvalidArgument signals a caller-side programmer
            // error — the repo refused to write because the
            // arguments would corrupt downstream semantics. Surface
            // as 500 because it indicates a bug at the route
            // handler, not a bad user request.
            ogrenotes_storage::repo::RepoError::InvalidArgument(_) => {
                ApiError::Internal(err.to_string())
            }
            // TooLarge fires when a bounded read would have
            // materialized more bytes than the per-request budget
            // (#91). 503 ServiceUnavailable communicates "this
            // doc is too large for the current task to serve" —
            // failing one doc is correct vs OOM-ing the task and
            // taking every doc down with it. The body carries the
            // numbers so operators can correlate with alarms.
            ogrenotes_storage::repo::RepoError::TooLarge { what, actual, cap } => {
                ApiError::ServiceUnavailable(format!(
                    "Response too large for {what}: {actual} bytes \
                     exceeds the {cap}-byte per-request cap. \
                     This doc needs compaction before it can be served."
                ))
            }
        }
    }
}

impl From<ogrenotes_auth::jwt::AuthError> for ApiError {
    fn from(err: ogrenotes_auth::jwt::AuthError) -> Self {
        match err {
            ogrenotes_auth::jwt::AuthError::TokenExpired => ApiError::Unauthorized,
            ogrenotes_auth::jwt::AuthError::TokenInvalid => ApiError::Unauthorized,
            ogrenotes_auth::jwt::AuthError::SessionNotFound => ApiError::Unauthorized,
            ogrenotes_auth::jwt::AuthError::SessionExpired => ApiError::Unauthorized,
            ogrenotes_auth::jwt::AuthError::RefreshTokenInvalid => ApiError::Unauthorized,
            ogrenotes_auth::jwt::AuthError::RefreshTokenReused => ApiError::Unauthorized,
            // Cross-provider / credential conflict (SAML account, dev-login
            // adopting a real account, email reassignment, etc.) — a 409, not a
            // 500 that pollutes internal-error metrics. Log the specific reason
            // server-side, but return a GENERIC message: don't confirm to the
            // client that the email is registered or which provider it uses
            // (account-existence / provider enumeration — gap-004).
            ogrenotes_auth::jwt::AuthError::OAuth(detail) => {
                tracing::info!(detail = %detail, "oauth account conflict");
                ApiError::Conflict(
                    "This account can't be used with this sign-in method. If you \
                     already have an account, sign in with the method you used to \
                     create it."
                        .to_string(),
                )
            }
            _ => ApiError::Internal(err.to_string()),
        }
    }
}

impl From<ogrenotes_collab::document::DocError> for ApiError {
    fn from(err: ogrenotes_collab::document::DocError) -> Self {
        match &err {
            ogrenotes_collab::document::DocError::DecodeState(_) => {
                ApiError::BadRequest("Invalid document content".to_string())
            }
            _ => ApiError::Internal(err.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status_of(err: ApiError) -> u16 {
        err.into_response().status().as_u16()
    }

    // ─── Status codes ───────────────────────────────────────────

    #[test]
    fn test_not_found_status() {
        assert_eq!(status_of(ApiError::NotFound("x".into())), 404);
    }

    #[test]
    fn test_unauthorized_status() {
        assert_eq!(status_of(ApiError::Unauthorized), 401);
    }

    #[test]
    fn test_forbidden_status() {
        assert_eq!(status_of(ApiError::Forbidden), 403);
    }

    #[test]
    fn test_bad_request_status() {
        assert_eq!(status_of(ApiError::BadRequest("x".into())), 400);
    }

    #[test]
    fn test_conflict_status() {
        assert_eq!(status_of(ApiError::Conflict("x".into())), 409);
    }

    #[test]
    fn test_internal_status() {
        assert_eq!(status_of(ApiError::Internal("x".into())), 500);
    }

    #[test]
    fn test_auth_oauth_conflict_maps_to_409() {
        // Regression: cross-provider / credential-conflict rejections must be
        // 409 (user-facing), not 500. Previously fell through to Internal.
        let err: ApiError = ogrenotes_auth::jwt::AuthError::OAuth("already linked".into()).into();
        assert_eq!(status_of(err), 409);
    }

    #[tokio::test]
    async fn test_auth_oauth_conflict_scrubs_detail() {
        // gap-004: the specific reason (which can name the email / provider)
        // must NOT reach the client — the 409 body is generic.
        let resp = ApiError::from(ogrenotes_auth::jwt::AuthError::OAuth(
            "Email victim@example.com is registered via SAML".into(),
        ))
        .into_response();
        assert_eq!(resp.status().as_u16(), 409);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "conflict");
        let msg = json["message"].as_str().unwrap_or("");
        assert!(!msg.contains("victim@example.com"), "must not leak the email: {msg}");
        assert!(!msg.to_lowercase().contains("saml"), "must not leak the provider: {msg}");
    }

    #[tokio::test]
    async fn test_error_json_shape() {
        let resp = ApiError::NotFound("test msg".into()).into_response();
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "not_found");
        assert_eq!(json["message"], "test msg");
    }

    // ─── RepoError conversions ──────────────────────────────────

    #[test]
    fn test_repo_missing_field_maps_to_internal() {
        let err: ApiError = ogrenotes_storage::repo::RepoError::MissingField("x".into()).into();
        assert_eq!(status_of(err), 500);
    }

    #[test]
    fn test_repo_conditional_check_maps_to_conflict() {
        let err: ApiError = ogrenotes_storage::repo::RepoError::Dynamo(
            "ConditionalCheckFailed".into(),
        ).into();
        assert_eq!(status_of(err), 409);
    }

    #[test]
    fn test_repo_dynamo_other_maps_to_internal() {
        let err: ApiError = ogrenotes_storage::repo::RepoError::Dynamo(
            "some other error".into(),
        ).into();
        assert_eq!(status_of(err), 500);
    }

    #[test]
    fn test_repo_s3_maps_to_internal() {
        let err: ApiError = ogrenotes_storage::repo::RepoError::S3("s3 fail".into()).into();
        assert_eq!(status_of(err), 500);
    }

    // ─── AuthError conversions ──────────────────────────────────

    #[test]
    fn test_auth_errors_map_to_unauthorized() {
        let cases = vec![
            ogrenotes_auth::jwt::AuthError::TokenExpired,
            ogrenotes_auth::jwt::AuthError::TokenInvalid,
            ogrenotes_auth::jwt::AuthError::SessionNotFound,
            ogrenotes_auth::jwt::AuthError::SessionExpired,
            ogrenotes_auth::jwt::AuthError::RefreshTokenInvalid,
            ogrenotes_auth::jwt::AuthError::RefreshTokenReused,
        ];
        for err in cases {
            let api_err: ApiError = err.into();
            assert_eq!(status_of(api_err), 401);
        }
    }

    // ─── DocError conversions ───────────────────────────────────

    #[test]
    fn test_doc_decode_maps_to_bad_request() {
        let err: ApiError = ogrenotes_collab::document::DocError::DecodeState(
            "bad bytes".into(),
        ).into();
        assert_eq!(status_of(err), 400);
    }

    #[test]
    fn test_doc_apply_update_maps_to_internal() {
        let err: ApiError = ogrenotes_collab::document::DocError::ApplyUpdate(
            "fail".into(),
        ).into();
        assert_eq!(status_of(err), 500);
    }

    // ─── Retry-bearing + 503 variants ───────────────────────────
    // These share a separate response branch from the simple status
    // mappings above; their status, error code, and Retry-After header
    // are part of the client/IdP backoff contract.

    #[test]
    fn test_too_many_requests_status_and_retry_after() {
        let resp = ApiError::TooManyRequests {
            message: "slow down".into(),
            retry_after_secs: 30,
        }
        .into_response();
        assert_eq!(resp.status().as_u16(), 429);
        assert_eq!(
            resp.headers().get(axum::http::header::RETRY_AFTER).unwrap(),
            "30",
            "429 must carry a Retry-After header with the whole-seconds value"
        );
    }

    #[test]
    fn test_overloaded_status_and_retry_after() {
        let resp = ApiError::Overloaded {
            message: "busy".into(),
            retry_after_secs: 5,
        }
        .into_response();
        assert_eq!(resp.status().as_u16(), 503);
        assert_eq!(
            resp.headers().get(axum::http::header::RETRY_AFTER).unwrap(),
            "5",
        );
    }

    #[test]
    fn test_service_unavailable_status() {
        assert_eq!(status_of(ApiError::ServiceUnavailable("down".into())), 503);
    }

    #[test]
    fn test_repo_too_large_maps_to_service_unavailable() {
        // #91: a bounded read that blows its cap must surface as 503
        // (transient/retryable), NOT the generic 500.
        let err: ApiError = ogrenotes_storage::repo::RepoError::TooLarge {
            what: "pending updates for doc-x".into(),
            actual: 100,
            cap: 10,
        }
        .into();
        assert_eq!(status_of(err), 503);
    }
}
