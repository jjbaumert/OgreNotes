// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P6 piece D — integration tests for the embed-resolver
//! endpoint.
//!
//! The route wraps `embed_allowlist::validate_url`; the allowlist
//! itself has 15 unit tests in the api crate. These cover the HTTP
//! shape: auth gate, the three EmbedRejection → 400 mappings, and
//! the happy 200 with iframe-ready src + provider + height.

use axum::http::Method;
use serde_json::json;

mod common;

#[tokio::test]
async fn test_embed_resolve_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/embeds/resolve",
            None,
            Some(json!({ "url": "https://www.youtube.com/watch?v=abc" })),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_embed_resolve_youtube_rewrites_watch_to_embed() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("embed-yt@test.com").await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/embeds/resolve",
            Some(&token),
            Some(json!({ "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ" })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["provider"].as_str().unwrap(), "youtube");
    // ddc03be added the privacy-enhanced rewrite (apply_privacy) and the
    // harness runs with embed_youtube_nocookie: true — matching the
    // production default (EMBED_YOUTUBE_NOCOOKIE defaults to "true") —
    // so the resolved src is the youtube-nocookie.com host. Both
    // apply_privacy branches (enabled / disabled / non-YouTube) are
    // covered by unit tests in embed_allowlist.rs.
    assert_eq!(
        json["src"].as_str().unwrap(),
        "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
    );
    assert_eq!(json["height"].as_u64().unwrap(), 315);

    app.cleanup().await;
}

#[tokio::test]
async fn test_embed_resolve_vimeo_rewrites_to_player() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("embed-vimeo@test.com").await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/embeds/resolve",
            Some(&token),
            Some(json!({ "url": "https://vimeo.com/76979871" })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["provider"].as_str().unwrap(), "vimeo");
    assert_eq!(
        json["src"].as_str().unwrap(),
        "https://player.vimeo.com/video/76979871",
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_embed_resolve_rejects_http() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("embed-http@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/embeds/resolve",
            Some(&token),
            Some(json!({ "url": "http://www.youtube.com/watch?v=abc" })),
        )
        .await;
    // Returns 400 with the "must use https://" reason.
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_embed_resolve_rejects_unknown_provider() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("embed-unk@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/embeds/resolve",
            Some(&token),
            Some(json!({ "url": "https://random.example.com/page" })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_embed_resolve_loom_share_rewrites_to_embed() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("embed-loom@test.com").await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/embeds/resolve",
            Some(&token),
            Some(json!({ "url": "https://www.loom.com/share/abc123def" })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["provider"].as_str().unwrap(), "loom");
    assert_eq!(
        json["src"].as_str().unwrap(),
        "https://www.loom.com/embed/abc123def",
    );

    app.cleanup().await;
}
