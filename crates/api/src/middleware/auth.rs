// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::HeaderMap;

use crate::error::ApiError;
use crate::state::AppState;

/// Hard cap on Bearer token length. JWTs in this system are ~300–500 bytes;
/// 4 KB leaves generous headroom while shutting down DoS-sized inputs
/// before jsonwebtoken ever looks at them.
const MAX_BEARER_LEN: usize = 4096;

/// Authenticated user ID extracted from the JWT in the Authorization header.
///
/// `is_admin` is sourced from the live User row on every request — not from
/// a claim — so demotion takes effect on the next request instead of after
/// the current access token's TTL expires. Disabled users are rejected
/// here with 403; the token itself stays syntactically valid until expiry
/// but cannot be used against any protected route.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AuthUser {
    pub user_id: String,
    pub email: String,
    pub is_admin: bool,
    /// #148 — per-user AI-assistant access policy. Three states
    /// (Disabled / SystemOnly / SystemOrByok); see the
    /// `AskPolicy` doc on the User model. Admins can flip it via
    /// `PUT /api/v1/admin/users/:id/ask-policy`. `is_admin`
    /// bypasses `Disabled` at the route layer, and also bypasses
    /// the BYOK-rejection under `SystemOnly`.
    pub ask_policy: ogrenotes_storage::models::user::AskPolicy,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer_token(&parts.headers)?;
        let claims = ogrenotes_auth::jwt::validate_token(&token, &state.config.jwt_secret)?;

        // Per-request refresh of the User row: a revoked admin or disabled
        // user has a still-valid JWT in hand; we source the authority from
        // the live row instead of the stale claim.
        let user = state
            .user_repo
            .get_by_id(&claims.sub)
            .await
            .map_err(|_| ApiError::Unauthorized)?
            .ok_or(ApiError::Unauthorized)?;

        if user.is_disabled {
            return Err(ApiError::Forbidden);
        }

        // Attach user_id to the current request span so all log lines inside
        // the handler carry it; feed the rolling-users tracker so the 5m/60m
        // active-user gauges stay accurate.
        tracing::Span::current().record("user_id", user.user_id.as_str());
        state.rolling_users.mark(&user.user_id);
        // Persist last_active_at (debounced). Feeds the "active in-app"
        // suppression window in the email service.
        state
            .activity_tracker
            .mark(&user.user_id, state.user_repo.clone());

        // `is_admin()` and `ask_policy()` both borrow `&user`;
        // capture the derived values before destructuring moves
        // `user_id` out below.
        let is_admin = user.is_admin();
        let ask_policy = user.ask_policy();
        Ok(AuthUser {
            user_id: user.user_id,
            email: claims.email,
            is_admin,
            ask_policy,
        })
    }
}

/// Extract the Bearer token from the Authorization header.
fn extract_bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(ApiError::Unauthorized)?;

    if token.is_empty() || token.len() > MAX_BEARER_LEN {
        return Err(ApiError::Unauthorized);
    }

    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn test_valid_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer abc123".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers).unwrap(), "abc123");
    }

    #[test]
    fn test_missing_auth_header() {
        let headers = HeaderMap::new();
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn test_non_bearer_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Basic abc123".parse().unwrap());
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn test_empty_token() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer ".parse().unwrap());
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn test_oversize_token_rejected() {
        let mut headers = HeaderMap::new();
        let big = format!("Bearer {}", "a".repeat(MAX_BEARER_LEN + 1));
        headers.insert("Authorization", big.parse().unwrap());
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn test_max_length_token_accepted() {
        let mut headers = HeaderMap::new();
        let exact = format!("Bearer {}", "a".repeat(MAX_BEARER_LEN));
        headers.insert("Authorization", exact.parse().unwrap());
        assert!(extract_bearer_token(&headers).is_ok());
    }
}
