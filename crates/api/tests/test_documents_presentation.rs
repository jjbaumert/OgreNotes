// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Task 1 (Presentations P1): `DocType::Presentation` end-to-end through the
//! `POST /documents` and `GET /documents/{id}` routes. No production route
//! change was needed for this — `create_document` already forwards
//! `req.doc_type` verbatim (`documents.rs:243`) — so this test exists purely
//! to pin the wire contract: `docType: "presentation"` round-trips through
//! DynamoDB exactly like the pre-existing variants.

mod common;

use hyper::Method;

#[tokio::test]
async fn test_create_and_get_presentation_document() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;

    // Create with docType: "presentation".
    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&token),
            Some(serde_json::json!({ "title": "Deck", "docType": "presentation" })),
        )
        .await;
    assert_eq!(status, 201, "create should succeed: {json}");
    assert_eq!(json["docType"], "presentation");
    let doc_id = json["id"].as_str().expect("create returns doc id").to_string();

    // GET the document meta (same route the frontend uses) — docType must
    // round-trip through DynamoDB, not just reflect the create response.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200, "get should succeed: {json}");
    assert_eq!(json["docType"], "presentation");

    app.cleanup().await;
}
