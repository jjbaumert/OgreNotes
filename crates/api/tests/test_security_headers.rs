// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for #35 — every response (including static
//! files, errors, and the auth-routed paths) carries the
//! defense-in-depth security headers stack.

mod common;

use http_body_util::BodyExt;
use hyper::Method;
use tower::ServiceExt;

async fn dispatch(
    app: &common::TestApp,
    method: Method,
    path: &str,
) -> (u16, hyper::HeaderMap) {
    let req = hyper::Request::builder()
        .method(method)
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let _ = resp.into_body().collect().await; // drain
    (status, headers)
}

fn header(headers: &hyper::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

#[tokio::test]
async fn health_endpoint_carries_full_security_header_stack() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, headers) = dispatch(&app, Method::GET, "/health").await;
    assert_eq!(status, 200, "health must respond");

    // CSP — verify load-bearing directives are present and that we
    // explicitly forbid framing.
    let csp = header(&headers, "content-security-policy").expect("CSP header missing");
    assert!(csp.contains("default-src 'self'"), "default-src missing: {csp}");
    assert!(
        csp.contains("frame-ancestors 'none'"),
        "frame-ancestors 'none' missing: {csp}"
    );
    assert!(
        csp.contains("script-src 'self' 'wasm-unsafe-eval'"),
        "Leptos WASM needs 'wasm-unsafe-eval': {csp}"
    );
    assert!(
        csp.contains("base-uri 'self'"),
        "base-uri 'self' missing — base-tag injection vector still open: {csp}"
    );

    // Other defense-in-depth headers
    assert_eq!(
        header(&headers, "x-content-type-options").as_deref(),
        Some("nosniff"),
        "X-Content-Type-Options must be nosniff"
    );
    assert_eq!(
        header(&headers, "x-frame-options").as_deref(),
        Some("DENY"),
        "X-Frame-Options must be DENY (legacy fallback for browsers older than CSP frame-ancestors)"
    );
    assert_eq!(
        header(&headers, "referrer-policy").as_deref(),
        Some("strict-origin-when-cross-origin"),
    );
    assert!(
        header(&headers, "permissions-policy")
            .map(|v| v.contains("camera=()") && v.contains("microphone=()") && v.contains("geolocation=()"))
            .unwrap_or(false),
        "Permissions-Policy must explicitly disable camera, microphone, and geolocation"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn hsts_omitted_in_dev_mode() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Test config has `dev_mode = true`. HSTS must be absent so a
    // developer running against http://localhost doesn't get their
    // browser pinned to HTTPS for a year.
    let (status, headers) = dispatch(&app, Method::GET, "/health").await;
    assert_eq!(status, 200);
    assert!(
        header(&headers, "strict-transport-security").is_none(),
        "HSTS must not be set in dev_mode (would brick localhost dev)"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn error_responses_also_carry_security_headers() {
    // A 401 from a missing-Bearer route must still ship the stack —
    // an attacker triggering errors shouldn't be able to bypass CSP
    // or get the page rendered without protections.
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, headers) = dispatch(&app, Method::GET, "/api/v1/users/me").await;
    assert_eq!(status, 401, "unauthenticated /users/me must 401");

    assert!(
        header(&headers, "content-security-policy").is_some(),
        "CSP must be present on error responses"
    );
    assert_eq!(
        header(&headers, "x-frame-options").as_deref(),
        Some("DENY"),
        "X-Frame-Options must be present on error responses"
    );

    app.cleanup().await;
}
