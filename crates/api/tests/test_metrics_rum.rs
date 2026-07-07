// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P9 piece C — integration tests for the frontend RUM
//! ingest endpoint.
//!
//! The handler validates page kind + each vital, classifies the
//! User-Agent into 3 buckets, and records histograms to the
//! in-process recorder. These tests cover the auth gate, the page-
//! kind validation, the vital sanity bounds, and the 204 happy
//! path; the histograms themselves are exercised by the unit tests
//! in `routes::metrics::tests`.

use axum::http::Method;
use serde_json::json;

mod common;

#[tokio::test]
async fn test_rum_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/metrics/rum",
            None,
            Some(json!({
                "page": "home",
                "vitals": { "lcp": 1200.0 }
            })),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_rum_accepts_known_page() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("rum-known@test.com").await;

    for page in ["home", "editor", "spreadsheet", "other"] {
        let (status, _) = app
            .json_request(
                Method::POST,
                "/api/v1/metrics/rum",
                Some(&token),
                Some(json!({
                    "page": page,
                    "vitals": { "lcp": 1234.5 }
                })),
            )
            .await;
        assert_eq!(status, 204, "page={page}");
    }

    app.cleanup().await;
}

#[tokio::test]
async fn test_rum_rejects_unknown_page() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("rum-unknown@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/metrics/rum",
            Some(&token),
            Some(json!({
                "page": "admin-console",
                "vitals": { "lcp": 100.0 }
            })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_rum_full_payload_204() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("rum-full@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/metrics/rum",
            Some(&token),
            Some(json!({
                "page": "editor",
                "vitals": {
                    "lcp": 1234.5,
                    "fcp": 567.0,
                    "inp": 80.0,
                    "cls": 0.05,
                    "navDcl": 800.0,
                    "navLoad": 1100.0
                }
            })),
        )
        .await;
    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_rum_empty_vitals_still_succeeds() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("rum-empty@test.com").await;

    // No vitals — beacon body shape is still legal; the handler
    // emits nothing and returns 204. Useful for the page-shape
    // case where a session loads but no observer ever fires.
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/metrics/rum",
            Some(&token),
            Some(json!({
                "page": "home",
                "vitals": {}
            })),
        )
        .await;
    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_rum_silently_drops_pathological_values() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("rum-bad@test.com").await;

    // 999 999 ms LCP and a negative INP are obvious junk; the
    // handler clamps via `finite_in_range`. The beacon still 204s
    // — one bad field doesn't drop the whole submission, and the
    // recorder simply doesn't get those samples.
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/metrics/rum",
            Some(&token),
            Some(json!({
                "page": "spreadsheet",
                "vitals": {
                    "lcp": 999999.0,
                    "fcp": 1500.0,
                    "inp": -10.0,
                    "cls": 100.0
                }
            })),
        )
        .await;
    assert_eq!(status, 204);

    app.cleanup().await;
}
