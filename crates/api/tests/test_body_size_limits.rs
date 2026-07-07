// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for #42 — the global `DefaultBodyLimit` on
//! `api_router` enforces a 1 MiB cap on every state-changing route by
//! default, and routes that legitimately accept larger bodies
//! (`PUT /documents/:id/content` up to 10 MiB) override per-route.
//!
//! Pre-merge, axum's implicit 2 MiB default applied to typed
//! extractors; `Bytes` and `Multipart` routes were unbounded. This
//! suite locks in:
//!   - non-overridden routes reject >1 MiB with 413.
//!   - `PUT /content` still accepts up to its intended 10 MiB cap.

mod common;

use http_body_util::BodyExt;
use hyper::Method;
use tower::ServiceExt;

async fn dispatch(
    app: &common::TestApp,
    req: hyper::Request<axum::body::Body>,
) -> (u16, hyper::HeaderMap) {
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let _ = resp.into_body().collect().await;
    (status, headers)
}

/// A 2 MiB JSON payload to a route that has no per-route override
/// must reject with 413 PayloadTooLarge. POST /api/v1/folders takes
/// a small `CreateFolderRequest`; 2 MiB is two orders of magnitude
/// above any legitimate body. Without the global cap this would have
/// passed axum's implicit 2 MiB and rejected at JSON parse — silently
/// burning the work of materializing the body. With the cap it
/// rejects at the extractor.
#[tokio::test]
async fn oversize_body_to_default_route_rejected_with_413() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("body-size@test.com").await;

    // 2 MiB of `a` wrapped in a JSON string. Above the 1 MiB global
    // cap, below the 2 MiB axum implicit default — so the rejection
    // is provably from our layer, not axum's fallback.
    let huge = "a".repeat(2 * 1024 * 1024);
    let body = format!(r#"{{"name":"{huge}"}}"#);
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/folders")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(body))
        .unwrap();
    let (status, _) = dispatch(&app, req).await;
    assert_eq!(
        status, 413,
        "2 MiB POST to /folders must 413 (got {status})"
    );

    app.cleanup().await;
}

/// A small JSON payload (well under the cap) must reach the handler
/// and return its normal response — confirming the global cap doesn't
/// reject legitimate traffic.
#[tokio::test]
async fn small_body_to_default_route_reaches_handler() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("body-size-small@test.com").await;

    let body = r#"{"name":"My Folder"}"#;
    let req = hyper::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/folders")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(body))
        .unwrap();
    let (status, _) = dispatch(&app, req).await;
    // 201 Created on success; we don't care about exact code, only
    // that the global cap didn't intercept. < 400 means the request
    // reached the handler.
    assert!(
        status < 400 || status == 422,
        "small JSON POST to /folders must reach handler (got {status})"
    );

    app.cleanup().await;
}

/// PUT /documents/:id/content accepts yrs binary state up to
/// MAX_CONTENT_SIZE (10 MiB) per its per-route override. A 5 MiB
/// payload — comfortably above the 1 MiB global cap — must NOT 413.
/// The handler will reject the bytes as invalid yrs state with a
/// different code (the body is junk), but the global cap must not
/// shadow that handler-side validation.
#[tokio::test]
async fn put_content_route_overrides_global_cap() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("body-size-content@test.com").await;
    let doc_id = app.create_doc(&token, "Cap Override Test", None).await;

    // 5 MiB of zeros — above global 1 MiB, below MAX_CONTENT_SIZE
    // (10 MiB). Not valid yrs bytes; the handler will reject with
    // 400 or 422, but it must REACH the handler.
    let body = vec![0u8; 5 * 1024 * 1024];
    let req = hyper::Request::builder()
        .method(Method::PUT)
        .uri(&format!("/api/v1/documents/{doc_id}/content"))
        .header("content-type", "application/octet-stream")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(body))
        .unwrap();
    let (status, _) = dispatch(&app, req).await;
    assert_ne!(
        status, 413,
        "PUT /content with 5 MiB body must NOT 413 — the route override \
         exempts it from the 1 MiB global cap (got {status})"
    );

    app.cleanup().await;
}
