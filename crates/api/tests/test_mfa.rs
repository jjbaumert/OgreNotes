// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for the Phase 4 M-E3 MFA endpoints (piece B).
//!
//! Covers POST /auth/mfa/enroll, POST /auth/mfa/verify, and
//! DELETE /auth/mfa. The challenge + recovery login-flow endpoints
//! land in piece C and have their own test file.

mod common;

use hyper::Method;

use ogrenotes_auth::mfa;
use ogrenotes_storage::models::security_audit::SecurityAuditAction;

/// Poll the SecurityAudit table for a matching row for `user_id`.
/// MFA audit writers fire via `tokio::spawn`, so the HTTP response can
/// race the DynamoDB write — same 10×20ms bound as the audit-writer suite.
async fn wait_for_mfa_audit(
    app: &common::TestApp,
    user_id: &str,
    matcher: impl Fn(&SecurityAuditAction) -> bool,
) {
    for _ in 0..10 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(user_id, 20)
            .await
            .unwrap();
        if rows.iter().any(|r| matcher(&r.action)) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("expected MFA SecurityAudit row for user {user_id} within 200ms");
}

/// Run a TOTP for a Base32 secret + email account, return the
/// current 6-digit code. Used as the "user-side" of the round trip:
/// we generate the code the same way the authenticator app would.
fn current_code(secret_b32: &str, email: &str) -> String {
    let totp = mfa::totp_for(secret_b32, "OgreNotes", email).expect("totp_for");
    totp.generate_current().expect("generate_current")
}

#[tokio::test]
async fn enroll_returns_provisioning_uri_and_recovery_codes() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("mfa-enroll@test.com").await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);

    // Provisioning URI must embed the same secret the response
    // returns — otherwise the QR shown to the user encodes a
    // different secret than the server stored.
    let secret = json["secret"].as_str().expect("secret");
    assert!(!secret.is_empty());
    let uri = json["provisioningUri"].as_str().expect("provisioningUri");
    assert!(uri.starts_with("otpauth://totp/"), "uri: {uri}");
    assert!(uri.contains(&format!("secret={secret}")), "uri must embed secret");

    let codes = json["recoveryCodes"].as_array().expect("recoveryCodes");
    assert_eq!(codes.len(), 10);

    app.cleanup().await;
}

#[tokio::test]
async fn verify_with_correct_code_finalizes_enrollment() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("mfa-verify@test.com").await;

    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();

    // Pre-verify: User row should have secret but no enrolled_at.
    let user = app.state.user_repo.get_by_id(&user_id).await.unwrap().unwrap();
    assert!(user.mfa_secret.is_some(), "enroll must persist secret");
    assert!(
        user.mfa_enrolled_at.is_none(),
        "verify is the only path that sets mfa_enrolled_at"
    );

    let code = current_code(&secret, "mfa-verify@test.com");
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;
    assert_eq!(status, 204);

    // Post-verify: enrolled_at is now set.
    let user = app.state.user_repo.get_by_id(&user_id).await.unwrap().unwrap();
    assert!(
        user.mfa_enrolled_at.is_some(),
        "verify must flip mfa_enrolled_at"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn verify_with_wrong_code_returns_401_and_does_not_enroll() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("mfa-wrong-code@test.com").await;

    let (_, _enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": "000000" })),
        )
        .await;
    assert_eq!(status, 401);

    let user = app.state.user_repo.get_by_id(&user_id).await.unwrap().unwrap();
    assert!(
        user.mfa_enrolled_at.is_none(),
        "wrong code must NOT enroll the user"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn enroll_idempotently_wipes_old_recovery_codes() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("mfa-re-enroll@test.com").await;

    // First enrollment: 10 codes land.
    let (_, first) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let _first_secret = first["secret"].as_str().unwrap().to_string();

    let rows = app
        .state
        .mfa_recovery_repo
        .list_for_user(&user_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 10);

    // Re-enroll: the prior 10 codes must be gone, replaced by 10
    // fresh ones — bcrypt of any prior plaintext should NOT verify
    // against the new hashes. We just confirm the count is still 10
    // (not 20), which proves the wipe-then-write semantic.
    let (_, second) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let _second_secret = second["secret"].as_str().unwrap().to_string();

    let rows = app
        .state
        .mfa_recovery_repo
        .list_for_user(&user_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 10, "re-enroll must wipe prior codes, not accumulate");

    app.cleanup().await;
}

#[tokio::test]
async fn disarm_clears_secret_enrollment_and_recovery() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("mfa-disarm@test.com").await;

    // Enroll + verify so the user is fully armed.
    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let code = current_code(&secret, "mfa-disarm@test.com");
    let (vs, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;
    assert_eq!(vs, 204);

    // Disarm with a fresh code (still within the 30s step).
    let disarm_code = current_code(&secret, "mfa-disarm@test.com");
    let (status, _) = app
        .json_request(
            Method::DELETE,
            "/api/v1/auth/mfa/disarm",
            Some(&token),
            Some(serde_json::json!({ "code": disarm_code })),
        )
        .await;
    assert_eq!(status, 204);

    // All three pieces of MFA state must be gone.
    let user = app.state.user_repo.get_by_id(&user_id).await.unwrap().unwrap();
    assert!(user.mfa_secret.is_none(), "disarm must clear mfa_secret");
    assert!(
        user.mfa_enrolled_at.is_none(),
        "disarm must clear mfa_enrolled_at"
    );
    let rows = app
        .state
        .mfa_recovery_repo
        .list_for_user(&user_id)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "disarm must wipe recovery codes (otherwise a recovery code from the pre-disarm state would still redeem against a re-enroll's fresh secret)"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn disarm_without_fresh_code_returns_401() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("mfa-disarm-401@test.com").await;

    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let code = current_code(&secret, "mfa-disarm-401@test.com");
    let _ = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;

    // Wrong disarm code → 401, MFA state untouched.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            "/api/v1/auth/mfa/disarm",
            Some(&token),
            Some(serde_json::json!({ "code": "000000" })),
        )
        .await;
    assert_eq!(status, 401);

    let user = app.state.user_repo.get_by_id(&user_id).await.unwrap().unwrap();
    assert!(
        user.mfa_secret.is_some() && user.mfa_enrolled_at.is_some(),
        "failed disarm must NOT clear MFA state"
    );

    app.cleanup().await;
}

// ─── Login flow (Phase 4 M-E3 piece C) ──────────────────────

/// Dev-login an existing email, returning the response status +
/// JSON. Used to exercise the MFA-pending branch without going
/// through the OAuth callback.
async fn dev_login(app: &common::TestApp, email: &str) -> (u16, serde_json::Value) {
    app.json_request(
        Method::POST,
        "/api/v1/auth/dev-login",
        None,
        Some(serde_json::json!({ "email": email, "name": "Probe" })),
    )
    .await
}

/// Enroll + verify a user so they're fully MFA-armed. Returns the
/// TOTP secret so the caller can compute current codes for the
/// challenge step.
async fn enroll_and_verify(app: &common::TestApp, email: &str) -> (String, String) {
    let (_, token) = app.create_user(email).await;
    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let code = current_code(&secret, email);
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;
    assert_eq!(status, 204);
    (secret, token)
}

#[tokio::test]
async fn dev_login_returns_202_with_handle_when_mfa_enrolled() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-login-flow@test.com";
    let _ = enroll_and_verify(&app, email).await;

    let (status, json) = dev_login(&app, email).await;
    assert_eq!(status, 202, "MFA-enrolled user must NOT get an immediate token");
    let handle = json["handle"].as_str().expect("handle must be returned");
    assert!(!handle.is_empty());
    // The post-MFA-pending response must NOT carry an accessToken
    // (that would defeat the whole point — JWT minted without the
    // second factor).
    assert!(
        json.get("accessToken").is_none(),
        "MFA-pending response must not include accessToken, got: {json}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn dev_login_returns_200_with_token_when_mfa_not_enrolled() {
    // Regression guard: the MFA branch must not affect users who
    // haven't enrolled. Without this test, a typo in the
    // `mfa_enrolled_at.is_some()` condition could lock everyone
    // out.
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (status, json) = dev_login(&app, "no-mfa@test.com").await;
    assert_eq!(status, 200, "non-MFA user must get an immediate token");
    assert!(
        json["accessToken"].as_str().is_some(),
        "non-MFA response must include accessToken, got: {json}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn challenge_with_correct_code_mints_session() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-challenge-ok@test.com";
    let (secret, _) = enroll_and_verify(&app, email).await;

    // dev_login again — this time the user is MFA-enrolled.
    let (status, json) = dev_login(&app, email).await;
    assert_eq!(status, 202);
    let handle = json["handle"].as_str().unwrap().to_string();

    // Submit the current TOTP via /auth/mfa/challenge.
    let code = current_code(&secret, email);
    let (status, body) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/challenge",
            None,
            Some(serde_json::json!({ "handle": handle, "code": code })),
        )
        .await;
    assert_eq!(status, 200, "correct code must mint session, got body: {body}");
    assert!(body["accessToken"].as_str().is_some());
    assert!(body["refreshToken"].as_str().is_some());
    assert_eq!(body["email"].as_str().unwrap(), email);

    app.cleanup().await;
}

#[tokio::test]
async fn challenge_with_wrong_code_401s_and_preserves_handle() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-challenge-bad@test.com";
    let (secret, _) = enroll_and_verify(&app, email).await;
    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/challenge",
            None,
            Some(serde_json::json!({ "handle": handle, "code": "000000" })),
        )
        .await;
    assert_eq!(status, 401);

    // Handle MUST still be valid — typos shouldn't burn the
    // partial-auth state. Retry with the correct code succeeds.
    let code = current_code(&secret, email);
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/challenge",
            None,
            Some(serde_json::json!({ "handle": handle, "code": code })),
        )
        .await;
    assert_eq!(status, 200, "handle must survive a wrong-code attempt");

    app.cleanup().await;
}

/// M-E8 gap-002 — leaked-handle-then-spray defense. The test
/// AppConfig sets `mfa_challenge_max_failures = 3`; the 4th wrong
/// submission must 429 and invalidate the handle so subsequent
/// attempts can't keep spraying.
#[tokio::test]
async fn challenge_exhausts_failure_budget_then_burns_handle() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-budget-exhaust@test.com";
    let (secret, _) = enroll_and_verify(&app, email).await;
    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();

    // Three wrong codes — each 401, handle preserved.
    for attempt in 1..=3 {
        let (status, _) = app
            .json_request(
                Method::POST,
                "/api/v1/auth/mfa/challenge",
                None,
                Some(serde_json::json!({ "handle": handle, "code": "000000" })),
            )
            .await;
        assert_eq!(status, 401, "attempt {attempt} should be 401 within budget");
    }

    // Fourth wrong code — budget exhausted, must 429 AND burn the
    // handle.
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/challenge",
            None,
            Some(serde_json::json!({ "handle": handle, "code": "000000" })),
        )
        .await;
    assert_eq!(status, 429, "4th wrong submission must trip the budget");

    // Handle is now gone — even a correct code returns 401 (peek
    // sees no handle, returns Unauthorized).
    let code = current_code(&secret, email);
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/challenge",
            None,
            Some(serde_json::json!({ "handle": handle, "code": code })),
        )
        .await;
    assert_eq!(status, 401, "post-burn submissions get 401 (handle gone)");

    app.cleanup().await;
}

/// M-E8 gap-002 — the failure counter is shared across
/// `/auth/mfa/challenge` and `/auth/mfa/recovery` so an attacker
/// can't cycle between the two endpoints to double their budget.
/// 2 wrong challenge attempts + 2 wrong recovery attempts (4 total
/// against a budget of 3) → the 4th submission, on whichever
/// endpoint, is 429.
#[tokio::test]
async fn mfa_failure_counter_is_shared_across_challenge_and_recovery() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-shared-counter@test.com";
    let _ = enroll_and_verify(&app, email).await;
    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();

    // Two wrong challenges.
    for _ in 0..2 {
        let (status, _) = app
            .json_request(
                Method::POST,
                "/api/v1/auth/mfa/challenge",
                None,
                Some(serde_json::json!({ "handle": handle, "code": "000000" })),
            )
            .await;
        assert_eq!(status, 401);
    }

    // One wrong recovery — still within budget (count = 3).
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/recovery",
            None,
            Some(serde_json::json!({ "handle": handle, "code": "ZZZZZ-YYYYY" })),
        )
        .await;
    assert_eq!(status, 401, "3rd attempt across endpoints stays within budget");

    // Second wrong recovery — count = 4, must 429.
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/recovery",
            None,
            Some(serde_json::json!({ "handle": handle, "code": "ZZZZZ-YYYYY" })),
        )
        .await;
    assert_eq!(
        status, 429,
        "shared counter must trip on the 4th combined attempt, regardless of endpoint",
    );

    app.cleanup().await;
}

#[tokio::test]
async fn challenge_with_unknown_handle_401s() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/challenge",
            None,
            Some(serde_json::json!({
                "handle": "never-issued-handle-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                "code": "123456"
            })),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn challenge_handle_is_single_use_on_success() {
    // After a successful challenge, replaying the same handle must
    // fail — the GETDEL consumes it.
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-challenge-replay@test.com";
    let (secret, _) = enroll_and_verify(&app, email).await;
    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();
    let code = current_code(&secret, email);

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/challenge",
            None,
            Some(serde_json::json!({ "handle": handle.clone(), "code": code.clone() })),
        )
        .await;
    assert_eq!(status, 200);

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/challenge",
            None,
            Some(serde_json::json!({ "handle": handle, "code": code })),
        )
        .await;
    assert_eq!(status, 401, "consumed handle must not redeem twice");

    app.cleanup().await;
}

#[tokio::test]
async fn recovery_with_valid_code_mints_session_and_consumes_code() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-recovery-ok@test.com";
    let (user_id, token) = app.create_user(email).await;
    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let recovery_codes: Vec<String> = enroll["recoveryCodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let code = current_code(&secret, email);
    let _ = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;

    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();

    // Redeem the first recovery code via the recovery endpoint.
    let recovery_code = recovery_codes[0].clone();
    let (status, body) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/recovery",
            None,
            Some(serde_json::json!({ "handle": handle, "code": recovery_code.clone() })),
        )
        .await;
    assert_eq!(status, 200, "recovery body: {body}");
    assert!(body["accessToken"].as_str().is_some());

    // The redeemed code is single-use: try it again via a fresh
    // handle and expect 401.
    let (_, json) = dev_login(&app, email).await;
    let handle2 = json["handle"].as_str().unwrap().to_string();
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/recovery",
            None,
            Some(serde_json::json!({ "handle": handle2, "code": recovery_code })),
        )
        .await;
    assert_eq!(status, 401, "consumed recovery code must not redeem again");

    // Bookkeeping: the user now has 9 recovery rows, not 10.
    let rows = app
        .state
        .mfa_recovery_repo
        .list_for_user(&user_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 9, "successful redemption must delete one row");

    app.cleanup().await;
}

#[tokio::test]
async fn recovery_with_invalid_code_401s() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-recovery-bad@test.com";
    let _ = enroll_and_verify(&app, email).await;
    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/recovery",
            None,
            Some(serde_json::json!({ "handle": handle, "code": "NOPE!-NOPE!" })),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn recovery_handle_consumed_before_request_returns_401() {
    // Coarse coverage of the "handle is gone by the time recovery
    // arrives" path. Specifically tests the cheap peek-fails case
    // (the handler short-circuits at peek_mfa_pending and never
    // looks at the codes). The narrower "peek succeeds, then take
    // races to None between delete and take" path is NOT reachable
    // without an in-process Redis injection seam — see the
    // documented trade-off in the `recovery` handler. This test
    // pins the simpler observable: pre-consumed handle → 401 with
    // ZERO side effects (recovery rows unchanged).
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-recovery-stale-handle@test.com";
    let (user_id, token) = app.create_user(email).await;
    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let recovery_codes: Vec<String> = enroll["recoveryCodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let code = current_code(&secret, email);
    let _ = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;

    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();
    let _ = app
        .state
        .redis_session
        .take_mfa_pending(&handle)
        .await
        .expect("test setup: consume handle before recovery POST");

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/recovery",
            None,
            Some(serde_json::json!({ "handle": handle, "code": recovery_codes[0].clone() })),
        )
        .await;
    assert_eq!(status, 401, "stale handle must surface as 401");

    let rows = app
        .state
        .mfa_recovery_repo
        .list_for_user(&user_id)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        10,
        "stale-handle 401 must NOT touch recovery rows (peek short-circuits before delete)"
    );

    app.cleanup().await;
}

// ─── Workspace MFA enforcement (Phase 4 M-E3 piece D) ────────

#[tokio::test]
async fn login_response_carries_mfa_enrollment_required_when_workspace_requires_it() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-enforce@test.com";
    let (user_id, _) = app.create_user(email).await;

    // Flip the user's default workspace to mfa_required.
    let user = app.state.user_repo.get_by_id(&user_id).await.unwrap().unwrap();
    let ws_id = user
        .default_workspace_id
        .expect("first-login should have minted a default workspace");
    app.state
        .workspace_repo
        .set_mfa_required(&ws_id, true)
        .await
        .unwrap();

    // dev-login again. The user has NOT enrolled in MFA, so the
    // response must carry mfa_enrollment_required = true.
    let (status, json) = dev_login(&app, email).await;
    assert_eq!(status, 200, "unenrolled user still gets a session");
    assert_eq!(
        json["mfaEnrollmentRequired"].as_bool(),
        Some(true),
        "workspace flag must surface as mfaEnrollmentRequired on the token, got: {json}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn login_response_omits_mfa_enrollment_required_when_workspace_does_not_require_it() {
    // Regression guard for the wire-shape promise: the field is
    // ABSENT (skip_serializing_if_none), not `null`. Frontend
    // decoders that treat null as false-default would still work,
    // but the explicit-absence shape is what the doc-comment claims.
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (status, json) = dev_login(&app, "mfa-not-enforced@test.com").await;
    assert_eq!(status, 200);
    assert!(
        json.get("mfaEnrollmentRequired").is_none(),
        "field must be absent on the no-MFA path, got: {json}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn login_response_omits_mfa_enrollment_required_after_user_enrolls() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-enrolled-ws-required@test.com";
    let (user_id, _) = app.create_user(email).await;
    let ws_id = app
        .state
        .user_repo
        .get_by_id(&user_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .unwrap();
    app.state
        .workspace_repo
        .set_mfa_required(&ws_id, true)
        .await
        .unwrap();

    // User enrolls + verifies.
    let _ = enroll_and_verify(&app, email).await;

    // Next login: user is enrolled, so they go through the MFA
    // challenge step. The handle response (202) doesn't have the
    // mfa_enrollment_required field — it's a different shape.
    let (status, json) = dev_login(&app, email).await;
    assert_eq!(status, 202, "enrolled user gets MFA challenge handle");
    assert!(json.get("mfaEnrollmentRequired").is_none());

    app.cleanup().await;
}

#[tokio::test]
async fn users_me_carries_mfa_enrollment_required() {
    // /users/me re-reports the same flag so the frontend can check
    // on page hydration (after a refresh) without going through the
    // login flow.
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-me-flag@test.com";
    let (user_id, token) = app.create_user(email).await;
    let ws_id = app
        .state
        .user_repo
        .get_by_id(&user_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .unwrap();
    app.state
        .workspace_repo
        .set_mfa_required(&ws_id, true)
        .await
        .unwrap();

    let (status, json) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        json["mfaEnrollmentRequired"].as_bool(),
        Some(true),
        "users/me must surface workspace-required flag, got: {json}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn set_mfa_required_route_requires_workspace_admin() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    // Owner creates a workspace.
    let (owner_id, owner_token) = app.create_user("ws-owner@test.com").await;
    let (status, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&owner_token),
            Some(serde_json::json!({ "name": "Acme" })),
        )
        .await;
    assert_eq!(status, 201);
    let ws_id = ws_json["id"].as_str().unwrap().to_string();
    let _ = owner_id;

    // A random user (not a member) tries to flip the flag → 403.
    let (_, intruder_token) = app.create_user("ws-intruder@test.com").await;
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/mfa-required"),
            Some(&intruder_token),
            Some(serde_json::json!({ "required": true })),
        )
        .await;
    assert_eq!(status, 403);

    // The owner can flip it.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/mfa-required"),
            Some(&owner_token),
            Some(serde_json::json!({ "required": true })),
        )
        .await;
    assert_eq!(status, 204);
    let ws = app.state.workspace_repo.get(&ws_id).await.unwrap().unwrap();
    assert!(ws.mfa_required, "owner toggle must persist");

    // Owner can also turn it off (REMOVE attribute path).
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/mfa-required"),
            Some(&owner_token),
            Some(serde_json::json!({ "required": false })),
        )
        .await;
    assert_eq!(status, 204);
    let ws = app.state.workspace_repo.get(&ws_id).await.unwrap().unwrap();
    assert!(!ws.mfa_required, "owner toggle-off must persist");

    app.cleanup().await;
}

#[tokio::test]
async fn users_me_clears_mfa_enrollment_required_after_verify() {
    // F4 from the M-E3 piece-D review: pin the flag's expiration
    // semantic on the /users/me side. The login-response side has
    // its own test (`login_response_omits_..._after_user_enrolls`);
    // this is the matching observable for the page-hydration path.
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-me-flag-clears@test.com";
    let (user_id, token) = app.create_user(email).await;
    let ws_id = app
        .state
        .user_repo
        .get_by_id(&user_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .unwrap();
    app.state
        .workspace_repo
        .set_mfa_required(&ws_id, true)
        .await
        .unwrap();

    // Pre-enroll: flag must be true.
    let (_, json) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(json["mfaEnrollmentRequired"].as_bool(), Some(true));

    // Enroll + verify. The verify endpoint flips mfa_enrolled_at,
    // which is the helper's early-return condition.
    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let code = current_code(&secret, email);
    let (vs, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;
    assert_eq!(vs, 204);

    // Post-enroll: flag must be ABSENT (skip_serializing_if_none).
    let (_, json) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert!(
        json.get("mfaEnrollmentRequired").is_none(),
        "flag must clear once mfa_enrolled_at is set, got: {json}"
    );

    app.cleanup().await;
}

// ─── SecurityAudit emission ─────────────────────────────────────
//
// MFA mutations are security-critical events the audit log must record
// (CLAUDE.md: identity write-paths emit SecurityAudit). These assert the
// rows actually fire — the behavior tests above never checked the audit
// trail.

/// Enrolling writes a `MfaEnroll` audit row (secret generated, QR scanned,
/// not yet verified). Self-event: actor == subject.
#[tokio::test]
async fn enroll_writes_mfa_enroll_audit_row() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("mfa-audit-enroll@test.com").await;
    let (status, _) = app
        .json_request(Method::POST, "/api/v1/auth/mfa/enroll", Some(&token), None)
        .await;
    assert_eq!(status, 200);

    wait_for_mfa_audit(&app, &user_id, |a| {
        matches!(a, SecurityAuditAction::MfaEnroll)
    })
    .await;

    app.cleanup().await;
}

/// A wrong code at verify time writes `MfaVerify { ok: false }` — the row
/// that MFA-bypass-attempt rate-limit alerts watch for.
#[tokio::test]
async fn verify_wrong_code_writes_mfa_verify_false_audit_row() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("mfa-audit-wrong@test.com").await;
    let (_, _enroll) = app
        .json_request(Method::POST, "/api/v1/auth/mfa/enroll", Some(&token), None)
        .await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": "000000" })),
        )
        .await;
    assert_eq!(status, 401);

    wait_for_mfa_audit(&app, &user_id, |a| {
        matches!(a, SecurityAuditAction::MfaVerify { ok: false })
    })
    .await;

    app.cleanup().await;
}

/// Disarming (after a fresh TOTP) writes `MfaDisarm` — the only forensic
/// trail that an account's MFA was removed, since the enrollment rows are
/// gone afterwards.
#[tokio::test]
async fn disarm_writes_mfa_disarm_audit_row() {
    common::require_infra!();
    let _ = *common::MFA_KEY_INIT;
    let app = common::TestApp::new().await;

    let email = "mfa-audit-disarm@test.com";
    let (user_id, token) = app.create_user(email).await;

    // Enroll + verify so the account is fully armed.
    let (_, enroll) = app
        .json_request(Method::POST, "/api/v1/auth/mfa/enroll", Some(&token), None)
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let (vs, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": current_code(&secret, email) })),
        )
        .await;
    assert_eq!(vs, 204);

    // Disarm with a fresh code.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            "/api/v1/auth/mfa/disarm",
            Some(&token),
            Some(serde_json::json!({ "code": current_code(&secret, email) })),
        )
        .await;
    assert_eq!(status, 204);

    wait_for_mfa_audit(&app, &user_id, |a| {
        matches!(a, SecurityAuditAction::MfaDisarm)
    })
    .await;

    app.cleanup().await;
}

/// Redeeming a valid recovery code writes `SecurityAudit::MfaRecoveryUsed`.
/// A consumed recovery code is a second-factor bypass event — the design
/// keeps it distinct from `MfaVerify { ok: true }` so forensics can see
/// which logins skipped TOTP. This writer had no coverage; the functional
/// redemption test never looks at the audit table.
#[tokio::test]
async fn recovery_success_writes_recovery_used_audit_row() {
    common::require_infra!();
    std::sync::LazyLock::force(&common::MFA_KEY_INIT);
    let app = common::TestApp::new().await;

    let email = "mfa-recovery-audit-used@test.com";
    let (user_id, token) = app.create_user(email).await;
    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let recovery_code = enroll["recoveryCodes"][0].as_str().unwrap().to_string();
    let code = current_code(&secret, email);
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;
    assert_eq!(status, 204);

    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();

    let (status, body) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/recovery",
            None,
            Some(serde_json::json!({ "handle": handle, "code": recovery_code })),
        )
        .await;
    assert_eq!(status, 200, "recovery body: {body}");

    wait_for_mfa_audit(&app, &user_id, |a| {
        matches!(a, SecurityAuditAction::MfaRecoveryUsed)
    })
    .await;

    app.cleanup().await;
}

/// A recovery attempt with a non-matching code writes
/// `SecurityAudit::MfaRecoveryFailed`. The design keeps this distinct from
/// `MfaVerify { ok: false }` because the bypass-attempt rate-limit alert
/// needs to tell 50-bit recovery-code brute-force apart from 6-digit TOTP
/// brute-force. This writer had no coverage.
#[tokio::test]
async fn recovery_wrong_code_writes_recovery_failed_audit_row() {
    common::require_infra!();
    std::sync::LazyLock::force(&common::MFA_KEY_INIT);
    let app = common::TestApp::new().await;

    let email = "mfa-recovery-audit-failed@test.com";
    let (user_id, token) = app.create_user(email).await;
    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let code = current_code(&secret, email);
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;
    assert_eq!(status, 204);

    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();

    // One wrong attempt — inside the failure budget (3), so a plain 401.
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/recovery",
            None,
            Some(serde_json::json!({ "handle": handle, "code": "NOPE!-NOPE!" })),
        )
        .await;
    assert_eq!(status, 401);

    wait_for_mfa_audit(&app, &user_id, |a| {
        matches!(a, SecurityAuditAction::MfaRecoveryFailed)
    })
    .await;

    app.cleanup().await;
}

/// Issue #14: recovery codes are minted uppercase from a case-free
/// base32 alphabet, so a lowercase transcription is the same code —
/// redemption must accept it end to end (and not burn a lockout-counter
/// attempt on letter case).
#[tokio::test]
async fn recovery_with_lowercase_code_mints_session() {
    common::require_infra!();
    std::sync::LazyLock::force(&common::MFA_KEY_INIT);
    let app = common::TestApp::new().await;

    let email = "mfa-recovery-lowercase@test.com";
    let (_user_id, token) = app.create_user(email).await;
    let (_, enroll) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/enroll",
            Some(&token),
            None,
        )
        .await;
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let recovery_code = enroll["recoveryCodes"][0].as_str().unwrap().to_string();
    let code = current_code(&secret, email);
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/verify",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;
    assert_eq!(status, 204);

    let (_, json) = dev_login(&app, email).await;
    let handle = json["handle"].as_str().unwrap().to_string();

    let (status, body) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/mfa/recovery",
            None,
            Some(serde_json::json!({
                "handle": handle,
                "code": recovery_code.to_lowercase(),
            })),
        )
        .await;
    assert_eq!(status, 200, "lowercase recovery body: {body}");

    app.cleanup().await;
}
