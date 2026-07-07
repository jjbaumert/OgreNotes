// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! SCIM bearer-token verification (Phase 4 M-E5 piece B).
//!
//! Distinct from `AuthUser` because:
//!   - SCIM principals are not real users — they're machine
//!     identities owned by a workspace's IdP provisioning agent.
//!     A SCIM request must NOT touch `last_active_at`, the
//!     rolling-users gauge, or any other "did a real user act"
//!     tracker.
//!   - The token format is `<token_id>.<secret>` rather than a
//!     JWT, so JWT validation does not apply.
//!   - Lookup is workspace-scoped: the URL path carries the
//!     workspace_id, and the (workspace_id, token_id) pair is the
//!     DDB key. There is no global token lookup — a stolen token
//!     does not gain power across workspaces.
//!
//! Wire format expected in the Authorization header:
//!
//!   Authorization: Bearer <token_id>.<secret>
//!
//! `token_id` is the 16-char public nanoid stored on the row;
//! `secret` is the 32-byte base64-url-no-pad string only the admin
//! ever saw. bcrypt-verify of `secret` against `row.secret_hash`
//! is the authentication check.

use axum::http::HeaderMap;

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::workspace_scim_token::{BcryptHash, WorkspaceScimToken};

use crate::error::ApiError;
use crate::state::AppState;

/// Hard cap on Bearer token length. Real SCIM tokens are ~60 bytes
/// (16-char token_id + `.` + ~43-char base64 secret); 4 KB matches
/// the AuthUser extractor's cap and shuts down DoS-sized inputs
/// before bcrypt ever runs.
const MAX_BEARER_LEN: usize = 4096;

/// bcrypt work factor. Same as the codebase's MFA path
/// (`crates/auth/src/mfa.rs::hash_recovery_code`). Bcrypt 10 is the
/// industry-standard balance for human-paced verification — every
/// SCIM request runs one verify, and 10 keeps that to a few ms.
const BCRYPT_COST: u32 = 10;

/// Bytes of randomness in the secret half of the token. 32 bytes =
/// 256 bits of entropy, base64-url-no-pad encoded to ~43 chars.
const SECRET_RANDOM_BYTES: usize = 32;

/// Output of [`mint_token`]. Three values that arrive together at
/// the issuance handler:
///   - `plaintext` — shown to the admin exactly once.
///   - `token_id`  — stored on the row as the SK suffix.
///   - `secret_hash` — stored on the row.
pub struct MintedToken {
    pub plaintext: String,
    pub token_id: String,
    pub secret_hash: BcryptHash,
}

/// Mint a fresh SCIM token. Wrapping the generate-and-hash steps in
/// one helper closes the bug class where issuance code might store
/// plaintext instead of the hash (the F-1 finding on the prior
/// piece). The plaintext returned is the only place the secret
/// ever exists outside the bcrypt output.
pub fn mint_token() -> Result<MintedToken, bcrypt::BcryptError> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let token_id = nanoid::nanoid!(16);
    let secret_bytes: [u8; SECRET_RANDOM_BYTES] = rand::random();
    let secret = URL_SAFE_NO_PAD.encode(secret_bytes);
    let hash = bcrypt::hash(&secret, BCRYPT_COST)?;
    let plaintext = format!("{token_id}.{secret}");
    Ok(MintedToken {
        plaintext,
        token_id,
        secret_hash: BcryptHash::new(hash),
    })
}

/// Verified SCIM principal. Returned by [`verify_scim_request`] on
/// every authenticated SCIM call. Carries the `token_id` so audit
/// rows (piece F) can correlate a request to the row that issued
/// it; carries the `name` so log lines can identify which token
/// connector is acting.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ScimAuth {
    pub workspace_id: String,
    pub token_id: String,
    pub name: String,
}

/// Authenticate a SCIM request. Returns the verified principal or
/// an `ApiError::Unauthorized` with no discriminating detail in the
/// body (all failure paths collapse to one response, same pattern
/// as the SAML ACS handler).
///
/// On success, fires a fire-and-forget `touch_last_used` update so
/// the admin can see when each token was last exercised. A failure
/// of that update must NOT reject the request — the auth check
/// already passed.
pub async fn verify_scim_request(
    state: &AppState,
    headers: &HeaderMap,
    workspace_id: &str,
) -> Result<ScimAuth, ApiError> {
    // M-E8 gap-005: per-workspace rate limit. Runs BEFORE the DDB
    // lookup and the bcrypt verify so a flood of well-formed-but-
    // wrong-bearer requests against a known workspace_id can't
    // drive unbounded CPU work. Workspace_id is path-extracted
    // pre-auth — safe to use as a rate-limit key because the
    // workspace identifier isn't a secret.
    //
    // Trade-off: an unauthenticated attacker who probes valid
    // workspace_ids can DoS that workspace's SCIM endpoint. We
    // accept that because (a) per-workspace keying isolates the
    // blast radius from the rest of the API and from other
    // workspaces, and (b) a victim admin can rotate their tokens
    // to invalidate the attacker's foothold before it matters.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "scim_request",
        workspace_id,
        state.config.scim_request_rate_limit_per_minute,
        60,
    )
    .await?;

    let bearer = extract_bearer_token(headers)?;
    let (token_id, secret) = parse_token_id_and_secret(&bearer)?;

    let row = state
        .workspace_scim_token_repo
        .get(workspace_id, token_id)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, workspace_id, "SCIM token lookup failed");
            ApiError::Unauthorized
        })?
        .ok_or_else(|| {
            tracing::warn!(workspace_id, "SCIM token_id has no row");
            ApiError::Unauthorized
        })?;

    if !row.is_active() {
        tracing::warn!(
            workspace_id,
            token_id = %row.token_id,
            "SCIM token presented but disabled — rejecting"
        );
        return Err(ApiError::Unauthorized);
    }

    // bcrypt verify. `unwrap_or(false)` because any internal error
    // (malformed hash, etc.) must fail closed; we don't leak the
    // distinction to the caller.
    let ok = bcrypt::verify(secret, row.secret_hash.as_str()).unwrap_or(false);
    if !ok {
        tracing::warn!(
            workspace_id,
            token_id = %row.token_id,
            "SCIM bearer secret failed bcrypt verify"
        );
        return Err(ApiError::Unauthorized);
    }

    // Fire-and-forget last_used_at stamp. Done in a spawned task so
    // the auth check returns immediately and the SCIM handler isn't
    // blocked on a write. A failure here logs but does NOT reject
    // the request.
    let repo = state.workspace_scim_token_repo.clone();
    let ws = row.workspace_id.clone();
    let tid = row.token_id.clone();
    let now = now_usec();
    tokio::spawn(async move {
        if let Err(e) = repo.touch_last_used(&ws, &tid, now).await {
            tracing::warn!(
                error = %e,
                workspace_id = %ws,
                token_id = %tid,
                "SCIM touch_last_used failed (auth still succeeded)"
            );
        }
    });

    Ok(ScimAuth::from(&row))
}

impl ScimAuth {
    fn from(row: &WorkspaceScimToken) -> Self {
        Self {
            workspace_id: row.workspace_id.clone(),
            token_id: row.token_id.clone(),
            name: row.name.clone(),
        }
    }
}

/// Pull the bearer token out of the Authorization header. Mirrors
/// the AuthUser extractor's caps and error shapes so a SCIM caller
/// gets the same observable behavior on malformed headers.
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

/// Split the on-wire token into `(token_id, secret)`. The wire
/// format is `<token_id>.<secret>` (GitHub PAT pattern). Returns a
/// generic `Unauthorized` on any structural error — the caller does
/// not get to learn which half was malformed.
fn parse_token_id_and_secret(token: &str) -> Result<(&str, &str), ApiError> {
    let (token_id, secret) = token.split_once('.').ok_or_else(|| {
        tracing::warn!("SCIM bearer token has no `.` separator");
        ApiError::Unauthorized
    })?;
    if token_id.is_empty() || secret.is_empty() {
        tracing::warn!("SCIM bearer token has empty token_id or secret");
        return Err(ApiError::Unauthorized);
    }
    Ok((token_id, secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bearer_token_accepts_well_formed() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer tok-id.sec123".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers).unwrap(), "tok-id.sec123");
    }

    #[test]
    fn extract_bearer_token_rejects_missing_header() {
        let headers = HeaderMap::new();
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn extract_bearer_token_rejects_non_bearer_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Basic abc".parse().unwrap());
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn extract_bearer_token_rejects_empty_value() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer ".parse().unwrap());
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn extract_bearer_token_rejects_oversized_value() {
        let mut headers = HeaderMap::new();
        let big = format!("Bearer {}", "a".repeat(MAX_BEARER_LEN + 1));
        headers.insert("Authorization", big.parse().unwrap());
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn parse_token_id_and_secret_splits_on_dot() {
        let (id, sec) = parse_token_id_and_secret("abc.xyz").unwrap();
        assert_eq!(id, "abc");
        assert_eq!(sec, "xyz");
    }

    #[test]
    fn parse_token_id_and_secret_rejects_no_separator() {
        assert!(parse_token_id_and_secret("abcxyz").is_err());
    }

    #[test]
    fn parse_token_id_and_secret_rejects_empty_token_id() {
        assert!(parse_token_id_and_secret(".xyz").is_err());
    }

    #[test]
    fn parse_token_id_and_secret_rejects_empty_secret() {
        assert!(parse_token_id_and_secret("abc.").is_err());
    }

    #[test]
    fn mint_token_round_trips_via_parse_and_bcrypt_verify() {
        // End-to-end: mint → split → bcrypt-verify must succeed.
        // If issuance and verification disagree, every SCIM request
        // fails 401. Cheapest possible regression test.
        let minted = mint_token().unwrap();
        let (id, secret) = parse_token_id_and_secret(&minted.plaintext).unwrap();
        assert_eq!(id, minted.token_id);
        assert!(bcrypt::verify(secret, minted.secret_hash.as_str()).unwrap());
    }

    #[test]
    fn mint_token_produces_distinct_tokens() {
        // 256 bits of entropy in the secret + 16-char nanoid in the
        // token_id make collision astronomically unlikely; a
        // regression where both came from a static value would be
        // catastrophic.
        let a = mint_token().unwrap();
        let b = mint_token().unwrap();
        assert_ne!(a.plaintext, b.plaintext);
        assert_ne!(a.token_id, b.token_id);
        assert_ne!(a.secret_hash.as_str(), b.secret_hash.as_str());
    }

    #[test]
    fn parse_token_id_and_secret_only_splits_on_first_dot() {
        // The secret is base64-url-no-pad so it won't contain `.`,
        // but defensively a token_id with a `.` would be malformed
        // on our side, not the wire. Our generator never emits `.`
        // in token_id, so any input matching `id.sec` shape parses
        // as (id, sec). If a future change introduces `.` somewhere,
        // splitn(2) ensures the secret retains the rest verbatim.
        let (id, sec) = parse_token_id_and_secret("abc.x.y.z").unwrap();
        assert_eq!(id, "abc");
        assert_eq!(sec, "x.y.z");
    }
}
