// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

#[tokio::test]
async fn test_create_and_list_relationships() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("reluser@test.com").await;
    let doc_a = app.create_doc(&token, "Source Doc", None).await;
    let doc_b = app.create_doc(&token, "Target Doc", None).await;

    // Create a relationship
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_a}/relationships"),
            Some(&token),
            Some(serde_json::json!({
                "targetDocId": doc_b,
                "relationType": "references"
            })),
        )
        .await;
    assert_eq!(status, 201);

    // List relationships
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_a}/relationships"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let rels = json.as_array().unwrap();
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0]["targetDocId"].as_str().unwrap(), doc_b);
    assert_eq!(rels[0]["relationType"].as_str().unwrap(), "references");

    app.cleanup().await;
}

#[tokio::test]
async fn test_relationship_prevents_self_reference() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("selfref@test.com").await;
    let doc_id = app.create_doc(&token, "Self Doc", None).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/relationships"),
            Some(&token),
            Some(serde_json::json!({
                "targetDocId": doc_id,
                "relationType": "references"
            })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_relationship() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("delrel@test.com").await;
    let doc_a = app.create_doc(&token, "Source", None).await;
    let doc_b = app.create_doc(&token, "Target", None).await;

    // Create
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_a}/relationships"),
            Some(&token),
            Some(serde_json::json!({
                "targetDocId": doc_b,
                "relationType": "depends-on"
            })),
        )
        .await;
    assert_eq!(status, 201);

    // Delete
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_a}/relationships/depends-on/{doc_b}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Verify gone
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_a}/relationships"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert!(json.as_array().unwrap().is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn test_relationship_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("authrel@test.com").await;
    let doc_id = app.create_doc(&token, "Auth Doc", None).await;

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/relationships"),
            None,
            None,
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_duplicate_relationship_returns_409() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("duprel@test.com").await;
    let doc_a = app.create_doc(&token, "Source", None).await;
    let doc_b = app.create_doc(&token, "Target", None).await;

    let body = serde_json::json!({
        "targetDocId": doc_b,
        "relationType": "references"
    });

    // First create succeeds
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_a}/relationships"),
            Some(&token),
            Some(body.clone()),
        )
        .await;
    assert_eq!(status, 201);

    // Second create of the same relationship should return 409 Conflict
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_a}/relationships"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 409, "Duplicate relationship should return 409 Conflict");

    app.cleanup().await;
}

// ─── Regression: no relationship mutation on trashed doc ──────

async fn trash_doc(app: &common::TestApp, token: &str, doc_id: &str) {
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(token),
            None,
        )
        .await;
    assert_eq!(status, 204, "failed to trash doc");
}

/// Creating a relationship whose source is a trashed doc must 404 — the
/// strict access check should reject the write before touching the repo.
#[tokio::test]
async fn test_create_relationship_on_trashed_source_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("reluser@test.com").await;
    let source = app.create_doc(&token, "Source", None).await;
    let target = app.create_doc(&token, "Target", None).await;
    trash_doc(&app, &token, &source).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{source}/relationships"),
            Some(&token),
            Some(serde_json::json!({
                "targetDocId": target,
                "relationType": "references"
            })),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

/// A trashed *target* must also be rejected. The handler calls
/// `check_doc_access(target_id)` with View access — trashed docs 404.
#[tokio::test]
async fn test_create_relationship_on_trashed_target_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("reluser@test.com").await;
    let source = app.create_doc(&token, "Source", None).await;
    let target = app.create_doc(&token, "Target", None).await;
    trash_doc(&app, &token, &target).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{source}/relationships"),
            Some(&token),
            Some(serde_json::json!({
                "targetDocId": target,
                "relationType": "references"
            })),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_relationship_on_trashed_source_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("reluser@test.com").await;
    let source = app.create_doc(&token, "Source", None).await;
    let target = app.create_doc(&token, "Target", None).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{source}/relationships"),
            Some(&token),
            Some(serde_json::json!({
                "targetDocId": target,
                "relationType": "references"
            })),
        )
        .await;
    assert_eq!(status, 201);

    trash_doc(&app, &token, &source).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{source}/relationships/references/{target}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

/// `list_relationships` filters out relationships whose *target* the caller
/// cannot view. A regression that dropped this filter would leak the
/// existence of unshared documents. Alice relates source→target, shares only
/// the source with Bob; Bob's listing of the source must omit the target.
#[tokio::test]
async fn test_list_relationships_filters_unviewable_targets() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_alice_id, alice_token) = app.create_user("rel-filter-alice@test.com").await;
    let (bob_id, bob_token) = app.create_user("rel-filter-bob@test.com").await;
    let source = app.create_doc(&alice_token, "Source", None).await;
    let target = app.create_doc(&alice_token, "Secret Target", None).await;

    // Alice relates source → target (she owns both).
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{source}/relationships"),
            Some(&alice_token),
            Some(serde_json::json!({ "targetDocId": target, "relationType": "references" })),
        )
        .await;
    assert_eq!(status, 201);

    // Share ONLY the source with Bob (View). The target stays private.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{source}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" })),
        )
        .await;
    assert_eq!(status, 204);

    // Bob can list the source's relationships but the unviewable target is
    // filtered out — he sees an empty list.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{source}/relationships"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert!(
        json.as_array().unwrap().is_empty(),
        "the relationship to an unviewable target must be filtered out, got: {json}"
    );

    // Alice (who can see the target) still sees the relationship.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{source}/relationships"),
            Some(&alice_token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let rels = json.as_array().unwrap();
    assert_eq!(rels.len(), 1, "owner sees the relationship");
    assert_eq!(rels[0]["targetDocId"].as_str().unwrap(), target);

    app.cleanup().await;
}
