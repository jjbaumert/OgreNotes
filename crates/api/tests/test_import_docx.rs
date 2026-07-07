// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.5 piece C — async DOCX import-job route tests.
//!
//! Covers the request-path half: multipart upload → S3 stage →
//! enqueue → 202 + jobId, plus the validation gates. No worker
//! consumes in this harness, so the enqueued job stays `pending`;
//! the parse/persist half is unit-tested in
//! `ogrenotes_collab::import_docx` and exercised end-to-end once the
//! DOCX export writer (piece B) lands for the round-trip (piece E).
//! The route never parses the upload, so a dummy `.docx` body is
//! enough to drive it.

mod common;

use hyper::Method;

/// Build a minimal multipart/form-data body carrying a single file field.
fn multipart_body(boundary: &str, filename: &str, content_type: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

/// Multipart body whose file field carries NO `filename` attribute —
/// the case that would otherwise skip the .docx extension check.
fn multipart_body_no_filename(boundary: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"\r\n");
    out.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    out.extend_from_slice(data);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

#[tokio::test]
async fn import_job_enqueues_and_polls_pending() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("docx-import@test.com").await;

    let boundary = "docximportboundary";
    // The route stages bytes + enqueues without parsing, so any bytes
    // named *.docx drive the request path.
    let body = multipart_body(boundary, "report.docx", DOCX_MIME, b"PK\x03\x04 fake docx bytes");
    let (status, bytes) = app
        .bytes_request(
            Method::POST,
            "/api/v1/documents/import-job",
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 202, "import-job should accept: {}", String::from_utf8_lossy(&bytes));

    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
    let job_id = json["jobId"].as_str().expect("jobId in response").to_string();
    assert!(!job_id.is_empty());

    // No worker runs in this harness, so the job stays pending.
    let (status, poll) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/jobs/{job_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200, "poll should succeed: {poll}");
    assert_eq!(poll["state"], "pending", "freshly-enqueued job pending: {poll}");

    app.cleanup().await;
}

#[tokio::test]
async fn import_job_accepts_pdf() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("pdf-import@test.com").await;

    let boundary = "pdfimportboundary";
    // The route stages + enqueues without parsing; any bytes named *.pdf
    // drive the request path (the worker would do the real parse).
    let body = multipart_body(boundary, "report.pdf", "application/pdf", b"%PDF-1.5 fake");
    let (status, bytes) = app
        .bytes_request(
            Method::POST,
            "/api/v1/documents/import-job",
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 202, "pdf import-job should accept: {}", String::from_utf8_lossy(&bytes));

    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
    let job_id = json["jobId"].as_str().expect("jobId in response").to_string();
    assert!(!job_id.is_empty());

    let (status, poll) = app
        .json_request(Method::GET, &format!("/api/v1/jobs/{job_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 200, "poll should succeed: {poll}");
    assert_eq!(poll["state"], "pending", "freshly-enqueued job pending: {poll}");

    app.cleanup().await;
}

#[tokio::test]
async fn import_job_rejects_non_docx_filename() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("docx-badext@test.com").await;

    let boundary = "badextboundary";
    let body = multipart_body(boundary, "notes.txt", "text/plain", b"hello");
    let (status, _) = app
        .bytes_request(
            Method::POST,
            "/api/v1/documents/import-job",
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 400, "non-.docx filename should be rejected");

    app.cleanup().await;
}

#[tokio::test]
async fn import_job_rejects_missing_filename() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("docx-nofn@test.com").await;

    let boundary = "nofnboundary";
    let body = multipart_body_no_filename(boundary, b"PK\x03\x04 fake docx bytes");
    let (status, _) = app
        .bytes_request(
            Method::POST,
            "/api/v1/documents/import-job",
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 400, "a file field with no filename should be rejected");

    app.cleanup().await;
}

#[tokio::test]
async fn import_job_rejects_empty_file() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("docx-empty@test.com").await;

    let boundary = "emptyboundary";
    let body = multipart_body(boundary, "empty.docx", DOCX_MIME, b"");
    let (status, _) = app
        .bytes_request(
            Method::POST,
            "/api/v1/documents/import-job",
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 400, "empty upload should be rejected");

    app.cleanup().await;
}

#[tokio::test]
async fn import_job_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let boundary = "noauthboundary";
    let body = multipart_body(boundary, "report.docx", DOCX_MIME, b"PK fake");
    let (status, _) = app
        .bytes_request(
            Method::POST,
            "/api/v1/documents/import-job",
            None,
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 401, "import-job without a token should be unauthorized");

    app.cleanup().await;
}
