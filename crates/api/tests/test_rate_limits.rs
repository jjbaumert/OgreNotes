// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for #36 — production-facing endpoints (auth,
//! search, sharing) reject with 429 + Retry-After once the per-key
//! cap is exceeded. TestApp config sets every limit to 3/min so the
//! loops stay short.

mod common;

use http_body_util::BodyExt;
use hyper::Method;
use tower::ServiceExt;

/// Pure math behind `common::align_rate_limit_window` (issue #6):
/// given the epoch seconds, how long must a test wait so its burst
/// starts with at least `margin` seconds left in the fixed window?
#[test]
fn alignment_wait_math() {
    // Plenty of headroom → no wait.
    assert_eq!(common::rate_limit_alignment_wait(0, 60, 10), 0);
    assert_eq!(common::rate_limit_alignment_wait(30, 60, 10), 0);
    // Exactly `margin` remaining still qualifies → no wait.
    assert_eq!(common::rate_limit_alignment_wait(50, 60, 10), 0);
    // Inside the margin → wait until the next window boundary.
    assert_eq!(common::rate_limit_alignment_wait(51, 60, 10), 9);
    assert_eq!(common::rate_limit_alignment_wait(59, 60, 10), 1);
    // Boundary itself is a fresh window → no wait.
    assert_eq!(common::rate_limit_alignment_wait(60, 60, 10), 0);
    assert_eq!(common::rate_limit_alignment_wait(3661, 60, 10), 0);
}

async fn dispatch_full(
    app: &common::TestApp,
    req: hyper::Request<axum::body::Body>,
) -> (u16, hyper::HeaderMap) {
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let _ = resp.into_body().collect().await;
    (status, headers)
}

#[tokio::test]
async fn auth_login_rate_limit_returns_429_with_retry_after() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Cap is 3/min, keyed by X-Forwarded-For. A single client gets 3
    // through and the 4th 429s.
    common::align_rate_limit_window().await;
    for i in 0..3 {
        let req = hyper::Request::builder()
            .method(Method::GET)
            .uri("/api/v1/auth/login/github")
            .header("X-Forwarded-For", "203.0.113.5")
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, _) = dispatch_full(&app, req).await;
        assert!(
            status < 500,
            "iter {i}: handler must not 5xx (real flow returns 302 redirect)"
        );
        assert_ne!(status, 429, "iter {i}: under-cap request must not 429");
    }

    let req = hyper::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/auth/login/github")
        .header("X-Forwarded-For", "203.0.113.5")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, headers) = dispatch_full(&app, req).await;
    assert_eq!(status, 429, "4th request from same IP must 429");
    let retry_after = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .expect("Retry-After must be set");
    let secs: u64 = retry_after.parse().unwrap();
    assert!(
        secs > 0 && secs <= 60,
        "Retry-After must be in (0, 60] seconds, got {secs}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn auth_login_rate_limit_isolates_distinct_ips() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Different IPs share the global counter only via Redis bucket
    // boundaries — the key includes the IP, so 4 requests from 4
    // distinct IPs all clear the limit.
    for ip in ["203.0.113.10", "203.0.113.11", "203.0.113.12", "203.0.113.13"] {
        let req = hyper::Request::builder()
            .method(Method::GET)
            .uri("/api/v1/auth/login/github")
            .header("X-Forwarded-For", ip)
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, _) = dispatch_full(&app, req).await;
        assert_ne!(
            status, 429,
            "IP {ip} must not be rate-limited (each IP has its own bucket)"
        );
    }

    app.cleanup().await;
}

#[tokio::test]
async fn auth_refresh_rate_limit_fires_on_repeated_attempts() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // A leaked refresh-token replay scenario. Each attempt sends an
    // empty body + no cookie → the handler reaches the rate-limit
    // check (which runs first) and returns 429 once the cap fires.
    // Below the cap the handler returns 401 (no credentials).
    common::align_rate_limit_window().await;
    for _ in 0..3 {
        let req = hyper::Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/refresh")
            .header("X-Forwarded-For", "203.0.113.20")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from("{}"))
            .unwrap();
        let (status, _) = dispatch_full(&app, req).await;
        assert_eq!(status, 401, "no creds → 401 below cap");
    }

    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/refresh")
        .header("X-Forwarded-For", "203.0.113.20")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let (status, _) = dispatch_full(&app, req).await;
    assert_eq!(
        status, 429,
        "4th refresh attempt from same IP must 429 (cap fires before credential check)"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn search_rate_limit_fires_per_user() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("ratelimit-search@test.com").await;

    common::align_rate_limit_window().await;
    for i in 0..3 {
        let (status, _) = app
            .json_request(
                Method::GET,
                "/api/v1/search?q=anything",
                Some(&token),
                None,
            )
            .await;
        assert!(
            status < 500,
            "iter {i}: search must not 5xx (200 with empty results, or 400 — both fine)"
        );
        assert_ne!(status, 429, "iter {i}: under-cap search must not 429");
    }

    let (status, _) = app
        .json_request(
            Method::GET,
            "/api/v1/search?q=anything",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 429, "4th search must 429");

    app.cleanup().await;
}

/// gap-002 from the post-hardening security audit: /users/search
/// used to be unrate-limited, so a scripted caller could walk past
/// the frontend's 250 ms debounce and enumerate the directory or
/// amplify per-hit workspace-scope GSI4 queries. This test pins
/// the fix — 4th call from the same user within the window 429s.
#[tokio::test]
async fn user_search_rate_limit_fires_per_user() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("ratelimit-user-search@test.com").await;

    common::align_rate_limit_window().await;
    for i in 0..3 {
        let (status, _) = app
            .json_request(
                Method::GET,
                "/api/v1/users/search?q=nobody",
                Some(&token),
                None,
            )
            .await;
        assert!(
            status < 500,
            "iter {i}: user-search must not 5xx (200 with empty results is fine)"
        );
        assert_ne!(status, 429, "iter {i}: under-cap user-search must not 429");
    }

    let (status, _) = app
        .json_request(
            Method::GET,
            "/api/v1/users/search?q=nobody",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 429, "4th user-search must 429");

    app.cleanup().await;
}

#[tokio::test]
async fn sharing_rate_limit_fires_on_mutation_loop() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Test config: rate_limit_sharing_per_min = 10, max_members_per
    // _folder = 3. We exercise the rate limit via PATCH on a single
    // existing member (update_folder_member is NOT bound by the
    // member cap) so the rate-limit fires without colliding with the
    // member-cap test in test_sharing.rs.
    let (_, owner_token) = app.create_user("ratelimit-sharing-owner@test.com").await;
    let folder_id = app
        .create_folder(&owner_token, "Sharing rate-limit folder", None)
        .await;
    let (target_id, _) = app.create_user("ratelimit-share-target@test.com").await;

    // 1 add (uses 1 of 10 budget) + 9 PATCHes (fills out to 10) = the
    // last PATCH succeeds. The 11th call (next PATCH) must 429.
    common::align_rate_limit_window().await;
    let add_body = serde_json::json!({ "userId": target_id, "accessLevel": "EDIT" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&owner_token),
            Some(add_body),
        )
        .await;
    assert_eq!(status, 204, "initial add must succeed");

    for i in 0..9 {
        let body = serde_json::json!({ "userId": target_id, "accessLevel": "VIEW" });
        let (status, _) = app
            .json_request(
                Method::POST,
                &format!("/api/v1/folders/{folder_id}/members"),
                Some(&owner_token),
                Some(body),
            )
            .await;
        // POST on an existing member updates rather than re-adds, so
        // doesn't trip the member cap.
        assert!(
            status < 400,
            "iter {i}: under-cap update must succeed (status was {status})"
        );
    }

    // 11th sharing mutation — rate-limit fires.
    let body = serde_json::json!({ "userId": target_id, "accessLevel": "EDIT" });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&owner_token),
            Some(body),
        )
        .await;
    assert_eq!(status, 429, "11th sharing mutation must 429");
    assert_eq!(json["error"], "rate_limited");
    let msg = json["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("Rate limit exceeded for sharing"),
        "message must identify rate-limit (not member-cap), got {msg:?}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn admin_mut_rate_limit_fires_on_repeated_promotes() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Test config: rate_limit_admin_mut_per_min = 10. Loop a single
    // admin actor through promote on a peer 11 times — the 11th must
    // 429. Promote is idempotent at the repo layer (just writes
    // role=Admin again), so we don't have to alternate.
    let (admin_id, _) = app.create_user("ratelimit-admin@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("ratelimit-admin@test.com").await;
    let (target_id, _) = app.create_user("ratelimit-target@test.com").await;

    common::align_rate_limit_window().await;
    for i in 0..10 {
        let (status, _) = app
            .json_request(
                Method::POST,
                &format!("/api/v1/admin/users/{target_id}/promote"),
                Some(&admin_token),
                None,
            )
            .await;
        assert!(
            status < 400,
            "iter {i}: under-cap promote must succeed (status was {status})"
        );
    }

    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{target_id}/promote"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 429, "11th promote must 429");
    let msg = json["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("admin_mut") || msg.to_lowercase().contains("rate"),
        "message must identify rate-limit, got {msg:?}"
    );

    app.cleanup().await;
}

// ─── M-E7 item 10 rate-limit coverage gaps ────────────────────────
//
// One test per new rate-limit. The dev_login consolidation has no
// dedicated test here — it now uses the same shared module exercised
// by the auth_login / auth_refresh tests above, and the production
// cap (100/min) is unchanged from the prior DashMap implementation.

/// Test config: rate_limit_comments_per_min = 5. The budget is
/// shared between create_thread and add_message (one helper, one
/// scope), so a typical chatty session that creates a thread and
/// rapid-fires messages can exhaust the cap from either entry.
/// 1 create_thread + 4 add_message = 5 budget used; the 6th call
/// (5th add_message) must 429.
#[tokio::test]
async fn comments_rate_limit_fires_across_thread_and_messages() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("ratelimit-comments@test.com").await;
    let doc_id = app.create_doc(&token, "Comments doc", None).await;

    // Burns 1 of 5.
    common::align_rate_limit_window().await;
    let (status, thread) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(serde_json::json!({ "threadType": "document" })),
        )
        .await;
    assert_eq!(status, 201, "thread create must succeed within budget");
    let thread_id = thread["thread"]["thread_id"]
        .as_str()
        .or_else(|| thread["threadId"].as_str())
        .or_else(|| thread["id"].as_str())
        .expect("thread id field");

    // 4 add_message calls — burn budget to 5 (at-cap, not over).
    for i in 0..4 {
        let (status, _) = app
            .json_request(
                Method::POST,
                &format!("/api/v1/threads/{thread_id}/messages"),
                Some(&token),
                Some(serde_json::json!({ "content": format!("msg {i}") })),
            )
            .await;
        assert!(
            status < 400,
            "iter {i}: under-cap message must succeed (status was {status})"
        );
    }

    // 6th comments call (5th add_message) — must 429.
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(serde_json::json!({ "content": "over the line" })),
        )
        .await;
    assert_eq!(status, 429, "6th comment write must 429");
    assert_eq!(json["error"], "rate_limited");
    let msg = json["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("comments"),
        "message must identify the comments scope, got {msg:?}"
    );

    app.cleanup().await;
}

/// Test config: rate_limit_content_write_per_min = 5. Loop 5
/// PUT /content requests with valid Y.Doc bytes — the 6th must
/// 429. Uses the same OgreDoc fixture as test_put_content_roundtrip.
#[tokio::test]
async fn content_write_rate_limit_fires_on_repeated_puts() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("ratelimit-content@test.com").await;
    let doc_id = app.create_doc(&token, "Content rate-limit doc", None).await;
    let state_bytes = ogrenotes_collab::document::OgreDoc::new().to_state_bytes();

    common::align_rate_limit_window().await;
    for i in 0..5 {
        let (status, _) = app
            .bytes_request(
                Method::PUT,
                &format!("/api/v1/documents/{doc_id}/content"),
                Some(&token),
                state_bytes.clone(),
                "application/octet-stream",
            )
            .await;
        assert!(
            status < 400,
            "iter {i}: under-cap PUT /content must succeed (status was {status})"
        );
    }

    let (status, body) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            state_bytes,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 429, "6th PUT /content must 429");
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("content_write"),
        "body must identify the content_write scope, got {body_str:?}"
    );

    app.cleanup().await;
}

/// Test config: rate_limit_ws_upgrade_per_min = 5. The rate-limit
/// fires AFTER `validate_ws_token` so the budget is keyed on the
/// authenticated user_id (not on IP); we drive 6 valid WS upgrade
/// attempts with fresh ws_tokens each, and assert the 6th is the
/// one that 429s. Per-document and per-user CONNECTION caps fire
/// at different code paths (different test budget) and won't
/// shadow the rate-limit at these volumes (cap is 3 / 2, but
/// oneshot-mode requests never increment the room registry).
#[tokio::test]
async fn ws_upgrade_rate_limit_fires_on_repeated_handshakes() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("ratelimit-ws@test.com").await;
    let doc_id = app.create_doc(&token, "WS rate-limit doc", None).await;

    // Helper: mint a fresh ws_token (single-use via GETDEL) and
    // drive the upgrade handler in oneshot mode. Under cap, the
    // request fails post-rate-limit at the WebSocketUpgrade
    // extractor (the test transport has no real socket); the
    // observable contract is "not 429".
    async fn upgrade_once(app: &common::TestApp, token: &str, doc_id: &str) -> u16 {
        let (_, ws_token_json) = app
            .json_request(
                hyper::Method::POST,
                &format!("/api/v1/documents/{doc_id}/ws-token"),
                Some(token),
                None,
            )
            .await;
        let ws_token = ws_token_json["token"].as_str().unwrap().to_string();
        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(&format!("/api/v1/documents/{doc_id}/ws?token={ws_token}"))
            .header("Origin", "http://localhost:8080")
            .body(axum::body::Body::empty())
            .unwrap();
        app.raw_request(req).await.0
    }

    common::align_rate_limit_window().await;
    for i in 0..5 {
        let status = upgrade_once(&app, &token, &doc_id).await;
        assert_ne!(
            status, 429,
            "iter {i}: under-cap upgrade must not 429 (got {status})"
        );
    }

    let status = upgrade_once(&app, &token, &doc_id).await;
    assert_eq!(status, 429, "6th upgrade attempt must 429 from rate-limit");

    app.cleanup().await;
}

/// #83 — POST /auth/saml/acs is unauthenticated by necessity and
/// triggers DDB + Redis + XML parse work on every call. Without a
/// per-IP cap, a single source can degrade SAML SSO and amortize
/// DDB capacity used by the workspace's SAML config + user tables.
/// TestApp's `rate_limit_saml_acs_per_min = 3`; the 4th POST from
/// the same X-Forwarded-For must 429.
#[tokio::test]
async fn saml_acs_rate_limit_returns_429_per_source_ip() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // The handler's inner work fails fast (no workspace config; 401)
    // — we're asserting the rate-limit short-circuit fires before
    // the work even starts, so the body content is irrelevant.
    let body = "SAMLResponse=ignored&RelayState=ws-nonexistent";
    let mk_req = || {
        hyper::Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/saml/acs")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("X-Forwarded-For", "203.0.113.83")
            .body(axum::body::Body::from(body))
            .unwrap()
    };

    common::align_rate_limit_window().await;
    for i in 0..3 {
        let (status, _) = dispatch_full(&app, mk_req()).await;
        assert_ne!(
            status, 429,
            "iter {i}: under-cap ACS POST must not 429 (got {status})"
        );
    }

    let (status, headers) = dispatch_full(&app, mk_req()).await;
    assert_eq!(status, 429, "4th ACS POST from same IP must 429");
    let retry_after = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .expect("Retry-After must be set on 429");
    let secs: u64 = retry_after.parse().unwrap();
    assert!(
        secs > 0 && secs <= 60,
        "Retry-After must be in (0, 60] seconds, got {secs}"
    );

    app.cleanup().await;
}

/// #58/T-2: the per-user WS message limiter (scope `ws_message`) denies
/// once a user exceeds the budget in the window. This is the mechanism the
/// `ws.rs` recv loop calls (`ws_message_rate_limited`) before persisting an
/// `Update`/`SyncStep2`, closing the socket on breach so a client can't
/// flood the persist path at socket speed.
///
/// The in-loop gate itself needs a real TCP WebSocket, which the oneshot
/// test transport can't provide (see the note on ws_upgrade handler tests
/// in test_ws.rs), so we exercise the `enforce` path that backs it directly
/// with a low budget.
#[tokio::test]
async fn ws_message_rate_limit_denies_after_budget() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Unique per run: `check` is a fixed-window INCR whose Redis key
    // (`ratelimit:ws_message:<user>:<bucket>`) survives to the bucket
    // boundary. A hardcoded identifier would let a re-run within the same
    // 60s bucket start already over budget (shared local Redis), so derive
    // a fresh identifier each run.
    let user = format!(
        "ws-msg-rl-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let user = user.as_str();
    let limit = 3u64;

    common::align_rate_limit_window().await;
    for i in 0..limit {
        assert!(
            ogrenotes_api::middleware::rate_limit::enforce(
                &app.state.redis, "ws_message", user, limit, 60
            )
            .await
            .is_ok(),
            "frame {i} under budget must be allowed"
        );
    }
    // The next frame is over budget — the recv loop closes the socket here.
    assert!(
        ogrenotes_api::middleware::rate_limit::enforce(
            &app.state.redis, "ws_message", user, limit, 60
        )
        .await
        .is_err(),
        "frame over the per-user WS message budget must be denied"
    );

    app.cleanup().await;
}
