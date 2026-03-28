use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

/// JWT claims for access tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Claims {
    /// Subject (user ID).
    pub sub: String,
    /// User email.
    pub email: String,
    /// Issued at (Unix timestamp seconds).
    pub iat: u64,
    /// Expires at (Unix timestamp seconds).
    pub exp: u64,
}

/// Access token lifetime: 15 minutes.
const ACCESS_TOKEN_TTL_SECS: u64 = 15 * 60;

/// Minimum JWT secret length in bytes (256 bits for HS256).
const MIN_SECRET_LEN: usize = 32;

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
    validation.set_required_spec_claims(&["sub", "exp", "iat"]);

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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test-secret-key-for-jwt-signing-minimum-256-bits!!";

    #[test]
    fn create_and_validate_token() {
        let token = create_access_token("user123", "test@example.com", TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.email, "test@example.com");
    }

    #[test]
    fn token_expiry_is_15_minutes() {
        let token = create_access_token("user123", "test@example.com", TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.exp - claims.iat, ACCESS_TOKEN_TTL_SECS);
    }

    #[test]
    fn expired_token_rejected() {
        let claims = Claims {
            sub: "user123".to_string(),
            email: "test@example.com".to_string(),
            iat: 1_000_000_000,
            exp: 1_000_000_001,
        };

        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
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
    fn missing_sub_claim_rejected() {
        #[derive(Serialize)]
        struct NoClaims {
            email: String,
            iat: u64,
            exp: u64,
        }
        let now = jsonwebtoken::get_current_timestamp();
        let claims = NoClaims {
            email: "test@example.com".to_string(),
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
