use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::session::Session;
use ogrenotes_storage::repo::session_repo::SessionRepo;
use crate::jwt::AuthError;
use crate::oauth::{generate_refresh_token, hash_token};

/// Refresh token lifetime: 30 days in microseconds.
const REFRESH_TOKEN_TTL_USEC: i64 = 30 * 24 * 3600 * 1_000_000;

/// Result of creating a new session.
pub struct NewSession {
    pub session_id: String,
    pub refresh_token: String,
}

/// Create a new session with a hashed refresh token.
pub async fn create_session(
    repo: &SessionRepo,
    user_id: &str,
    device_info: Option<&str>,
) -> Result<NewSession, AuthError> {
    let session_id = new_id();
    let refresh_token = generate_refresh_token();
    let refresh_hash = hash_token(&refresh_token);
    let now = now_usec();

    let session = Session {
        user_id: user_id.to_string(),
        session_id: session_id.clone(),
        refresh_token_hash: refresh_hash,
        expires_at: now + REFRESH_TOKEN_TTL_USEC,
        device_info: device_info.map(|s| s.to_string()),
        created_at: now,
    };

    repo.create(&session)
        .await
        .map_err(|e| AuthError::Storage(e.to_string()))?;

    Ok(NewSession {
        session_id,
        refresh_token,
    })
}

/// Validate a refresh token and rotate it (issue a new one, invalidate old).
/// Returns (session_id, new_refresh_token).
pub async fn rotate_refresh_token(
    repo: &SessionRepo,
    user_id: &str,
    session_id: &str,
    presented_token: &str,
) -> Result<String, AuthError> {
    let session = repo
        .get(user_id, session_id)
        .await
        .map_err(|e| AuthError::Storage(e.to_string()))?
        .ok_or(AuthError::SessionNotFound)?;

    // Check expiry
    if session.is_expired() {
        return Err(AuthError::SessionExpired);
    }

    // Verify the presented refresh token
    let presented_hash = hash_token(presented_token);
    if presented_hash != session.refresh_token_hash {
        // Token reuse detected -- revoke all sessions for this user
        tracing::warn!(
            user_id = user_id,
            session_id = session_id,
            "refresh token reuse detected, revoking all sessions"
        );
        repo.delete_all_for_user(user_id)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        return Err(AuthError::RefreshTokenReused);
    }

    // Issue new refresh token
    let new_token = generate_refresh_token();
    let new_hash = hash_token(&new_token);
    let new_expires = now_usec() + REFRESH_TOKEN_TTL_USEC;

    repo.update_refresh_token(user_id, session_id, &new_hash, new_expires)
        .await
        .map_err(|e| AuthError::Storage(e.to_string()))?;

    Ok(new_token)
}

/// Revoke a specific session.
pub async fn revoke_session(
    repo: &SessionRepo,
    user_id: &str,
    session_id: &str,
) -> Result<(), AuthError> {
    repo.delete(user_id, session_id)
        .await
        .map_err(|e| AuthError::Storage(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_session_stores_hash_not_plaintext() {
        let token = generate_refresh_token();
        let hash = hash_token(&token);
        // The hash should not equal the original token
        assert_ne!(token, hash);
        // The hash should be consistent
        assert_eq!(hash, hash_token(&token));
    }

    #[test]
    fn refresh_token_ttl_is_30_days() {
        let thirty_days_usec: i64 = 30 * 24 * 3600 * 1_000_000;
        assert_eq!(REFRESH_TOKEN_TTL_USEC, thirty_days_usec);
    }
}
