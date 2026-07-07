// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

#[tokio::test]
async fn test_ask_returns_503_without_api_key() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("asker@test.com").await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/ask",
            Some(&token),
            Some(serde_json::json!({"question": "What documents exist?"})),
        )
        .await;
    assert_eq!(status, 503);
    assert_eq!(json["error"].as_str().unwrap(), "service_unavailable");

    app.cleanup().await;
}

#[tokio::test]
async fn test_ask_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/ask",
            None,
            Some(serde_json::json!({"question": "test"})),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_ask_rejects_empty_question() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("empty@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/ask",
            Some(&token),
            Some(serde_json::json!({"question": ""})),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_ask_rejects_long_question() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("long@test.com").await;
    let long_question = "a".repeat(2001);

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/ask",
            Some(&token),
            Some(serde_json::json!({"question": long_question})),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}
