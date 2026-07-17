// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! `/auth/mfa/*` — TOTP enrollment, verify, and disarm (Phase 4
//! M-E3 piece B). The challenge + recovery endpoints used by the
//! mid-login flow live in piece C.
//!
//! Three handlers here:
//!
//!   POST /auth/mfa/enroll  — generate fresh secret + recovery codes,
//!                            write them under the caller's user_id,
//!                            return the provisioning URI for the
//!                            client to render as a QR.
//!
//!   POST /auth/mfa/verify  — consume a 6-digit TOTP, flip
//!                            `mfa_enrolled_at` so the next login
//!                            requires the challenge step.
//!
//!   DELETE /auth/mfa       — disarm. Requires a fresh TOTP to prove
//!                            the caller still controls the
//!                            authenticator (defends against a
//!                            stolen-token disarm). Wipes secret,
//!                            enrollment timestamp, and all recovery
//!                            rows.
//!
//! Every failed verification collapses to `ApiError::Unauthorized` so
//! an attacker cannot distinguish "bad code" from "missing key" or
//! "no enrollment yet" — the `MfaError` variants are logged
//! server-side only.

use axum::routing::{delete, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_auth::mfa;
use ogrenotes_storage::models::security_audit::SecurityAuditAction;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::routes::audit::record_security_event;
use crate::routes::auth::{issue_session_response, SessionSource};
use crate::state::AppState;

/// Issuer string baked into the `otpauth://` provisioning URI. Shows
/// up in the user's authenticator app as the account label —
/// "OgreNotes (alice@example.com)". Hardcoded because we have one
/// product; per-workspace branding is a v2 carry-forward.
const TOTP_ISSUER: &str = "OgreNotes";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/enroll", post(enroll))
        .route("/verify", post(verify))
        // Login-flow second-factor endpoints (Phase 4 M-E3 piece C).
        // Unauthenticated by design — they complete the OAuth/dev-
        // login sequence. The `handle` in the body is the bearer of
        // partial-auth state, written by the upstream login handler
        // into Redis with a 60s TTL.
        .route("/challenge", post(challenge))
        .route("/recovery", post(recovery))
        // The plan listed disarm at the bare `/auth/mfa` path. Axum
        // 0.8's `.nest("/mfa", _).route("/", _)` matches `/mfa/`
        // (trailing slash) only — and only via the deprecated empty-
        // path syntax. An explicit `/disarm` path is unambiguous in
        // logs and avoids a router-shape gotcha that would silently
        // 404 every disarm attempt.
        .route("/disarm", delete(disarm))
}

// ─── Enroll ──────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnrollResponse {
    /// Base32-encoded TOTP secret. Manual-entry fallback for users
    /// who can't scan a QR.
    secret: String,
    /// Full `otpauth://totp/...` URI. The frontend renders this as a
    /// QR via the `qrcode` crate (browser-side; no server CPU spent
    /// on PNG generation).
    provisioning_uri: String,
    /// Ten single-use codes. Shown to the user EXACTLY ONCE — the
    /// server only retains a bcrypt hash. If the user dismisses the
    /// page without saving them, a re-enroll mints a fresh batch and
    /// supersedes these.
    recovery_codes: Vec<String>,
}

/// POST /auth/mfa/enroll
///
/// Idempotent at the user level: re-enrolling overwrites the secret
/// and recovery codes. The pre-existing recovery rows are deleted
/// FIRST so a re-enroll-then-redeem-old-code race can't redeem a
/// stale code against the new secret.
async fn enroll(
    axum::extract::State(state): axum::extract::State<AppState>,
    auth: AuthUser,
) -> Result<Json<EnrollResponse>, ApiError> {
    let key = mfa::load_key().map_err(|e| {
        tracing::error!(error = ?e, "MFA_ENCRYPTION_KEY missing/malformed");
        ApiError::Internal("MFA not configured".to_string())
    })?;

    // Build the secret + provisioning URI in one pass so the URI
    // embeds the same bytes we encrypt at rest.
    let secret_b32 = mfa::new_totp_secret();
    let totp = mfa::totp_for(&secret_b32, TOTP_ISSUER, &auth.email)
        .map_err(|_| ApiError::Internal("totp init".to_string()))?;
    let provisioning_uri = totp.get_url();

    // Encrypt before any storage touches the plaintext.
    let encrypted = mfa::encrypt(&key, secret_b32.as_bytes())
        .map_err(|_| ApiError::Internal("mfa encrypt".to_string()))?;
    state
        .user_repo
        .set_mfa_secret(&auth.user_id, Some(&encrypted))
        .await?;
    // `mfa_enrolled_at` is intentionally NOT set yet — verify flips it.
    // A user who enrolls but doesn't verify can still log in normally
    // since the login flow keys off `mfa_enrolled_at`, not on the
    // presence of the secret.
    record_security_event(&state, &auth.user_id, SecurityAuditAction::MfaEnroll);

    // Mint and store recovery codes. Wipe-then-write so a re-enroll
    // doesn't accumulate codes from prior attempts.
    state
        .mfa_recovery_repo
        .delete_all_for_user(&auth.user_id)
        .await?;

    let plain_codes = mfa::generate_recovery_codes();
    let now = ogrenotes_common::time::now_usec();
    for (idx, plaintext) in plain_codes.iter().enumerate() {
        let hash = mfa::hash_recovery_code(plaintext)
            .map_err(|_| ApiError::Internal("bcrypt hash".to_string()))?;
        state
            .mfa_recovery_repo
            .put_hashed(&auth.user_id, idx, &hash, now)
            .await?;
    }

    Ok(Json(EnrollResponse {
        secret: secret_b32,
        provisioning_uri,
        recovery_codes: plain_codes,
    }))
}

// ─── Verify ──────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyRequest {
    /// 6-digit TOTP from the user's authenticator app. The string
    /// shape (not numeric) keeps leading zeros — `"012345"` is a
    /// valid code.
    code: String,
}

/// POST /auth/mfa/verify
///
/// Consumes one TOTP, finalizes enrollment. Idempotent: re-verifying
/// an already-enrolled user with a fresh code overwrites
/// `mfa_enrolled_at` with the new timestamp but doesn't change any
/// observable state. Wrong codes → 401 with no detail leakage.
async fn verify(
    axum::extract::State(state): axum::extract::State<AppState>,
    auth: AuthUser,
    Json(req): Json<VerifyRequest>,
) -> Result<axum::http::StatusCode, ApiError> {
    // The login-flow second factor (`/challenge`, `/recovery`) is throttled
    // via the handle-scoped failure counter, but these two AUTHENTICATED
    // endpoints had no lockout — a stolen JWT (without the authenticator)
    // could otherwise spray the 6-digit TOTP space with unlimited parallel
    // requests. Cap attempts per user.
    enforce_mfa_attempt_limit(&state, &auth.user_id).await?;

    let user = state
        .user_repo
        .get_by_id(&auth.user_id)
        .await?
        .ok_or(ApiError::Unauthorized)?;

    let secret_b32 = decrypt_or_unauthorized(&user)?;
    if !mfa::verify_totp(&secret_b32, &req.code, TOTP_ISSUER, &auth.email) {
        record_security_event(
            &state,
            &auth.user_id,
            SecurityAuditAction::MfaVerify { ok: false },
        );
        return Err(ApiError::Unauthorized);
    }

    let now = ogrenotes_common::time::now_usec();
    state
        .user_repo
        .set_mfa_enrolled_at(&auth.user_id, Some(now))
        .await?;
    // Only `MfaVerify { ok: true }` on a successful verify — the
    // `MfaEnroll` row fires from the enroll handler at secret-write
    // time (per the variant doc comment in `security_audit.rs`:
    // "User initiated TOTP enrollment, not yet verified"). Emitting
    // both here would put two semantically-different events at the
    // same timestamp under the same actor, confusing the audit
    // reader.
    record_security_event(
        &state,
        &auth.user_id,
        SecurityAuditAction::MfaVerify { ok: true },
    );

    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ─── Disarm ──────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisarmRequest {
    /// Fresh TOTP. Required so a stolen JWT alone can't disarm the
    /// second factor — the attacker would also need possession of
    /// the authenticator device.
    code: String,
}

/// DELETE /auth/mfa
async fn disarm(
    axum::extract::State(state): axum::extract::State<AppState>,
    auth: AuthUser,
    Json(req): Json<DisarmRequest>,
) -> Result<axum::http::StatusCode, ApiError> {
    // Same rationale as `verify`: without this a stolen JWT ALONE could
    // brute-force the disarm code — exactly what `DisarmRequest::code`'s
    // doc comment says the fresh-TOTP requirement is meant to prevent.
    enforce_mfa_attempt_limit(&state, &auth.user_id).await?;

    let user = state
        .user_repo
        .get_by_id(&auth.user_id)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let secret_b32 = decrypt_or_unauthorized(&user)?;

    if !mfa::verify_totp(&secret_b32, &req.code, TOTP_ISSUER, &auth.email) {
        record_security_event(
            &state,
            &auth.user_id,
            SecurityAuditAction::MfaVerify { ok: false },
        );
        return Err(ApiError::Unauthorized);
    }

    // Audit BEFORE wiping. Once the secret + recovery rows are gone,
    // a subsequent read can't tell whether the user was ever
    // enrolled — the audit row is the only durable trail.
    record_security_event(&state, &auth.user_id, SecurityAuditAction::MfaDisarm);

    state.user_repo.set_mfa_secret(&auth.user_id, None).await?;
    state
        .user_repo
        .set_mfa_enrolled_at(&auth.user_id, None)
        .await?;
    state
        .mfa_recovery_repo
        .delete_all_for_user(&auth.user_id)
        .await?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ─── Helpers ─────────────────────────────────────────────────

/// Decrypt the stored TOTP secret, collapsing every failure path
/// (missing field, key config, bad ciphertext, decrypt failure) to a
/// single 401. The server-side logs preserve the distinction; the
/// client never sees which check failed.
pub(crate) fn decrypt_or_unauthorized(
    user: &ogrenotes_storage::models::user::User,
) -> Result<String, ApiError> {
    let blob = user.mfa_secret.as_ref().ok_or(ApiError::Unauthorized)?;
    let key = mfa::load_key().map_err(|e| {
        tracing::error!(error = ?e, "MFA_ENCRYPTION_KEY missing during MFA check");
        ApiError::Unauthorized
    })?;
    let plaintext = mfa::decrypt(&key, blob).map_err(|e| {
        tracing::warn!(
            user_id = %user.user_id,
            error = ?e,
            "MFA secret decrypt failed"
        );
        ApiError::Unauthorized
    })?;
    String::from_utf8(plaintext).map_err(|_| ApiError::Unauthorized)
}

/// Record one MFA wrong-code submission for `handle` and return
/// whether the failure budget is now exhausted (M-E8 gap-002).
///
/// Closes the leaked-handle-then-guess attack the pre-merge security
/// audit called out: without this, an attacker who exfiltrates a
/// pending handle (Referer-leak through a corporate proxy, broken
/// browser session, etc.) could spray TOTPs at unlimited rate within
/// the 60-second TTL.
///
/// Both `/auth/mfa/challenge` and `/auth/mfa/recovery` share one
/// counter per handle (`mfa_fail:<handle>`), so an attacker can't
/// cycle between the two endpoints to double their budget. The
/// counter TTL matches the handle's TTL (60s) — when the handle
/// would have expired anyway, the counter goes too.
///
/// Fail-open on Redis error (same posture as the `rate_limit`
/// module): a Redis blip mustn't lock out every legitimate user.
/// The blip surfaces as a `tracing::warn!` for ops visibility.
///
/// Returns true if `count > limit` (caller should burn the handle
/// via `take_mfa_pending` and return `ApiError::TooManyRequests`);
/// false if there's still budget. NEVER logs the raw handle —
/// leaking it server-side would compound the original exfil
/// scenario.
async fn record_mfa_failure_exhausted(state: &AppState, handle: &str) -> bool {
    let limit = state.config.mfa_challenge_max_failures as u64;
    let key = format!("mfa_fail:{handle}");
    let client = &state.redis;
    use fred::prelude::KeysInterface;
    match client.incr::<u64, _>(&key).await {
        Ok(count) => {
            // Match the handle's TTL so the counter resets when the
            // handle would have expired anyway. Unconditional EXPIRE
            // on every call — same idempotency pattern as
            // `rate_limit::check`.
            let _: Result<(), _> = client.expire(&key, MFA_PENDING_TTL_SECS as i64).await;
            count > limit
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "mfa failure-counter INCR failed; failing open (handle preserved)"
            );
            false
        }
    }
}

/// Window for the per-user attempt cap on the two AUTHENTICATED MFA
/// endpoints (`POST /auth/mfa/verify`, `DELETE /auth/mfa`).
const MFA_AUTHENTICATED_ATTEMPT_WINDOW_SECS: u64 = 5 * 60;

/// Cap TOTP attempts on the authenticated MFA endpoints (`verify`,
/// `disarm`), keyed on `user_id`. The login-flow endpoints burn an
/// opaque handle instead; these are authenticated, so there's no handle
/// to key on — without this a stolen JWT alone could brute-force the
/// 6-digit code with unlimited parallel requests. Reuses
/// `mfa_challenge_max_failures` so operators tune a single budget.
async fn enforce_mfa_attempt_limit(state: &AppState, user_id: &str) -> Result<(), ApiError> {
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "mfa_verify",
        user_id,
        state.config.mfa_challenge_max_failures as u64,
        MFA_AUTHENTICATED_ATTEMPT_WINDOW_SECS,
    )
    .await
}

/// TTL applied to the failure-counter key so it expires when the
/// pending handle would have expired anyway. Mirrors the value the
/// `redis_session` module uses for `mfa_pending:<handle>`; if that
/// constant ever moves, both should follow.
const MFA_PENDING_TTL_SECS: u64 = 60;

// ─── Challenge + Recovery (login step 2) ─────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeRequest {
    /// The opaque handle returned by `POST /auth/dev-login` or by
    /// the OAuth callback redirect (`?handle=...` query param).
    handle: String,
    /// 6-digit TOTP from the user's authenticator.
    code: String,
}

/// POST /auth/mfa/challenge
///
/// Completes a login by verifying the second factor. Unlike
/// `/verify`, this endpoint is NOT authenticated — it's the step
/// that transitions partial-auth (handle in Redis) into a full
/// session.
///
/// On success: consume the handle (atomic GETDEL via
/// `take_mfa_pending`), mint a session + JWT, return TokenResponse
/// with refresh cookie. On wrong code: leave the handle valid for
/// retry within the 60s TTL — a typo shouldn't force the user back
/// through OAuth.
async fn challenge(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(req): Json<ChallengeRequest>,
) -> Result<axum::response::Response, ApiError> {
    // Peek first so a wrong TOTP doesn't burn the handle.
    let user_id = state
        .redis_session
        .peek_mfa_pending(&req.handle)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "redis peek_mfa_pending failed");
            ApiError::Unauthorized
        })?
        .ok_or(ApiError::Unauthorized)?;

    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let secret_b32 = decrypt_or_unauthorized(&user)?;

    if !mfa::verify_totp(&secret_b32, &req.code, TOTP_ISSUER, &user.email) {
        record_security_event(
            &state,
            &user.user_id,
            SecurityAuditAction::MfaVerify { ok: false },
        );
        if record_mfa_failure_exhausted(&state, &req.handle).await {
            // Budget exhausted — burn the handle so further attempts
            // 401 on peek (handle gone) rather than 429-on-replay.
            // A concurrent failure may already have taken it; either
            // way return 429 to surface the exhausted-budget signal.
            let _ = state.redis_session.take_mfa_pending(&req.handle).await;
            return Err(ApiError::TooManyRequests {
                message: "MFA verification limit exceeded; restart login.".to_string(),
                retry_after_secs: MFA_PENDING_TTL_SECS,
            });
        }
        return Err(ApiError::Unauthorized);
    }

    // Atomic consume — two concurrent successful TOTP submissions
    // (e.g. duplicate-submit from a frontend reload) can't both
    // mint sessions because GETDEL leaves exactly one winner.
    let consumed = state
        .redis_session
        .take_mfa_pending(&req.handle)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "redis take_mfa_pending failed");
            ApiError::Unauthorized
        })?;
    if consumed.is_none() {
        // Lost the race or the handle expired between peek and
        // take. Surface as 401 — the user retries from scratch.
        return Err(ApiError::Unauthorized);
    }

    record_security_event(
        &state,
        &user.user_id,
        SecurityAuditAction::MfaVerify { ok: true },
    );

    issue_session_response(&state, &user, SessionSource::MfaTotp).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryRequest {
    handle: String,
    /// One of the ten codes minted at enroll-time, in `xxxxx-xxxxx`
    /// format. Verified case-insensitively (issue #14) — the base32
    /// alphabet (A-Z2-7) carries no case entropy, so a lowercase
    /// retype is the same code. See `mfa::verify_recovery_code`.
    code: String,
}

/// POST /auth/mfa/recovery
///
/// Single-use recovery-code redemption. Same flow as `/challenge`
/// but bcrypt-verifies the presented plaintext against each stored
/// hash row. On match: deletes the matched row first (so a
/// concurrent re-submission can't redeem the same code twice), then
/// consumes the handle, then mints a session.
async fn recovery(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(req): Json<RecoveryRequest>,
) -> Result<axum::response::Response, ApiError> {
    let user_id = state
        .redis_session
        .peek_mfa_pending(&req.handle)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "redis peek_mfa_pending failed");
            ApiError::Unauthorized
        })?
        .ok_or(ApiError::Unauthorized)?;

    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await?
        .ok_or(ApiError::Unauthorized)?;

    // Scan all rows for this user (bounded by RECOVERY_CODE_COUNT)
    // and bcrypt-verify each. Constant-time across the population
    // isn't needed — the bcrypt verify itself dominates the per-row
    // cost, and a partial-list user is the same target whether the
    // attacker can probe matches or not.
    let rows = state
        .mfa_recovery_repo
        .list_for_user(&user.user_id)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "list recovery codes failed");
            ApiError::Unauthorized
        })?;

    if rows.len() < ogrenotes_auth::mfa::RECOVERY_CODE_COUNT {
        // F-3 from the M-E3 piece-B review: surface degraded sets
        // (a partial-write during enroll) so an operator can see
        // them. Doesn't block redemption — the user's still
        // entitled to use one of the codes that did land.
        tracing::warn!(
            user_id = %user.user_id,
            count = rows.len(),
            expected = ogrenotes_auth::mfa::RECOVERY_CODE_COUNT,
            "recovery-code set is degraded (partial enroll); user should re-enroll"
        );
    }

    let matched_idx = rows
        .iter()
        .find(|row| mfa::verify_recovery_code(&req.code, &row.bcrypt_hash))
        .map(|row| row.idx);

    let Some(idx) = matched_idx else {
        record_security_event(
            &state,
            &user.user_id,
            SecurityAuditAction::MfaRecoveryFailed,
        );
        if record_mfa_failure_exhausted(&state, &req.handle).await {
            // Shared counter with /challenge — see record_mfa_failure_exhausted
            // for the leaked-handle-then-spray rationale.
            let _ = state.redis_session.take_mfa_pending(&req.handle).await;
            return Err(ApiError::TooManyRequests {
                message: "MFA verification limit exceeded; restart login.".to_string(),
                retry_after_secs: MFA_PENDING_TTL_SECS,
            });
        }
        return Err(ApiError::Unauthorized);
    };

    // Delete the matched row FIRST so a duplicate-submit race can't
    // redeem the same code twice. If delete fails, the redemption
    // aborts — better to fail closed than to mint a session against
    // a code that's still live.
    //
    // Trade-off: this ordering means a wrong-handle-race AFTER
    // delete (take_mfa_pending returns None) burns the code without
    // issuing a session. The user lost one recovery code without
    // benefit. The alternative ordering (take handle first) would
    // burn the handle on a typo, forcing the user back through the
    // OAuth flow on every recovery-code typo. We optimize for
    // typo-tolerance over race-tolerance; reviewer F-1 (M-E3 piece
    // C) flagged this as `should-fix, test-coverage`; the test for
    // the race is infeasible without a Redis injection seam between
    // delete and take. See `test_mfa::recovery_handle_consumed_
    // before_request_returns_401` for the adjacent observable.
    state
        .mfa_recovery_repo
        .delete(&user.user_id, idx)
        .await?;

    if state
        .redis_session
        .take_mfa_pending(&req.handle)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "redis take_mfa_pending failed");
            ApiError::Unauthorized
        })?
        .is_none()
    {
        return Err(ApiError::Unauthorized);
    }

    record_security_event(
        &state,
        &user.user_id,
        SecurityAuditAction::MfaRecoveryUsed,
    );

    issue_session_response(&state, &user, SessionSource::MfaRecovery).await
}
