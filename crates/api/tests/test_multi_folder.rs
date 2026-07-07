// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// #149: multi-folder document membership. Integration coverage for the
// add/remove/list endpoints, the access union across folders, that delete
// purges every folder edge, and that Move leaves additional memberships
// intact.

mod common;

use hyper::Method;

/// True if `doc_id` appears as a child of `folder_id` (reads GET /folders/:id).
async fn folder_contains(
    app: &common::TestApp,
    token: &str,
    folder_id: &str,
    doc_id: &str,
) -> bool {
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/folders/{folder_id}"),
            Some(token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    json["children"]
        .as_array()
        .map(|kids| kids.iter().any(|c| c["childId"].as_str() == Some(doc_id)))
        .unwrap_or(false)
}

async fn share_folder(
    app: &common::TestApp,
    owner_token: &str,
    folder_id: &str,
    user_id: &str,
    level: &str,
) {
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(owner_token),
            Some(serde_json::json!({ "userId": user_id, "accessLevel": level })),
        )
        .await;
    assert_eq!(status, 204);
}

#[tokio::test]
async fn add_to_second_folder_grants_access_via_either() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_alice, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    // Folder A holds the doc (primary); bob is a member of folder B only.
    let folder_a = app.create_folder(&token_a, "Folder A", None).await;
    let folder_b = app.create_folder(&token_a, "Folder B", None).await;
    share_folder(&app, &token_a, &folder_b, &bob_id, "EDIT").await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_a)).await;

    // Bob can't see it yet — it's only in folder A.
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 403, "bob is not in folder A");

    // Alice adds the doc to folder B.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/folders/{folder_b}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Now bob can see it — access unioned via folder B.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 200, "bob gains access via folder B");
    assert_eq!(json["id"], doc_id);

    // And it's listed under BOTH folders.
    assert!(folder_contains(&app, &token_a, &folder_a, &doc_id).await);
    assert!(folder_contains(&app, &token_a, &folder_b, &doc_id).await);

    app.cleanup().await;
}

#[tokio::test]
async fn remove_from_folder_revokes_only_that_path() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_alice, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    let folder_a = app.create_folder(&token_a, "Folder A", None).await;
    let folder_b = app.create_folder(&token_a, "Folder B", None).await;
    share_folder(&app, &token_a, &folder_b, &bob_id, "EDIT").await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_a)).await;

    // Add to B, bob can access.
    app.json_request(
        Method::PUT,
        &format!("/api/v1/documents/{doc_id}/folders/{folder_b}"),
        Some(&token_a),
        None,
    )
    .await;
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 200);

    // Remove from B → bob loses access; the doc still lives under A.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/folders/{folder_b}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 204);
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 403, "removing the B membership revokes bob's path");
    assert!(
        folder_contains(&app, &token_a, &folder_a, &doc_id).await,
        "doc remains under its primary folder"
    );
    assert!(!folder_contains(&app, &token_a, &folder_b, &doc_id).await);

    app.cleanup().await;
}

#[tokio::test]
async fn cannot_remove_primary_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_alice, token_a) = app.create_user("alice@test.com").await;
    let folder_a = app.create_folder(&token_a, "Folder A", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_a)).await;

    // Removing the primary via the multi-folder endpoint is refused.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/folders/{folder_a}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 400, "the primary folder can't be removed (use Move)");
    assert!(folder_contains(&app, &token_a, &folder_a, &doc_id).await);

    app.cleanup().await;
}

#[tokio::test]
async fn list_doc_folders_returns_primary_and_additional() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_alice, token_a) = app.create_user("alice@test.com").await;
    let folder_a = app.create_folder(&token_a, "Folder A", None).await;
    let folder_b = app.create_folder(&token_a, "Folder B", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_a)).await;

    app.json_request(
        Method::PUT,
        &format!("/api/v1/documents/{doc_id}/folders/{folder_b}"),
        Some(&token_a),
        None,
    )
    .await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/folders"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let folders = json.as_array().unwrap();
    assert_eq!(folders.len(), 2, "primary + one additional");
    let primary: Vec<&str> = folders
        .iter()
        .filter(|f| f["isPrimary"] == true)
        .filter_map(|f| f["id"].as_str())
        .collect();
    assert_eq!(primary, vec![folder_a.as_str()], "exactly the primary flagged");

    app.cleanup().await;
}

#[tokio::test]
async fn delete_purges_all_folder_edges() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_alice, token_a) = app.create_user("alice@test.com").await;
    let folder_a = app.create_folder(&token_a, "Folder A", None).await;
    let folder_b = app.create_folder(&token_a, "Folder B", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_a)).await;
    app.json_request(
        Method::PUT,
        &format!("/api/v1/documents/{doc_id}/folders/{folder_b}"),
        Some(&token_a),
        None,
    )
    .await;
    assert!(folder_contains(&app, &token_a, &folder_b, &doc_id).await);

    // Trash the doc → it must leave EVERY folder, not just the primary.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_a),
            None,
        )
        .await;
    assert!(status < 300, "delete should succeed, got {status}");
    assert!(
        !folder_contains(&app, &token_a, &folder_a, &doc_id).await,
        "purged from primary"
    );
    assert!(
        !folder_contains(&app, &token_a, &folder_b, &doc_id).await,
        "purged from the additional folder too"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn move_leaves_additional_membership_intact() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_alice, token_a) = app.create_user("alice@test.com").await;
    let folder_a = app.create_folder(&token_a, "Folder A", None).await;
    let folder_b = app.create_folder(&token_a, "Folder B", None).await;
    let folder_c = app.create_folder(&token_a, "Folder C", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_a)).await;

    // Add to B (additional), then Move the primary A → C.
    app.json_request(
        Method::PUT,
        &format!("/api/v1/documents/{doc_id}/folders/{folder_b}"),
        Some(&token_a),
        None,
    )
    .await;
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/move",
            Some(&token_a),
            Some(serde_json::json!({ "docIds": [doc_id], "destFolderId": folder_c })),
        )
        .await;
    assert!(status < 300, "bulk move should succeed, got {status}");

    // Primary is now C; the additional membership in B survives; A is gone.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/folders"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let ids: Vec<&str> = json
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["id"].as_str())
        .collect();
    assert!(ids.contains(&folder_c.as_str()), "primary moved to C");
    assert!(ids.contains(&folder_b.as_str()), "additional B membership intact");
    assert!(!ids.contains(&folder_a.as_str()), "old primary A dropped by Move");

    app.cleanup().await;
}

#[tokio::test]
async fn folder_membership_mutations_require_edit_not_view() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_alice, token_a) = app.create_user("alice@test.com").await;
    let (carol_id, token_c) = app.create_user("carol@test.com").await;
    let folder_a = app.create_folder(&token_a, "Folder A", None).await;
    let folder_b = app.create_folder(&token_a, "Folder B", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_a)).await;

    // Doc lives in A + B; Carol gets View on the DOC only (no folder access).
    app.json_request(
        Method::PUT,
        &format!("/api/v1/documents/{doc_id}/folders/{folder_b}"),
        Some(&token_a),
        None,
    )
    .await;
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&token_a),
            Some(serde_json::json!({ "userId": carol_id, "accessLevel": "VIEW" })),
        )
        .await;
    assert_eq!(status, 204);

    // View-only must not be able to ADD the doc to a folder...
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/folders/{folder_b}"),
            Some(&token_c),
            None,
        )
        .await;
    assert_eq!(status, 403, "View-only cannot add the doc to a folder");

    // ...nor REMOVE it from a (non-primary) folder.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/folders/{folder_b}"),
            Some(&token_c),
            None,
        )
        .await;
    assert_eq!(status, 403, "View-only cannot remove the doc from a folder");

    app.cleanup().await;
}

#[tokio::test]
async fn cannot_add_to_a_folder_the_caller_is_not_in() {
    // #149 review finding 1: a user with Edit on the doc must NOT be able to
    // add it to a folder they have no membership in (which would grant that
    // folder's members access via the union). Returns 404 (folder-id oracle
    // resistance), and creates no edge.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_alice, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_a = app.create_folder(&token_a, "Folder A", None).await; // doc primary
    let folder_x = app.create_folder(&token_a, "Folder X", None).await; // Alice's; Bob is NOT a member
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_a)).await;
    // Alice grants Bob Edit on the DOC (but no access to folder_x).
    app.json_request(
        Method::POST,
        &format!("/api/v1/documents/{doc_id}/members"),
        Some(&token_a),
        Some(serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" })),
    )
    .await;

    // Bob has Edit on the doc but isn't a member of folder_x → refused.
    // (folder_x isn't the primary, so this reaches the membership check rather
    // than the primary no-op guard.)
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/folders/{folder_x}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 404, "cannot add to a folder the caller isn't a member of");
    assert!(
        !folder_contains(&app, &token_a, &folder_x, &doc_id).await,
        "the refused add created no edge"
    );

    app.cleanup().await;
}
