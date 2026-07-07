// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

/// JWT claims for access tokens.
///
/// Note: `is_admin` was removed. Admin status is looked up from the User row
/// on every request so that demotion takes effect immediately instead of
/// after the current token's 15-minute TTL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Claims {
    /// Subject (user ID).
    pub sub: String,
    /// User email.
    pub email: String,
    /// Issuer — must match `EXPECTED_ISSUER`.
    pub iss: String,
    /// Audience — must match `EXPECTED_AUDIENCE`.
    pub aud: String,
    /// Token ID (random). Present in the claim so a future blacklist can
    /// revoke individual tokens without rotating `JWT_SECRET`.
    pub jti: String,
    /// Issued at (Unix timestamp seconds).
    pub iat: u64,
    /// Expires at (Unix timestamp seconds).
    pub exp: u64,
}

/// Access token lifetime: 15 minutes.
const ACCESS_TOKEN_TTL_SECS: u64 = 15 * 60;

/// Minimum JWT secret length in bytes (256 bits for HS256). Duplicated at
/// config-load time in `ogrenotes_common::config` so deploys fail fast on
/// a weak secret.
const MIN_SECRET_LEN: usize = 32;

/// Issuer claim — identifies this server as the minter.
pub const EXPECTED_ISSUER: &str = "ogrenotes";
/// Audience claim — identifies the api service as the consumer. Separate
/// from the issuer so future services (e.g. an admin backplane) can mint
/// their own tokens without being accepted here.
pub const EXPECTED_AUDIENCE: &str = "ogrenotes-api";

/// Create a signed JWT access token.
pub fn create_access_token(
    user_id: &str,
    email: &str,
    secret: &str,
) -> Result<String, AuthError> {
    if secret.len() < MIN_SECRET_LEN {
        return Err(AuthError::TokenCreation(format!(
            "JWT secret must be at least {MIN_SECRET_LEN} bytes, got {}",
            secret.len()
        )));
    }

    let now = jsonwebtoken::get_current_timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        email: email.to_string(),
        iss: EXPECTED_ISSUER.to_string(),
        aud: EXPECTED_AUDIENCE.to_string(),
        jti: ogrenotes_common::id::new_id(),
        iat: now,
        exp: now + ACCESS_TOKEN_TTL_SECS,
    };

    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AuthError::TokenCreation(e.to_string()))
}

/// Validate a JWT access token and return claims.
pub fn validate_token(token: &str, secret: &str) -> Result<Claims, AuthError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_required_spec_claims(&["sub", "exp", "iat", "iss", "aud"]);
    validation.set_issuer(&[EXPECTED_ISSUER]);
    validation.set_audience(&[EXPECTED_AUDIENCE]);

    let data = jsonwebtoken::decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::TokenExpired,
        jsonwebtoken::errors::ErrorKind::InvalidSignature => AuthError::TokenInvalid,
        _ => AuthError::TokenInvalid,
    })?;

    Ok(data.claims)
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("failed to create token: {0}")]
    TokenCreation(String),

    #[error("token expired")]
    TokenExpired,

    #[error("token invalid")]
    TokenInvalid,

    #[error("session not found")]
    SessionNotFound,

    #[error("session expired")]
    SessionExpired,

    #[error("refresh token invalid")]
    RefreshTokenInvalid,

    #[error("refresh token reused")]
    RefreshTokenReused,

    #[error("OAuth error: {0}")]
    OAuth(String),

    #[error("storage error: {0}")]
    Storage(String),
}

/// Every repo call in this crate funnels a `RepoError` into the generic
/// `Storage` variant — the discriminant is erased to a string because no
/// caller distinguishes storage failure kinds at the `AuthError` level.
/// Having the conversion here lets the `find_or_create_*` and session
/// functions use a plain `?` instead of repeating the closure at each
/// repo boundary.
impl From<ogrenotes_storage::repo::RepoError> for AuthError {
    fn from(e: ogrenotes_storage::repo::RepoError) -> Self {
        AuthError::Storage(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test-secret-key-for-jwt-signing-minimum-256-bits!!";

    fn expired_claims() -> Claims {
        Claims {
            sub: "user123".to_string(),
            email: "test@example.com".to_string(),
            iss: EXPECTED_ISSUER.to_string(),
            aud: EXPECTED_AUDIENCE.to_string(),
            jti: "jti-1".to_string(),
            iat: 1_000_000_000,
            exp: 1_000_000_001,
        }
    }

    #[test]
    fn create_and_validate_token() {
        let token = create_access_token("user123", "test@example.com", TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.email, "test@example.com");
        assert_eq!(claims.iss, EXPECTED_ISSUER);
        assert_eq!(claims.aud, EXPECTED_AUDIENCE);
        assert!(!claims.jti.is_empty(), "jti must be set");
    }

    #[test]
    fn distinct_tokens_have_distinct_jti() {
        // jti is random per mint so a future blacklist can target single
        // tokens.
        let a = create_access_token("u", "a@b", TEST_SECRET).unwrap();
        let b = create_access_token("u", "a@b", TEST_SECRET).unwrap();
        let a_claims = validate_token(&a, TEST_SECRET).unwrap();
        let b_claims = validate_token(&b, TEST_SECRET).unwrap();
        assert_ne!(a_claims.jti, b_claims.jti);
    }

    #[test]
    fn token_expiry_is_15_minutes() {
        let token = create_access_token("user123", "test@example.com", TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.exp - claims.iat, ACCESS_TOKEN_TTL_SECS);
    }

    #[test]
    fn expired_token_rejected() {
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &expired_claims(),
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .unwrap();

        let result = validate_token(&token, TEST_SECRET);
        assert!(matches!(result, Err(AuthError::TokenExpired)));
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = create_access_token("user123", "test@example.com", TEST_SECRET).unwrap();
        let result = validate_token(&token, "wrong-secret-key-that-does-not-match!!!!!");
        assert!(matches!(result, Err(AuthError::TokenInvalid)));
    }

    #[test]
    fn tampered_token_rejected() {
        let token = create_access_token("user123", "test@example.com", TEST_SECRET).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        let tampered = format!("{}.{}X.{}", parts[0], parts[1], parts[2]);
        let result = validate_token(&tampered, TEST_SECRET);
        assert!(result.is_err());
    }

    #[test]
    fn short_secret_rejected() {
        let result = create_access_token("user123", "test@example.com", "short");
        assert!(matches!(result, Err(AuthError::TokenCreation(_))));
    }

    #[test]
    fn wrong_issuer_rejected() {
        let now = jsonwebtoken::get_current_timestamp();
        let claims = Claims {
            sub: "user".into(),
            email: "e@e".into(),
            iss: "not-ogrenotes".into(),
            aud: EXPECTED_AUDIENCE.into(),
            jti: "jti".into(),
            iat: now,
            exp: now + 60,
        };
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .unwrap();
        assert!(matches!(
            validate_token(&token, TEST_SECRET),
            Err(AuthError::TokenInvalid)
        ));
    }

    #[test]
    fn wrong_audience_rejected() {
        let now = jsonwebtoken::get_current_timestamp();
        let claims = Claims {
            sub: "user".into(),
            email: "e@e".into(),
            iss: EXPECTED_ISSUER.into(),
            aud: "someone-else".into(),
            jti: "jti".into(),
            iat: now,
            exp: now + 60,
        };
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .unwrap();
        assert!(matches!(
            validate_token(&token, TEST_SECRET),
            Err(AuthError::TokenInvalid)
        ));
    }

    #[test]
    fn missing_sub_claim_rejected() {
        #[derive(Serialize)]
        struct NoSub {
            email: String,
            iss: String,
            aud: String,
            jti: String,
            iat: u64,
            exp: u64,
        }
        let now = jsonwebtoken::get_current_timestamp();
        let claims = NoSub {
            email: "test@example.com".to_string(),
            iss: EXPECTED_ISSUER.to_string(),
            aud: EXPECTED_AUDIENCE.to_string(),
            jti: "jti".to_string(),
            iat: now,
            exp: now + 900,
        };
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .unwrap();

        let result = validate_token(&token, TEST_SECRET);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    const TEST_SECRET: &str = "test-secret-key-for-jwt-signing-minimum-256-bits!!";

    proptest! {
        #[test]
        fn prop_jwt_roundtrip(
            user_id in "[a-zA-Z0-9_-]{1,20}",
            email in "[a-z]{3,10}@[a-z]{3,8}\\.com"
        ) {
            let token = create_access_token(&user_id, &email, TEST_SECRET).unwrap();
            let claims = validate_token(&token, TEST_SECRET).unwrap();
            prop_assert_eq!(claims.sub, user_id);
            prop_assert_eq!(claims.email, email);
        }

        #[test]
        fn prop_signature_is_deterministic(
            user_id in "[a-zA-Z0-9]{5,10}"
        ) {
            let claims = Claims {
                sub: user_id.clone(),
                email: "test@test.com".to_string(),
                iss: EXPECTED_ISSUER.to_string(),
                aud: EXPECTED_AUDIENCE.to_string(),
                jti: "fixed-jti".to_string(),
                iat: 1_700_000_000,
                exp: 1_700_000_900,
            };

            let token1 = jsonwebtoken::encode(
                &Header::new(Algorithm::HS256),
                &claims,
                &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
            ).unwrap();

            let token2 = jsonwebtoken::encode(
                &Header::new(Algorithm::HS256),
                &claims,
                &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
            ).unwrap();

            prop_assert_eq!(token1, token2);
        }
    }
}
