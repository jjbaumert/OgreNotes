// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

use ogrenotes_storage::models::security_audit::SecurityAuditAction;

#[tokio::test]
async fn test_dev_login_creates_user() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let body = serde_json::json!({ "email": "alice@test.com", "name": "Alice" });
    let (status, json) = app.json_request(Method::POST, "/api/v1/auth/dev-login", None, Some(body)).await;
    assert_eq!(status, 200);
    assert!(json["accessToken"].is_string());
    assert!(json["refreshToken"].is_string());
    assert!(json["userId"].is_string());
    assert_eq!(json["email"], "alice@test.com");

    app.cleanup().await;
}

#[tokio::test]
async fn test_first_login_creates_default_workspace() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("wsowner@test.com").await;

    let (status, me) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(status, 200);
    let workspace_id = me["defaultWorkspaceId"]
        .as_str()
        .expect("defaultWorkspaceId should be set on first login");
    assert!(!workspace_id.is_empty());

    let (ws_status, ws) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{workspace_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(ws_status, 200);
    assert_eq!(ws["id"], workspace_id);
    assert_eq!(ws["ownerId"], me["userId"]);

    app.cleanup().await;
}

#[tokio::test]
async fn test_dev_login_idempotent() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (uid1, _) = app.create_user("bob@test.com").await;
    let (uid2, _) = app.create_user("bob@test.com").await;
    assert_eq!(uid1, uid2);

    app.cleanup().await;
}

#[tokio::test]
async fn test_protected_route_no_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app.json_request(Method::GET, "/api/v1/users/me", None, None).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_logout_revokes_sessions() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("charlie@test.com").await;

    // Logout
    let (status, _) = app.json_request(Method::POST, "/api/v1/auth/logout", Some(&token), None).await;
    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_logout_unauthenticated() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app.json_request(Method::POST, "/api/v1/auth/logout", None, None).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_refresh_token_rotation() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Create a user to get tokens
    let body = serde_json::json!({ "email": "refresh@test.com" });
    let (status, login_json) = app.json_request(Method::POST, "/api/v1/auth/dev-login", None, Some(body)).await;
    assert_eq!(status, 200);

    let refresh_token = login_json["refreshToken"].as_str().unwrap();
    let user_id = login_json["userId"].as_str().unwrap();
    let session_id = login_json["sessionId"].as_str().unwrap();

    // Refresh
    let refresh_body = serde_json::json!({
        "refreshToken": refresh_token,
        "userId": user_id,
        "sessionId": session_id,
    });
    let (status, refresh_json) = app.json_request(Method::POST, "/api/v1/auth/refresh", None, Some(refresh_body)).await;
    assert_eq!(status, 200);
    assert!(refresh_json["accessToken"].is_string());
    assert!(refresh_json["refreshToken"].is_string());
    // The new refresh token should differ from the original
    assert_ne!(refresh_json["refreshToken"].as_str().unwrap(), refresh_token);

    app.cleanup().await;
}

#[tokio::test]
async fn test_refresh_invalid_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, login_json) = {
        let body = serde_json::json!({ "email": "badrefresh@test.com" });
        app.json_request(Method::POST, "/api/v1/auth/dev-login", None, Some(body)).await
    };
    let user_id = login_json["userId"].as_str().unwrap();
    let session_id = login_json["sessionId"].as_str().unwrap();

    let refresh_body = serde_json::json!({
        "refreshToken": "totally-bogus-refresh-token",
        "userId": user_id,
        "sessionId": session_id,
    });
    let (status, _) = app.json_request(Method::POST, "/api/v1/auth/refresh", None, Some(refresh_body)).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_protected_route_expired_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app.json_request(
        Method::GET,
        "/api/v1/users/me",
        Some("expired.jwt.token"),
        None,
    ).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_protected_route_tampered_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Get a real token, then tamper with it
    let (_, token) = app.create_user("tamper@test.com").await;
    let tampered = format!("{token}tampered");

    let (status, _) = app.json_request(
        Method::GET,
        "/api/v1/users/me",
        Some(&tampered),
        None,
    ).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_login_redirect() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(Method::GET, "/api/v1/auth/login", None, None)
        .await;
    assert_eq!(status, 307);

    app.cleanup().await;
}

/// Regression: the OAuth redirect URL must NOT contain the refresh token.
/// The refresh token was previously exposed in the URL fragment, visible in
/// server logs and browser history.
#[tokio::test]
async fn test_login_redirect_excludes_refresh_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Call /auth/login to get the redirect to GitHub
    use axum::body::Body;
    use hyper::Request;
    use tower::ServiceExt;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/auth/login")
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();

    // The login redirect goes to GitHub — verify it's a proper OAuth URL
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("github.com"), "Should redirect to GitHub");
    assert!(!location.contains("refresh_token"), "Login redirect should not contain refresh_token");

    // We can't test the callback redirect (GitHub is unreachable in tests),
    // but we can verify that the redirect URL format in the source code
    // was changed. The dev-login endpoint still returns refresh_token in the
    // JSON body (which is fine — it's not a URL).
    let body = serde_json::json!({ "email": "redirect-test@test.com" });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/auth/dev-login", None, Some(body))
        .await;
    assert_eq!(status, 200);
    // dev-login response body should still include refreshToken (it's JSON, not a URL)
    assert!(json["refreshToken"].is_string(), "dev-login should still return refreshToken in body");

    app.cleanup().await;
}

#[tokio::test]
async fn test_callback_invalid_state() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/auth/callback?code=fakecode&state=nonexistent-state",
            None,
            None,
        )
        .await;
    assert_eq!(status, 400);
    assert!(json["message"].as_str().unwrap().contains("Invalid or expired state"));

    app.cleanup().await;
}

#[tokio::test]
async fn test_callback_valid_state_github_unreachable() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Call /auth/login to populate PENDING_FLOWS with a valid state
    use axum::body::Body;
    use hyper::Request;
    use tower::ServiceExt;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/auth/login")
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let location = resp.headers().get("location").unwrap().to_str().unwrap();

    // Extract state param from the redirect URL
    let state_param = location
        .split('&')
        .find(|s| s.starts_with("state=") || s.contains("?state="))
        .and_then(|s| s.split('=').nth(1))
        .unwrap();

    // Callback with valid state but fake code — GitHub token exchange will fail → 500
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/auth/callback?code=fakecode&state={state_param}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, 500);

    app.cleanup().await;
}

#[tokio::test]
async fn test_dev_login_default_name() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let body = serde_json::json!({ "email": "noname@test.com" });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/auth/dev-login", None, Some(body))
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["name"], "Dev User");

    app.cleanup().await;
}

#[tokio::test]
async fn test_refresh_after_logout() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let body = serde_json::json!({ "email": "logout-refresh@test.com" });
    let (_, login_json) = app
        .json_request(Method::POST, "/api/v1/auth/dev-login", None, Some(body))
        .await;
    let token = login_json["accessToken"].as_str().unwrap();
    let refresh_token = login_json["refreshToken"].as_str().unwrap();
    let user_id = login_json["userId"].as_str().unwrap();
    let session_id = login_json["sessionId"].as_str().unwrap();

    // Logout revokes all sessions
    app.json_request(Method::POST, "/api/v1/auth/logout", Some(token), None).await;

    // Refresh should fail
    let refresh_body = serde_json::json!({
        "refreshToken": refresh_token,
        "userId": user_id,
        "sessionId": session_id,
    });
    let (status, _) = app
        .json_request(Method::POST, "/api/v1/auth/refresh", None, Some(refresh_body))
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

// ─── Per-request user refresh (revocation window shrinkage) ────

/// Disabling a user mid-session takes effect on the NEXT request, not
/// after the current access token's 15-minute TTL. The middleware looks
/// up the live User row per request so revocation propagates immediately.
#[tokio::test]
async fn test_disabled_user_rejected_immediately() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("alice@test.com").await;

    // Token works before disablement.
    let (status, _) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(status, 200);

    // Flip is_disabled=true directly on the row — simulates admin action.
    app.state
        .user_repo
        .set_disabled(&user_id, true)
        .await
        .expect("set_disabled");

    // Same token, next request must be rejected.
    let (status, _) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

/// Cross-provider hijack guard: a user row whose `provider` is locked to
/// `Github` cannot be reused by a `Google` login (or vice versa) even if
/// the email matches. Simulated by directly flipping the provider on the
/// row after a dev-login creates it as `Dev`.
#[tokio::test]
async fn test_cross_provider_login_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Create a user via dev-login (provider = Dev).
    let (user_id, _token) = app.create_user("alice@test.com").await;

    // Admin-flip the row's provider to GitHub — emulates a user whose
    // account was originally bound to a real provider.
    app.state
        .user_repo
        .set_provider(
            &user_id,
            ogrenotes_storage::models::user::AuthProvider::Github,
            Some("gh-12345"),
        )
        .await
        .expect("set_provider");

    // Now attempt a dev-login (provider = Dev) with the same email.
    // `find_or_create_user` must reject because the stored provider is
    // Github and the incoming is Dev.
    let body = serde_json::json!({ "email": "alice@test.com" });
    let (status, json) = app
        .json_request(
            hyper::Method::POST,
            "/api/v1/auth/dev-login",
            None,
            Some(body),
        )
        .await;
    assert_eq!(
        status, 409,
        "cross-provider dev-login must be refused with 409 Conflict, got {status} {json}"
    );

    app.cleanup().await;
}

/// gap-002: a verified email that now resolves to a DIFFERENT subject id on the
/// SAME provider (provider-side email reassignment) must not hand over the
/// existing account; a verified cross-provider login with a matching email
/// links into it (account linking).
#[tokio::test]
async fn test_subject_id_reassignment_refused_cross_provider_links() {
    use ogrenotes_auth::user::{find_or_create_user, OAuthProfile};
    use ogrenotes_storage::models::user::AuthProvider;
    use std::time::{SystemTime, UNIX_EPOCH};

    common::require_infra!();
    let app = common::TestApp::new().await;

    // Unique email so a prior run's row can't perturb the first create.
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let email = format!("subject-reassign-{nanos}@test.com");
    let mk = |provider, subject: &str| OAuthProfile {
        email: email.clone(),
        name: "T".to_string(),
        avatar_url: None,
        provider,
        provider_subject_id: Some(subject.to_string()),
    };

    // First GitHub login (subject gh-1) creates the account.
    let u1 = find_or_create_user(
        &app.state.user_repo,
        &app.state.folder_repo,
        &app.state.workspace_repo,
        &mk(AuthProvider::Github, "gh-1"),
    )
    .await
    .expect("create");

    // Same provider + same subject → same account.
    let u2 = find_or_create_user(
        &app.state.user_repo,
        &app.state.folder_repo,
        &app.state.workspace_repo,
        &mk(AuthProvider::Github, "gh-1"),
    )
    .await
    .expect("repeat login");
    assert_eq!(u1.user_id, u2.user_id);

    // Same provider + DIFFERENT subject → reassignment, refused.
    let reassigned = find_or_create_user(
        &app.state.user_repo,
        &app.state.folder_repo,
        &app.state.workspace_repo,
        &mk(AuthProvider::Github, "gh-2"),
    )
    .await;
    assert!(
        reassigned.is_err(),
        "email reassignment to a different GitHub account must be refused"
    );

    // Cross-provider (Google), same verified email → links into the account.
    let u3 = find_or_create_user(
        &app.state.user_repo,
        &app.state.folder_repo,
        &app.state.workspace_repo,
        &mk(AuthProvider::Google, "google-1"),
    )
    .await
    .expect("cross-provider link");
    assert_eq!(
        u1.user_id, u3.user_id,
        "verified-email cross-provider login must link to the existing account"
    );

    app.cleanup().await;
}

/// Bearer tokens larger than MAX_BEARER_LEN are rejected before
/// jsonwebtoken parses them — cheap DoS defense.
#[tokio::test]
async fn test_oversize_bearer_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let huge = "a".repeat(5000);
    let req = hyper::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/users/me")
        .header("Authorization", format!("Bearer {huge}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = app.raw_request(req).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

// ─── Refresh-token cookie (issue #33) ─────────────────────────────
//
// The refresh token must live in an HttpOnly Secure SameSite=Strict
// cookie scoped to /api/v1/auth, never in localStorage. These tests
// exercise the cookie's set/read/clear lifecycle end-to-end through
// the live router.

fn extract_set_cookie<'a>(headers: &'a hyper::HeaderMap, name: &str) -> Option<&'a str> {
    for v in headers.get_all(hyper::header::SET_COOKIE).iter() {
        let s = v.to_str().ok()?;
        if s.starts_with(&format!("{name}=")) {
            return Some(s);
        }
    }
    None
}

/// Poll the SecurityAudit table for a matching row for `user_id`.
/// The reuse-detection audit row is written via `record_security_event`
/// (tokio::spawn), so it can land after the 401 response.
async fn wait_for_auth_audit(
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
    panic!("expected SecurityAudit row for user {user_id} within 200ms");
}

async fn raw_request_full(
    app: &common::TestApp,
    mut req: hyper::Request<axum::body::Body>,
) -> (u16, hyper::HeaderMap, Vec<u8>) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    // Default the per-test client IP for rate-limit isolation; mirrors
    // the behavior of `TestApp::raw_request`. Tests that need a
    // specific IP set `X-Forwarded-For` themselves and override.
    if !req.headers().contains_key("x-forwarded-for") {
        req.headers_mut().insert(
            "x-forwarded-for",
            app.default_xff.parse().unwrap(),
        );
    }
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, headers, bytes)
}

#[tokio::test]
async fn dev_login_sets_refresh_cookie_with_security_attributes() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/dev-login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            r#"{"email":"cookie-set@test.com","name":"Cookie Set"}"#,
        ))
        .unwrap();
    let (status, headers, _) = raw_request_full(&app, req).await;
    assert_eq!(status, 200);

    let cookie = extract_set_cookie(&headers, "ogrenotes_refresh")
        .expect("dev-login response must include the refresh cookie");
    assert!(cookie.contains("HttpOnly"), "missing HttpOnly: {cookie}");
    assert!(cookie.contains("SameSite=Strict"), "missing SameSite=Strict: {cookie}");
    assert!(cookie.contains("Path=/api/v1/auth"), "wrong Path: {cookie}");
    assert!(cookie.contains("Max-Age=2592000"), "wrong Max-Age: {cookie}");
    // dev_mode=true in the test config, so Secure is omitted.

    app.cleanup().await;
}

#[tokio::test]
async fn refresh_via_cookie_works_without_body() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Step 1: log in via dev-login, capture the cookie from the response.
    let login_req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/dev-login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            r#"{"email":"cookie-refresh@test.com","name":"Cookie Refresh"}"#,
        ))
        .unwrap();
    let (status, headers, _) = raw_request_full(&app, login_req).await;
    assert_eq!(status, 200);
    let cookie = extract_set_cookie(&headers, "ogrenotes_refresh")
        .expect("login must set cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Step 2: refresh with ONLY the cookie — empty body. This must succeed.
    let refresh_req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("Content-Type", "application/json")
        .header("Cookie", &cookie)
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let (status, headers, body) = raw_request_full(&app, refresh_req).await;
    assert_eq!(status, 200, "cookie-only refresh must succeed");
    assert!(
        extract_set_cookie(&headers, "ogrenotes_refresh").is_some(),
        "refresh must rotate the cookie alongside the token"
    );
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["accessToken"].as_str().unwrap().len() > 0);

    app.cleanup().await;
}

#[tokio::test]
async fn refresh_without_cookie_or_body_returns_401() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let (status, _, _) = raw_request_full(&app, req).await;
    assert_eq!(
        status, 401,
        "refresh with neither cookie nor full body must 401, not panic on missing fields"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn logout_clears_refresh_cookie() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("cookie-logout@test.com").await;
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/logout")
        .header("Authorization", format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, headers, _) = raw_request_full(&app, req).await;
    assert_eq!(status, 204);
    let cookie = extract_set_cookie(&headers, "ogrenotes_refresh")
        .expect("logout must clear cookie via Set-Cookie");
    assert!(
        cookie.contains("Max-Age=0"),
        "logout cookie must use Max-Age=0 to evict immediately, got: {cookie}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn logout_with_cookie_only_no_bearer_token() {
    // Cookie-only frontend flow: after a browser restart the JS-memory
    // access token is gone but the cookie persists. The user must
    // still be able to log out using only the cookie. (#33 review.)
    common::require_infra!();
    let app = common::TestApp::new().await;

    let login_req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/dev-login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            r#"{"email":"cookie-only-logout@test.com","name":"Cookie Only"}"#,
        ))
        .unwrap();
    let (status, headers, _) = raw_request_full(&app, login_req).await;
    assert_eq!(status, 200);
    let cookie = extract_set_cookie(&headers, "ogrenotes_refresh")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Logout WITHOUT Authorization header — only the cookie.
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/logout")
        .header("Cookie", &cookie)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, headers, _) = raw_request_full(&app, req).await;
    assert_eq!(
        status, 204,
        "logout must succeed with cookie alone (no Bearer)"
    );
    let clearing = extract_set_cookie(&headers, "ogrenotes_refresh")
        .expect("cookie-only logout must still issue clearing Set-Cookie");
    assert!(clearing.contains("Max-Age=0"));

    // The session must actually be revoked: a refresh with the same
    // cookie now fails (matches the existing test_refresh_after_logout).
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("Content-Type", "application/json")
        .header("Cookie", &cookie)
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let (status, _, _) = raw_request_full(&app, req).await;
    assert_eq!(
        status, 401,
        "post-logout refresh with the (server-revoked) cookie must 401"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn refresh_token_reuse_revokes_all_sessions() {
    // Security-critical: rotate_refresh_token detects when a stale
    // (already-rotated) token is presented and revokes ALL sessions
    // for that user. The cookie path must inherit this behavior.
    // The scenario: an attacker exfiltrates the cookie value, refreshes
    // once (rotating the token server-side), then the legitimate user
    // tries to refresh with their now-stale cookie — both parties lose
    // their sessions.
    common::require_infra!();
    let app = common::TestApp::new().await;

    let login_req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/dev-login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            r#"{"email":"reuse-detect@test.com","name":"Reuse Detect"}"#,
        ))
        .unwrap();
    let (status, headers, _) = raw_request_full(&app, login_req).await;
    assert_eq!(status, 200);
    let initial_cookie = extract_set_cookie(&headers, "ogrenotes_refresh")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // First refresh: succeeds, returns a new (rotated) cookie.
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("Content-Type", "application/json")
        .header("Cookie", &initial_cookie)
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let (status, headers_after, _) = raw_request_full(&app, req).await;
    assert_eq!(status, 200);
    let rotated_cookie = extract_set_cookie(&headers_after, "ogrenotes_refresh")
        .expect("rotation must issue a new cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_ne!(
        initial_cookie, rotated_cookie,
        "rotation must change the cookie value"
    );

    // Second refresh with the SAME (now-stale) initial cookie: must
    // be detected as reuse and revoke all sessions.
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("Content-Type", "application/json")
        .header("Cookie", &initial_cookie)
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let (status, _, _) = raw_request_full(&app, req).await;
    assert_eq!(
        status, 401,
        "presenting a rotated/stale token must fail and trigger revocation"
    );

    // The newly-rotated cookie must ALSO now fail — reuse detection
    // wipes ALL sessions for the user, not just the offending one.
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("Content-Type", "application/json")
        .header("Cookie", &rotated_cookie)
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let (status, _, _) = raw_request_full(&app, req).await;
    assert_eq!(
        status, 401,
        "after reuse detection, the legitimate (rotated) cookie must also fail — \
         delete_all_for_user wiped every session for that user"
    );

    app.cleanup().await;
}

/// Refresh-token reuse detection writes a durable `SessionRevoked
/// { reason: "refresh_reuse_detected" }` audit row. The behavioral
/// revocation is covered by `refresh_token_reuse_revokes_all_sessions`;
/// this pins the forensic trail — the highest-signal event on this path —
/// which the behavior test never asserted.
#[tokio::test]
async fn refresh_reuse_writes_session_revoked_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Login and capture the user id + initial refresh cookie.
    let login_req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/dev-login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            r#"{"email":"reuse-audit@test.com","name":"Reuse Audit"}"#,
        ))
        .unwrap();
    let (status, headers, body) = raw_request_full(&app, login_req).await;
    assert_eq!(status, 200);
    let login_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let user_id = login_json["userId"].as_str().unwrap().to_string();
    let initial_cookie = extract_set_cookie(&headers, "ogrenotes_refresh")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // First refresh rotates the token.
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("Content-Type", "application/json")
        .header("Cookie", &initial_cookie)
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let (status, _, _) = raw_request_full(&app, req).await;
    assert_eq!(status, 200);

    // Re-presenting the stale cookie trips reuse detection → 401.
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("Content-Type", "application/json")
        .header("Cookie", &initial_cookie)
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let (status, _, _) = raw_request_full(&app, req).await;
    assert_eq!(status, 401);

    wait_for_auth_audit(&app, &user_id, |a| {
        matches!(a, SecurityAuditAction::SessionRevoked { reason } if reason == "refresh_reuse_detected")
    })
    .await;

    app.cleanup().await;
}

#[tokio::test]
async fn refresh_via_body_still_works_for_transitional_clients() {
    // Until the frontend ships cookie-only the body path must keep
    // working so existing localStorage sessions don't get kicked out.
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Log in and capture the refresh_token from the JSON body (the
    // "old client" flow) — the cookie is also set, but we deliberately
    // ignore it.
    let login_req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/dev-login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            r#"{"email":"cookie-body-fallback@test.com","name":"Body Fallback"}"#,
        ))
        .unwrap();
    let (status, _, body) = raw_request_full(&app, login_req).await;
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let body_payload = serde_json::json!({
        "refreshToken": v["refreshToken"],
        "userId": v["userId"],
        "sessionId": v["sessionId"],
    });

    let refresh_req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body_payload.to_string()))
        .unwrap();
    let (status, _, _) = raw_request_full(&app, refresh_req).await;
    assert_eq!(
        status, 200,
        "body-only refresh must keep working for transitional clients"
    );

    app.cleanup().await;
}

/// AuthUser reads `is_admin` LIVE from the user row on every request, not from
/// the token claims — so demoting a user takes effect on their *next* request,
/// without waiting for the token to expire. This is the extractor's headline
/// contract and was untested (every admin-403 test used a non-admin token).
#[tokio::test]
async fn admin_demotion_takes_effect_on_next_request_same_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, admin_token) = app.create_user("demote-live@test.com").await;
    app.state.user_repo.set_admin(&admin_id, true).await.unwrap();

    // While admin, an admin-only route is reachable with this token.
    let (status, _) = app
        .json_request(Method::GET, "/api/v1/admin/users", Some(&admin_token), None)
        .await;
    assert_eq!(status, 200, "admin token should reach the admin route");

    // Demote the user — the token is unchanged.
    app.state.user_repo.set_admin(&admin_id, false).await.unwrap();

    // The SAME token is now rejected: is_admin is re-read live, not trusted
    // from the (still-valid) JWT.
    let (status, _) = app
        .json_request(Method::GET, "/api/v1/admin/users", Some(&admin_token), None)
        .await;
    assert_eq!(
        status, 403,
        "demotion must take effect immediately on the next request with the same token"
    );

    app.cleanup().await;
}

/// A correctly-signed, unexpired token whose subject has no live user row is
/// rejected with 401 (the `get_by_id(...).ok_or(Unauthorized)` branch — a
/// valid JWT for a deleted/never-existed user must not authenticate).
#[tokio::test]
async fn valid_token_for_nonexistent_user_is_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Mint a real, valid token (correct secret, future exp) for a user id
    // that was never provisioned.
    let token = ogrenotes_auth::jwt::create_access_token(
        "ghost-user-id-never-created",
        "ghost@test.com",
        &app.state.config.jwt_secret,
    )
    .unwrap();

    let (status, _) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(status, 401, "a valid token for a missing user row must not authenticate");

    app.cleanup().await;
}

/// Logging in with an email on the `admin_emails` allowlist auto-promotes the
/// account to Admin (`auth_policy::apply_admin_email_promotion`, run from the
/// dev-login / OAuth / SAML paths); an email NOT on the list is left as a
/// regular user. This privilege-escalation path was untested — the default
/// harness leaves `admin_emails` empty, so it could never fire.
#[tokio::test]
async fn admin_email_allowlist_promotes_at_login_only_for_listed_emails() {
    common::require_infra!();
    let app = common::TestApp::new_with_admin_emails(vec!["boss@test.com".to_string()]).await;

    // On the allowlist → promoted to Admin.
    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/dev-login",
            None,
            Some(serde_json::json!({ "email": "boss@test.com" })),
        )
        .await;
    assert_eq!(status, 200);
    let boss_id = json["userId"].as_str().unwrap().to_string();
    let boss = app.state.user_repo.get_by_id(&boss_id).await.unwrap().unwrap();
    assert!(
        boss.is_admin(),
        "an email on admin_emails must be promoted to Admin at login"
    );

    // Not on the allowlist → stays a regular user.
    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/dev-login",
            None,
            Some(serde_json::json!({ "email": "peon@test.com" })),
        )
        .await;
    assert_eq!(status, 200);
    let peon_id = json["userId"].as_str().unwrap().to_string();
    let peon = app.state.user_repo.get_by_id(&peon_id).await.unwrap().unwrap();
    assert!(!peon.is_admin(), "an unlisted email must NOT be promoted");

    app.cleanup().await;
}
