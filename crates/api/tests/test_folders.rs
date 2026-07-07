// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

// ─── Helpers ───────────────────────────────────────────────────

async fn get_home_folder_id(app: &common::TestApp, token: &str) -> String {
    let (status, json) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(token), None)
        .await;
    assert_eq!(status, 200);
    json["homeFolderId"].as_str().unwrap().to_string()
}

// ─── Create ────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let body = serde_json::json!({ "title": "Work" });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/folders", Some(&token), Some(body))
        .await;

    assert_eq!(status, 201);
    assert!(json["id"].is_string());
    assert_eq!(json["title"], "Work");

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_folder_nested() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let parent_id = app.create_folder(&token, "Parent", None).await;

    let body = serde_json::json!({ "title": "Child", "parentId": parent_id });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/folders", Some(&token), Some(body))
        .await;

    assert_eq!(status, 201);
    assert_eq!(json["parentId"], parent_id);

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_folder_inaccessible_parent() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;
    let folder_a = app.create_folder(&token_a, "Private", None).await;

    let body = serde_json::json!({ "title": "Sneaky", "parentId": folder_a });
    let (status, _) = app
        .json_request(Method::POST, "/api/v1/folders", Some(&token_b), Some(body))
        .await;

    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_folder_unauthenticated() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let body = serde_json::json!({ "title": "Nope" });
    let (status, _) = app
        .json_request(Method::POST, "/api/v1/folders", None, Some(body))
        .await;

    assert_eq!(status, 401);

    app.cleanup().await;
}

// ─── Get ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_get_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Work", None).await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/folders/{folder_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 200);
    assert_eq!(json["id"], folder_id);
    assert_eq!(json["title"], "Work");
    assert!(json["children"].is_array());

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_folder_non_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;
    let folder_a = app.create_folder(&token_a, "Secret", None).await;

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/folders/{folder_a}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_folder_enriches_children() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Project", None).await;
    let _doc_id = app.create_doc(&token, "My Doc", Some(&folder_id)).await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/folders/{folder_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 200);
    let children = json["children"].as_array().unwrap();
    assert!(!children.is_empty());
    assert_eq!(children[0]["title"], "My Doc");

    app.cleanup().await;
}

// ─── Update ────────────────────────────────────────────────────

#[tokio::test]
async fn test_update_folder_title() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Old", None).await;

    let body = serde_json::json!({ "title": "New" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/folders/{folder_id}"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_update_folder_non_owner() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;
    let folder_a = app.create_folder(&token_a, "Mine", None).await;

    let body = serde_json::json!({ "title": "Hacked" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/folders/{folder_a}"),
            Some(&token_b),
            Some(body),
        )
        .await;

    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_update_system_folder_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let home_id = get_home_folder_id(&app, &token).await;

    let body = serde_json::json!({ "title": "Renamed Home" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/folders/{home_id}"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

// ─── Delete ────────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Temp", None).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_folder_non_owner() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;
    let folder_a = app.create_folder(&token_a, "Mine", None).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_a}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_system_folder_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let home_id = get_home_folder_id(&app, &token).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{home_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

// ─── Children ──────────────────────────────────────────────────

#[tokio::test]
async fn test_add_child_doc() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Target", None).await;
    let doc_id = app.create_doc(&token, "Loose Doc", None).await;

    let body = serde_json::json!({ "childId": doc_id, "childType": "doc" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/children"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 201);

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_child_invalid_type() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Target", None).await;

    let body = serde_json::json!({ "childId": "whatever", "childType": "invalid" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/children"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_remove_child() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Container", None).await;
    let doc_id = app.create_doc(&token, "Child Doc", Some(&folder_id)).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_id}/children/{doc_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

/// Regression: removing a child that doesn't belong to the folder should fail.
#[tokio::test]
async fn test_remove_child_not_in_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_a = app.create_folder(&token, "Folder A", None).await;
    let folder_b = app.create_folder(&token, "Folder B", None).await;
    let doc_id = app.create_doc(&token, "Doc in B", Some(&folder_b)).await;

    // Try to remove doc from folder_a (it's in folder_b)
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_a}/children/{doc_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 404, "Should not be able to remove a child that's not in this folder");

    app.cleanup().await;
}

/// Regression: remove_child should require Edit access (via check_folder_access).
#[tokio::test]
async fn test_remove_child_non_owner() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Alice's Folder", None).await;
    let doc_id = app.create_doc(&token_a, "Alice's Doc", Some(&folder_id)).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_id}/children/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 404, "Non-owner should not be able to remove children");

    app.cleanup().await;
}

// ─── Cycle detection on reparent ───────────────────────────────

/// Moving a folder under itself must be rejected — a folder as its own
/// parent would be an immediate cycle.
#[tokio::test]
async fn test_update_folder_parent_self_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Top", None).await;

    let body = serde_json::json!({ "parentId": folder_id });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/folders/{folder_id}"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 400, "Setting parent to self must 400");

    app.cleanup().await;
}

/// Moving a folder under one of its descendants must be rejected — that
/// would orphan the subtree into a cycle that's invisible to the list API
/// (parent pointers now form a ring).
#[tokio::test]
async fn test_update_folder_parent_descendant_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    // Root / Mid / Leaf — three-deep chain.
    let root = app.create_folder(&token, "Root", None).await;
    let mid = app.create_folder(&token, "Mid", Some(&root)).await;
    let leaf = app.create_folder(&token, "Leaf", Some(&mid)).await;

    // Try to make Root's parent be Leaf. That would put Root under its own
    // grandchild.
    let body = serde_json::json!({ "parentId": leaf });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/folders/{root}"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 400, "Descendant-as-parent must 400");

    app.cleanup().await;
}

/// `add_child` requires Edit access to the *child* being added, not just to
/// the target folder. A user who owns the folder but has no access to the
/// child document cannot attach it — otherwise folder ownership would become
/// a backdoor to referencing arbitrary docs. This gate was untested.
#[tokio::test]
async fn test_add_child_requires_access_to_child() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Alice owns a folder; Bob owns a doc that is NOT shared with Alice.
    let (_alice_id, alice_token) = app.create_user("addchild-alice@test.com").await;
    let (_bob_id, bob_token) = app.create_user("addchild-bob@test.com").await;
    let folder_id = app.create_folder(&alice_token, "Alice Folder", None).await;
    let bob_doc = app.create_doc(&bob_token, "Bob Private Doc", None).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/children"),
            Some(&alice_token),
            Some(serde_json::json!({ "childId": bob_doc, "childType": "doc" })),
        )
        .await;
    assert_eq!(
        status, 403,
        "owning the folder must not grant the right to attach a doc you can't edit"
    );

    app.cleanup().await;
}
