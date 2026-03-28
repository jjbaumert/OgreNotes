use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// API error type that maps to HTTP responses.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
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
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg.clone()),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, "conflict", msg.clone()),
            ApiError::Internal(msg) => {
                tracing::error!(error = %msg, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Something went wrong".to_string(),
                )
            }
        };

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
