// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for `POST /api/v1/client-telemetry`.
//!
//! The handler's internals (`project_counters`, the allowlist, label
//! truncation) have inline unit tests, but the HTTP surface had none — the
//! auth gate, the 16 KiB body cap, the 200-entry batch cap, and the
//! allowlist-rejection path were all unreachable from the unit tests. These
//! exercise the endpoint end-to-end.

mod common;

use hyper::Method;

const PATH: &str = "/api/v1/client-telemetry";
/// First entry of `CLIENT_METRIC_ALLOWLIST` in the handler.
const ALLOWLISTED: &str = "client.editor.transactions_total";

/// Unauthenticated requests are rejected (the endpoint is behind `AuthUser`).
#[tokio::test]
async fn client_telemetry_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(
            Method::POST,
            PATH,
            None,
            Some(serde_json::json!({ "counters": [{ "name": ALLOWLISTED, "delta": 1 }] })),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

/// A well-formed batch of allowlisted counters is accepted (204).
#[tokio::test]
async fn client_telemetry_accepts_valid_batch() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("telemetry-ok@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            PATH,
            Some(&token),
            Some(serde_json::json!({
                "counters": [
                    { "name": ALLOWLISTED, "delta": 3 },
                    { "name": "client.ws.frames_sent_total", "delta": 1 },
                ]
            })),
        )
        .await;
    assert_eq!(status, 204);

    app.cleanup().await;
}

/// A metric name not on the allowlist rejects the whole batch (400) — the
/// guard that stops a client from injecting arbitrary server-side metric
/// series.
#[tokio::test]
async fn client_telemetry_rejects_unknown_metric() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("telemetry-evil@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            PATH,
            Some(&token),
            Some(serde_json::json!({ "counters": [{ "name": "client.evil.injected", "delta": 1 }] })),
        )
        .await;
    assert_eq!(status, 400, "an unknown metric name must reject the batch");

    app.cleanup().await;
}

/// A body over the 16 KiB cap is rejected before parsing (400). The size
/// gate runs ahead of JSON parsing, so an oversized body fails fast.
#[tokio::test]
async fn client_telemetry_rejects_oversize_body() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("telemetry-big@test.com").await;

    // A single counter whose name alone exceeds 16 KiB.
    let huge_name = "a".repeat(17 * 1024);
    let (status, _) = app
        .json_request(
            Method::POST,
            PATH,
            Some(&token),
            Some(serde_json::json!({ "counters": [{ "name": huge_name, "delta": 1 }] })),
        )
        .await;
    assert_eq!(status, 400, "a batch over the 16 KiB cap must be rejected");

    app.cleanup().await;
}

/// A batch with more than 200 entries is rejected (400) even when every
/// entry is otherwise valid — the per-batch entry cap. Kept under the body
/// size cap so the entry-count gate is what fires.
#[tokio::test]
async fn client_telemetry_rejects_too_many_entries() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("telemetry-many@test.com").await;

    let counters: Vec<serde_json::Value> = (0..201)
        .map(|_| serde_json::json!({ "name": ALLOWLISTED, "delta": 1 }))
        .collect();
    let (status, _) = app
        .json_request(
            Method::POST,
            PATH,
            Some(&token),
            Some(serde_json::json!({ "counters": counters })),
        )
        .await;
    assert_eq!(status, 400, "a batch over 200 entries must be rejected");

    app.cleanup().await;
}
