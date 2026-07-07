// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #148 — three-state `ask_policy` gate for `/api/v1/ask`.
//!
//! Pins:
//! - Default for newly-minted production users is `Disabled`
//!   (dev-login auto-opens to `SystemOrByok`; these tests bypass
//!   that path via the storage layer so the production default
//!   is exercised).
//! - `Disabled` returns 403 with a useful message.
//! - `SystemOnly` allows the system-key path (200) but rejects
//!   requests carrying an `x-anthropic-key` header (400).
//! - `SystemOrByok` allows both paths.
//! - Admin bypasses `Disabled` outright, AND bypasses the
//!   `SystemOnly + BYOK` rejection.
//! - Admin routes: `PUT /admin/users/:id/ask-policy` accepts the
//!   three policy names; non-admins get 403 on both GET and PUT.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use http_body_util::BodyExt;
use hyper::{Method, Request};
use tower::ServiceExt;

use ogrenotes_api::claude::{
    ClaudeError, ClaudeMessages, Message, MessagesResponse, ResponseBlock, Tool,
};
use ogrenotes_storage::models::user::AskPolicy;

struct ConstantClaude;

#[async_trait]
impl ClaudeMessages for ConstantClaude {
    async fn messages(
        &self,
        _system: &str,
        _messages: &[Message],
        _tools: &[Tool],
        _max_tokens: u32,
    ) -> Result<MessagesResponse, ClaudeError> {
        Ok(MessagesResponse {
            content: vec![ResponseBlock::Text {
                text: "ok".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: None,
        })
    }
}

async fn ask_status(app: &common::TestApp, token: &str) -> u16 {
    ask_status_with_byok(app, token, None).await
}

async fn ask_status_with_byok(
    app: &common::TestApp,
    token: &str,
    byok_key: Option<&str>,
) -> u16 {
    let body = serde_json::json!({ "question": "anything" });
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ask")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"));
    if let Some(k) = byok_key {
        req = req.header("x-anthropic-key", k);
    }
    let req = req.body(Body::from(body.to_string())).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let _ = resp.into_body().collect().await;
    status
}

/// Force a specific `ask_policy` on the user via the storage
/// layer — bypasses dev-login's auto-open so the tests can
/// exercise each policy directly.
async fn force_ask_policy(app: &common::TestApp, user_id: &str, policy: AskPolicy) {
    app.state
        .user_repo
        .set_ask_policy(user_id, policy)
        .await
        .expect("set_ask_policy");
}

#[tokio::test]
async fn test_disabled_policy_returns_403() {
    common::require_infra!();
    let stub: Arc<dyn ClaudeMessages> = Arc::new(ConstantClaude);
    let app = common::TestApp::new_with_claude(Some(stub)).await;

    let (user_id, token) = app.create_user("gated@test.com").await;
    force_ask_policy(&app, &user_id, AskPolicy::Disabled).await;

    assert_eq!(
        ask_status(&app, &token).await,
        403,
        "user with policy=Disabled must be denied at the gate"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_system_only_allows_no_byok_and_rejects_byok() {
    common::require_infra!();
    let stub: Arc<dyn ClaudeMessages> = Arc::new(ConstantClaude);
    let app = common::TestApp::new_with_claude(Some(stub)).await;

    let (user_id, token) = app.create_user("systemonly@test.com").await;
    force_ask_policy(&app, &user_id, AskPolicy::SystemOnly).await;

    // No BYOK header → 200 (uses operator's key).
    assert_eq!(
        ask_status(&app, &token).await,
        200,
        "SystemOnly must allow the system-key path"
    );

    // BYOK header present → 400 (client-fixable request-shape error).
    assert_eq!(
        ask_status_with_byok(&app, &token, Some("sk-ant-user-key")).await,
        400,
        "SystemOnly must reject requests carrying x-anthropic-key"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_system_or_byok_allows_both_paths() {
    common::require_infra!();
    let stub: Arc<dyn ClaudeMessages> = Arc::new(ConstantClaude);
    let app = common::TestApp::new_with_claude(Some(stub)).await;

    let (user_id, token) = app.create_user("bothpaths@test.com").await;
    force_ask_policy(&app, &user_id, AskPolicy::SystemOrByok).await;

    assert_eq!(
        ask_status(&app, &token).await,
        200,
        "SystemOrByok must allow the system-key path"
    );
    assert_eq!(
        ask_status_with_byok(&app, &token, Some("sk-ant-user-key")).await,
        200,
        "SystemOrByok must allow the BYOK path"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_admin_can_set_ask_policy_across_states() {
    common::require_infra!();
    let stub: Arc<dyn ClaudeMessages> = Arc::new(ConstantClaude);
    let app = common::TestApp::new_with_claude(Some(stub)).await;

    let (user_id, token) = app.create_user("subject@test.com").await;
    force_ask_policy(&app, &user_id, AskPolicy::Disabled).await;
    assert_eq!(ask_status(&app, &token).await, 403, "baseline: Disabled");

    // Promote the admin caller directly through the repo (no
    // promote-API call needed for setup).
    let (admin_id, admin_token) = app.create_user("admin@test.com").await;
    app.state
        .user_repo
        .set_admin(&admin_id, true)
        .await
        .expect("set_admin");

    // Admin flips subject to SystemOnly via the admin route.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/admin/users/{user_id}/ask-policy"),
            Some(&admin_token),
            Some(serde_json::json!({ "policy": "system_only" })),
        )
        .await;
    assert_eq!(status, 204, "admin PUT ask-policy should be 204");

    // GET reflects the change.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/admin/users/{user_id}/ask-policy"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["policy"], "system_only");

    // Subject can now ask via system key but not via BYOK.
    assert_eq!(ask_status(&app, &token).await, 200);
    assert_eq!(
        ask_status_with_byok(&app, &token, Some("sk-ant-x")).await,
        400
    );

    // Admin flips back to Disabled; subject denied again.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/admin/users/{user_id}/ask-policy"),
            Some(&admin_token),
            Some(serde_json::json!({ "policy": "disabled" })),
        )
        .await;
    assert_eq!(status, 204);
    assert_eq!(
        ask_status(&app, &token).await,
        403,
        "after admin flip back to Disabled, gate is closed"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_non_admin_cannot_read_or_write_ask_policy() {
    common::require_infra!();
    let app = common::TestApp::new_with_claude(None).await;

    let (subject_id, _) = app.create_user("subject@test.com").await;
    let (_, attacker_token) = app.create_user("attacker@test.com").await;

    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/admin/users/{subject_id}/ask-policy"),
            Some(&attacker_token),
            Some(serde_json::json!({ "policy": "system_or_byok" })),
        )
        .await;
    assert_eq!(status, 403);

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/admin/users/{subject_id}/ask-policy"),
            Some(&attacker_token),
            None,
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_admin_bypasses_disabled_and_system_only_byok_rejection() {
    common::require_infra!();
    let stub: Arc<dyn ClaudeMessages> = Arc::new(ConstantClaude);
    let app = common::TestApp::new_with_claude(Some(stub)).await;

    let (admin_id, admin_token) = app.create_user("selftest@test.com").await;
    app.state
        .user_repo
        .set_admin(&admin_id, true)
        .await
        .expect("set_admin");

    // Admin bypasses Disabled.
    force_ask_policy(&app, &admin_id, AskPolicy::Disabled).await;
    assert_eq!(
        ask_status(&app, &admin_token).await,
        200,
        "is_admin must bypass Disabled"
    );

    // Admin bypasses the SystemOnly + BYOK rejection too — an
    // admin is the operator, they can pay their own bill if they
    // choose. Same policy, BYOK header carried.
    force_ask_policy(&app, &admin_id, AskPolicy::SystemOnly).await;
    assert_eq!(
        ask_status_with_byok(&app, &admin_token, Some("sk-ant-admin")).await,
        200,
        "is_admin must bypass the SystemOnly-BYOK rejection"
    );

    app.cleanup().await;
}
