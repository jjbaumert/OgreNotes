// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Typed Leptos/WASM client for `/api/v1/auth/mfa/*` (Phase 4 M-E3).
//!
//! Five operations:
//!   - `enroll()` — generate fresh secret + recovery codes (auth'd)
//!   - `verify(code)` — finalize enrollment with a TOTP (auth'd)
//!   - `challenge(handle, code)` — login step 2 (NOT auth'd)
//!   - `recovery(handle, code)` — recovery-code fallback (NOT auth'd)
//!   - `disarm(code)` — wipe MFA state (auth'd, requires fresh TOTP)
//!
//! Wire shapes match `crates/api/src/routes/mfa.rs`. The challenge
//! and recovery endpoints don't go through `api_post` (which attaches
//! a Bearer token via `ensure_token`) because the user is mid-login
//! and has no token yet — they hand-roll the fetch instead.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

use super::client::{
    api_delete_with_body, api_post, api_post_empty, http_error, ApiClientError,
};

const API_BASE: &str = "/api/v1";

// ─── Enroll ──────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResponse {
    pub secret: String,
    pub provisioning_uri: String,
    pub recovery_codes: Vec<String>,
}

/// `POST /auth/mfa/enroll` — generates a fresh TOTP secret and ten
/// single-use recovery codes. Re-enrolling supersedes any prior
/// secret + codes.
pub async fn enroll() -> Result<EnrollResponse, ApiClientError> {
    api_post("/auth/mfa/enroll", &serde_json::json!({})).await
}

// ─── Verify ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct VerifyRequest {
    code: String,
}

/// `POST /auth/mfa/verify` — submit a 6-digit TOTP to finalize
/// enrollment. Success flips the server-side `mfa_enrolled_at` so
/// the next login routes through the MFA challenge step.
pub async fn verify(code: &str) -> Result<(), ApiClientError> {
    api_post_empty(
        "/auth/mfa/verify",
        &VerifyRequest { code: code.to_string() },
    )
    .await
}

// ─── Disarm ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct DisarmRequest {
    code: String,
}

/// `DELETE /auth/mfa/disarm` — wipe secret + recovery codes.
/// Requires a fresh TOTP so a stolen-token disarm needs the
/// authenticator device too. Routes through `api_delete_with_body`
/// so an expired access token is refreshed from the HttpOnly cookie
/// before the call — otherwise a stale-token user would see a
/// generic 401 with no recovery path.
pub async fn disarm(code: &str) -> Result<(), ApiClientError> {
    api_delete_with_body(
        "/auth/mfa/disarm",
        &DisarmRequest { code: code.to_string() },
    )
    .await
}

// ─── Challenge ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeRequest {
    handle: String,
    code: String,
}

/// `POST /auth/mfa/challenge` — login step 2. Submits a TOTP against
/// the opaque handle the login handler issued. On success returns
/// the TokenResponse that the caller stores via `set_auth`. NOT
/// authenticated — the user is between OAuth/dev-login and a
/// minted session.
pub async fn challenge(
    handle: &str,
    code: &str,
) -> Result<super::client::TokenResponse, ApiClientError> {
    challenge_or_recovery("/auth/mfa/challenge", handle, code).await
}

/// `POST /auth/mfa/recovery` — same shape as challenge, but
/// verifies against the bcrypt-hashed recovery codes. Single-use
/// per code.
pub async fn recovery(
    handle: &str,
    code: &str,
) -> Result<super::client::TokenResponse, ApiClientError> {
    challenge_or_recovery("/auth/mfa/recovery", handle, code).await
}

async fn challenge_or_recovery(
    path: &str,
    handle: &str,
    code: &str,
) -> Result<super::client::TokenResponse, ApiClientError> {
    let body = serde_json::to_string(&ChallengeRequest {
        handle: handle.to_string(),
        code: code.to_string(),
    })
    .map_err(|e| ApiClientError::Serialize(e.to_string()))?;
    let url = format!("{API_BASE}{path}");
    let resp = Request::post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;
    if !resp.ok() {
        return Err(http_error(&resp));
    }
    resp.json()
        .await
        .map_err(|e| ApiClientError::Deserialize(e.to_string()))
}
