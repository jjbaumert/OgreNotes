// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.4 piece C — async-job status API integration tests.
//!
//! Exercises POST /api/v1/jobs (enqueue) and GET /api/v1/jobs/{id}
//! (poll) over the full HTTP stack. No worker consumes in this
//! harness, so an enqueued job stays in `pending` — that's enough to
//! verify the producer's status side-channel write and the poll read
//! path round-trip through the router. The consume/ack path has its
//! own coverage in test_worker_mode.rs.

mod common;

use hyper::Method;

#[tokio::test]
async fn enqueue_then_poll_returns_pending_status() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("jobs-enqueue@test.com").await;

    let body = serde_json::json!({ "label": "smoke-test" });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/jobs", Some(&token), Some(body))
        .await;
    assert_eq!(status, 202, "enqueue should return 202 Accepted: {json}");
    let job_id = json["jobId"]
        .as_str()
        .expect("response should carry jobId")
        .to_string();
    assert!(!job_id.is_empty());

    // No worker is running in this harness, so the job stays pending.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/jobs/{job_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200, "poll should return 200: {json}");
    assert_eq!(
        json["state"], "pending",
        "freshly-enqueued job should be pending: {json}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn poll_unknown_job_returns_404() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("jobs-404@test.com").await;

    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/jobs/does-not-exist",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 404, "unknown job id should 404: {json}");
    assert_eq!(json["error"], "not_found");

    app.cleanup().await;
}

#[tokio::test]
async fn enqueue_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let body = serde_json::json!({ "label": "no-auth" });
    let (status, _json) = app
        .json_request(Method::POST, "/api/v1/jobs", None, Some(body))
        .await;
    assert_eq!(status, 401, "enqueue without a token should be unauthorized");

    app.cleanup().await;
}

#[tokio::test]
async fn cross_user_poll_of_owned_job_returns_404() {
    // M-6.6 follow-up #85. A job enqueued by user A carries A's user
    // id as the polling owner; user B (an authenticated stranger who
    // somehow knows the job_id) gets 404 — not 403 — so the response
    // can't be used to confirm the job exists. A's own poll still
    // succeeds.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_a_id, token_a) = app.create_user("jobs-owner-a@test.com").await;
    let token_b = app.create_user_token("jobs-owner-b@test.com").await;

    // Enqueue an owned (ImportDocx) job directly via the producer —
    // bypasses the multipart upload route, which isn't what we're
    // testing. The s3_key/folder don't need to be valid since no
    // worker consumes in this harness.
    let producer = app.state.job_producer.as_ref().expect("producer wired");
    let job_id = producer
        .enqueue(ogrenotes_worker::Job::ImportDocx {
            s3_key: "imports/test/fake.docx".to_string(),
            title: "cross-user check".to_string(),
            folder_id: Some("folder-1".to_string()),
            owner_id: user_a_id.clone(),
        })
        .await
        .expect("enqueue should succeed");

    // User B polls — must 404, not 403.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/jobs/{job_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 404, "cross-user poll must 404: {json}");

    // User A's own poll succeeds.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/jobs/{job_id}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 200, "owner's own poll should succeed: {json}");
    assert_eq!(json["state"], "pending");

    app.cleanup().await;
}

#[tokio::test]
async fn enqueue_rejects_empty_label() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("jobs-empty@test.com").await;

    let body = serde_json::json!({ "label": "   " });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/jobs", Some(&token), Some(body))
        .await;
    assert_eq!(status, 400, "empty label should be a bad request: {json}");
    assert_eq!(json["error"], "bad_request");

    app.cleanup().await;
}
