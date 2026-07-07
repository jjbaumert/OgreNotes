// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

// ─── Add member ────────────────────────────────────────────────

#[tokio::test]
async fn test_add_member_edit() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;

    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            Some(body),
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_member_own_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;

    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "OWN" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            Some(body),
        )
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_member_self_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;

    let body = serde_json::json!({ "userId": alice_id, "accessLevel": "EDIT" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            Some(body),
        )
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_member_nonexistent_user() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;

    let body = serde_json::json!({ "userId": "nonexistent-user-id", "accessLevel": "EDIT" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            Some(body),
        )
        .await;

    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_member_non_owner_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let (charlie_id, _) = app.create_user("charlie@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;

    // Share with Bob so he can see the folder
    let share_body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/folders/{folder_id}/members"),
        Some(&token_a),
        Some(share_body),
    )
    .await;

    // Bob (non-owner) tries to add Charlie
    let body = serde_json::json!({ "userId": charlie_id, "accessLevel": "EDIT" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_b),
            Some(body),
        )
        .await;

    assert_eq!(status, 403, "non-owner should not be able to add members");

    app.cleanup().await;
}

// ─── List members ──────────────────────────────────────────────

#[tokio::test]
async fn test_list_members_as_owner() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;

    // Add Bob as member
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/folders/{folder_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            None,
        )
        .await;

    assert_eq!(status, 200);
    let members = json["members"].as_array().unwrap();
    assert!(!members.is_empty());
    assert!(members.iter().any(|m| m["userId"] == bob_id));

    app.cleanup().await;
}

#[tokio::test]
async fn test_list_members_non_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (_, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Private", None).await;

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

// ─── Remove member ─────────────────────────────────────────────

#[tokio::test]
async fn test_remove_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;

    // Add then remove Bob
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/folders/{folder_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_id}/members/{bob_id}"),
            Some(&token_a),
            None,
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_remove_owner_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let folder_id = app.create_folder(&token_a, "Mine", None).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_id}/members/{alice_id}"),
            Some(&token_a),
            None,
        )
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

// ─── Access propagation ────────────────────────────────────────

#[tokio::test]
async fn test_sharing_grants_doc_access() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Shared Doc", Some(&folder_id)).await;

    // Share folder with Bob (VIEW)
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/folders/{folder_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    // Bob can GET the document
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 200);
    assert_eq!(json["id"], doc_id);

    app.cleanup().await;
}

#[tokio::test]
async fn test_sharing_grants_create_access() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Team", None).await;

    // Share folder with Bob (EDIT)
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/folders/{folder_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    // Bob can create a document in the shared folder
    let doc_body = serde_json::json!({ "title": "Bob's Doc", "folderId": folder_id });
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&token_b),
            Some(doc_body),
        )
        .await;

    assert_eq!(status, 201);

    app.cleanup().await;
}

#[tokio::test]
async fn test_remove_revokes_doc_access() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;

    // Share then remove
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/folders/{folder_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    app.json_request(
        Method::DELETE,
        &format!("/api/v1/folders/{folder_id}/members/{bob_id}"),
        Some(&token_a),
        None,
    )
    .await;

    // Bob can no longer access the doc
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

// ─── Direct document sharing ──────────────────────────────────

#[tokio::test]
async fn test_share_document_directly() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let doc_id = app.create_doc(&token_a, "Direct Share", None).await;

    // Bob cannot access yet
    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 403);

    // Alice shares directly with Bob
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    let (status, _) = app
        .json_request(Method::POST, &format!("/api/v1/documents/{doc_id}/members"), Some(&token_a), Some(body))
        .await;
    assert_eq!(status, 204);

    // Bob can now access
    let (status, json) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["id"], doc_id);

    app.cleanup().await;
}

#[tokio::test]
async fn test_share_document_duplicate_conflict() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let doc_id = app.create_doc(&token_a, "Dup Share", None).await;

    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(Method::POST, &format!("/api/v1/documents/{doc_id}/members"), Some(&token_a), Some(body.clone())).await;

    let (status, _) = app
        .json_request(Method::POST, &format!("/api/v1/documents/{doc_id}/members"), Some(&token_a), Some(body))
        .await;
    assert_eq!(status, 409);

    app.cleanup().await;
}

#[tokio::test]
async fn test_update_doc_member_access_level() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let doc_id = app.create_doc(&token_a, "Update Access", None).await;

    // Share with VIEW
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(Method::POST, &format!("/api/v1/documents/{doc_id}/members"), Some(&token_a), Some(body)).await;

    // Bob can view
    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 200);

    // Update to EDIT
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    let (status, _) = app
        .json_request(Method::PATCH, &format!("/api/v1/documents/{doc_id}/members/{bob_id}"), Some(&token_a), Some(body))
        .await;
    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_remove_doc_member_revokes_access() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let doc_id = app.create_doc(&token_a, "Revoke Share", None).await;

    // Share then remove
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(Method::POST, &format!("/api/v1/documents/{doc_id}/members"), Some(&token_a), Some(body)).await;
    app.json_request(Method::DELETE, &format!("/api/v1/documents/{doc_id}/members/{bob_id}"), Some(&token_a), None).await;

    // Bob can no longer access
    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_update_folder_member_access_level() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;

    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(Method::POST, &format!("/api/v1/folders/{folder_id}/members"), Some(&token_a), Some(body)).await;

    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    let (status, _) = app
        .json_request(Method::PATCH, &format!("/api/v1/folders/{folder_id}/members/{bob_id}"), Some(&token_a), Some(body))
        .await;
    assert_eq!(status, 204);

    app.cleanup().await;
}

// ─── Regression: restricted folders ───────────────────────────

/// Regression: a Restricted folder should block folder-member access to child documents.
/// Only direct document members should be able to access.
#[tokio::test]
async fn test_restricted_folder_blocks_folder_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    let folder_id = app.create_folder(&token_a, "Restricted", None).await;
    let doc_id = app.create_doc(&token_a, "Secret Doc", Some(&folder_id)).await;

    // Share folder with Bob
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    app.json_request(Method::POST, &format!("/api/v1/folders/{folder_id}/members"), Some(&token_a), Some(body)).await;

    // Bob can access the doc via folder membership
    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 200);

    // Set folder to Restricted
    let body = serde_json::json!({ "inheritMode": "restricted" });
    let (status, _) = app
        .json_request(Method::PATCH, &format!("/api/v1/folders/{folder_id}"), Some(&token_a), Some(body))
        .await;
    assert_eq!(status, 204);

    // Bob can NO LONGER access via folder membership
    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 403, "Restricted folder should block folder-member access");

    // But direct doc sharing still works
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(Method::POST, &format!("/api/v1/documents/{doc_id}/members"), Some(&token_a), Some(body)).await;

    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 200, "Direct doc sharing should still work with restricted folder");

    app.cleanup().await;
}

// ─── Regression: owner removal guard ──────────────────────────

/// Regression: attempting to remove the document owner as a doc member should fail.
#[tokio::test]
async fn test_remove_doc_owner_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token_a, "Owner Doc", None).await;

    // Try to remove Alice (the owner) as a member
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/members/{alice_id}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 400, "Should not be able to remove document owner");

    app.cleanup().await;
}

// ─── Regression: inherit_mode persistence ─────────────────────

/// Regression: setting inherit_mode via PATCH should actually persist.
#[tokio::test]
async fn test_inherit_mode_persists() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Test Mode", None).await;

    // Set to restricted
    let body = serde_json::json!({ "inheritMode": "restricted" });
    let (status, _) = app
        .json_request(Method::PATCH, &format!("/api/v1/folders/{folder_id}"), Some(&token), Some(body))
        .await;
    assert_eq!(status, 204);

    // Re-fetch and verify (inheriting from GET response — folder response doesn't include
    // inherit_mode, but we verify via the restricted-access behavior)
    // Create a doc in the folder
    let doc_id = app.create_doc(&token, "Restricted Doc", Some(&folder_id)).await;

    // Create a second user and share the folder
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    app.json_request(Method::POST, &format!("/api/v1/folders/{folder_id}/members"), Some(&token), Some(body)).await;

    // Bob should NOT be able to access the doc (folder is restricted)
    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 403, "Restricted mode should have persisted — folder member should be blocked");

    app.cleanup().await;
}

// ─── Regression: link settings ────────────────────────────────

/// Regression: link settings should be updatable via the dedicated endpoint.
#[tokio::test]
async fn test_update_link_settings() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Link Test", None).await;

    // Set link sharing to view mode
    let body = serde_json::json!({ "linkSharingMode": "view" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 204);

    // Read back
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["linkSharingMode"], "view");

    app.cleanup().await;
}

/// Phase 1: `viewOptions` round-trip alongside the mode. (Replaces the
/// former `linkSharingAllowExternal` round-trip — that field was inert
/// and is removed; OgreNotes link sharing is workspace-internal only,
/// with no external/public access.) The owner enables a View link with
/// two sub-options on; GET echoes them, and the un-set options stay
/// false. Enforcement of the flags is Phase 2 — this only checks
/// persistence + echo.
#[tokio::test]
async fn test_link_settings_view_options_roundtrip() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "View Options Link", None).await;

    let body = serde_json::json!({
        "linkSharingMode": "view",
        "viewOptions": {
            "allowComments": true,
            "showHistory": true,
        },
    });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 204);

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["linkSharingMode"], "view");
    // Enabled sub-options round-trip true …
    assert_eq!(json["viewOptions"]["allowComments"], true);
    assert_eq!(json["viewOptions"]["showHistory"], true);
    // … and the ones we didn't set default to false (not absent).
    assert_eq!(json["viewOptions"]["showConversation"], false);
    assert_eq!(json["viewOptions"]["allowRequestAccess"], false);
    // The owner can manage link settings.
    assert_eq!(json["canManage"], true);

    app.cleanup().await;
}

/// Phase 1: resetting all view-options back to false is honored. The
/// repo internally REMOVEs the sparse `link_view_options` attribute
/// (rather than writing a stale all-false blob), so this exercises the
/// SET+REMOVE update expression against real DynamoDB. The second PATCH
/// also omits `linkSharingMode`, so it must NOT emit a spurious
/// "disabled" audit row — but that absence isn't asserted here.
#[tokio::test]
async fn test_link_settings_reset_view_options() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Reset Opts", None).await;

    // Enable a sub-option (writes the attribute) …
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token),
            Some(serde_json::json!({
                "linkSharingMode": "view",
                "viewOptions": { "allowComments": true },
            })),
        )
        .await;
    assert_eq!(status, 204);

    // … then reset all options to false (viewOptions-only PATCH → REMOVE).
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token),
            Some(serde_json::json!({ "viewOptions": { "allowComments": false } })),
        )
        .await;
    assert_eq!(status, 204);

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["viewOptions"]["allowComments"], false);
    assert_eq!(json["viewOptions"]["showHistory"], false);

    app.cleanup().await;
}

/// Only the owner can change link-settings — an editor-level collaborator
/// cannot flip link sharing on or off. Protects against a rogue editor
/// silently publishing the doc.
#[tokio::test]
async fn test_link_settings_editor_cannot_patch() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;

    // Share folder with Bob as EDIT.
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/folders/{folder_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    // Bob tries to patch link settings — owner-only, expect 403.
    let body = serde_json::json!({ "linkSharingMode": "edit" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token_b),
            Some(body),
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

/// A View-level collaborator can READ link-settings (useful for UI that
/// shows "link sharing: on"), but not patch. Paired with the editor-patch
/// test above to lock the owner-only-write contract.
#[tokio::test]
async fn test_link_settings_viewer_can_get() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;

    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/folders/{folder_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 200);
    // A non-owner viewer can read settings but cannot manage them.
    assert_eq!(json["canManage"], false);

    app.cleanup().await;
}

// ─── Link sharing HTTP coverage ─────────────────────────────────
//
// Exercises the link-sharing branch in `check_doc_access`
// (crates/api/src/routes/documents.rs:294-307). Pre-existing tests
// only round-tripped link-sharing fields through the model layer; the
// HTTP path that grants access to a workspace member of a link-shared
// doc was untested. These tests close that gap end-to-end.

/// link_sharing_mode = View on a doc whose workspace_id is W must
/// grant View access to every member of W. Non-workspace users still
/// get 403.
#[tokio::test]
async fn test_link_sharing_view_grants_workspace_member_access() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let (_, token_c) = app.create_user("carol@test.com").await;

    // Alice creates a workspace and a doc, then moves the doc into the
    // workspace. (The create endpoint does not surface workspace_id, so
    // we set it directly via the repo — the same pattern used by
    // test_workspaces::test_set_workspace_id_updates_gsi_attr.)
    let body = serde_json::json!({ "name": "Team" });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/workspaces", Some(&token_a), Some(body))
        .await;
    assert_eq!(status, 201, "create workspace failed: {json}");
    let ws_id = json["id"].as_str().unwrap().to_string();

    let doc_id = app.create_doc(&token_a, "Link-shared", None).await;
    app.state
        .doc_repo
        .set_workspace_id(&doc_id, &ws_id)
        .await
        .expect("set_workspace_id");

    // Add Bob to the workspace.
    let body = serde_json::json!({ "userId": bob_id, "role": "member" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status, 204);

    // Bob without link sharing: still gets 403 (workspace membership
    // alone does not grant doc access — it only enables link sharing
    // when the doc opts in).
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(
        status, 403,
        "workspace member should NOT get doc access without link sharing"
    );

    // Alice enables link sharing at View.
    let body = serde_json::json!({ "linkSharingMode": "view" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status, 204);

    // Bob (workspace member) now reads the doc.
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 200, "workspace member should read link-shared doc");

    // Carol (not in the workspace) is still denied.
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_c),
            None,
        )
        .await;
    assert_eq!(
        status, 403,
        "non-workspace user must NOT get access via link sharing"
    );

    app.cleanup().await;
}

/// link_sharing_mode = View must NOT promote a workspace member to
/// Edit access. Mode=Edit must grant both View reads and Edit writes.
#[tokio::test]
async fn test_link_sharing_view_does_not_grant_edit_to_workspace_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({ "name": "Team" });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/workspaces", Some(&token_a), Some(body))
        .await;
    let ws_id = json["id"].as_str().unwrap().to_string();

    let doc_id = app.create_doc(&token_a, "Link mode test", None).await;
    app.state
        .doc_repo
        .set_workspace_id(&doc_id, &ws_id)
        .await
        .expect("set_workspace_id");

    let body = serde_json::json!({ "userId": bob_id, "role": "member" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/workspaces/{ws_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    // Alice enables link sharing at View.
    let body = serde_json::json!({ "linkSharingMode": "view" });
    app.json_request(
        Method::PATCH,
        &format!("/api/v1/documents/{doc_id}/link-settings"),
        Some(&token_a),
        Some(body),
    )
    .await;

    // Bob (workspace member, mode=View) attempts to write content —
    // the link-sharing branch in evaluate_doc_access must NOT promote
    // View to Edit. Expect 403.
    let doc = ogrenotes_collab::document::OgreDoc::new();
    let state = doc.to_state_bytes();
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token_b),
            state.clone(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(
        status, 403,
        "link_sharing_mode=view must not satisfy required=Edit"
    );

    // Alice escalates to Edit.
    let body = serde_json::json!({ "linkSharingMode": "edit" });
    app.json_request(
        Method::PATCH,
        &format!("/api/v1/documents/{doc_id}/link-settings"),
        Some(&token_a),
        Some(body),
    )
    .await;

    // Bob can now write content.
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token_b),
            state,
            "application/octet-stream",
        )
        .await;
    assert_eq!(
        status, 204,
        "link_sharing_mode=edit must grant write access to workspace member"
    );

    app.cleanup().await;
}

// ─── Membership caps (issue #34) ─────────────────────────────────
//
// TestApp config sets max_members_per_folder = 3 and
// max_members_per_doc = 3 so the test can hit the cap with a small
// number of fake users. Production defaults are 200.

#[tokio::test]
async fn folder_member_cap_returns_429_with_retry_after() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, owner_token) = app.create_user("folder-cap-owner@test.com").await;
    let folder_id = app.create_folder(&owner_token, "Crowded folder", None).await;

    // Fill the folder up to the test cap (3).
    for i in 0..3 {
        let (uid, _) = app
            .create_user(&format!("folder-cap-fill-{i}@test.com"))
            .await;
        let body = serde_json::json!({ "userId": uid, "accessLevel": "EDIT" });
        let (status, _) = app
            .json_request(
                Method::POST,
                &format!("/api/v1/folders/{folder_id}/members"),
                Some(&owner_token),
                Some(body),
            )
            .await;
        assert_eq!(status, 204, "first {} members must succeed", i + 1);
    }

    // The 4th distinct user trips the cap.
    let (overflow_id, _) = app.create_user("folder-cap-overflow@test.com").await;
    let body = serde_json::json!({ "userId": overflow_id, "accessLevel": "EDIT" });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&owner_token),
            Some(body),
        )
        .await;
    assert_eq!(status, 429, "at-cap folder add must return 429");
    assert_eq!(json["error"], "rate_limited");
    let msg = json["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("membership cap"),
        "message must explain the cap, got {msg:?}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn folder_member_update_at_cap_still_succeeds() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // The cap counts distinct members. Re-adding (i.e. updating access
    // level for) an existing member must NOT be blocked by the cap,
    // otherwise an at-cap folder becomes immutable for routine work.
    let (_, owner_token) = app.create_user("folder-update-owner@test.com").await;
    let folder_id = app.create_folder(&owner_token, "Crowded", None).await;

    let mut existing_id = String::new();
    for i in 0..3 {
        let (uid, _) = app
            .create_user(&format!("folder-update-fill-{i}@test.com"))
            .await;
        if i == 0 {
            existing_id = uid.clone();
        }
        let body = serde_json::json!({ "userId": uid, "accessLevel": "EDIT" });
        let (status, _) = app
            .json_request(
                Method::POST,
                &format!("/api/v1/folders/{folder_id}/members"),
                Some(&owner_token),
                Some(body),
            )
            .await;
        assert_eq!(status, 204);
    }

    // Update existing member's access level — the cap shouldn't fire.
    let body = serde_json::json!({ "userId": existing_id, "accessLevel": "VIEW" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&owner_token),
            Some(body),
        )
        .await;
    assert_eq!(
        status, 204,
        "re-adding an existing member must bypass the cap (membership stays at 3)"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn document_member_cap_returns_429() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, owner_token) = app.create_user("doc-cap-owner@test.com").await;
    let doc_id = app.create_doc(&owner_token, "Crowded doc", None).await;

    for i in 0..3 {
        let (uid, _) = app
            .create_user(&format!("doc-cap-fill-{i}@test.com"))
            .await;
        let body = serde_json::json!({ "userId": uid, "accessLevel": "EDIT" });
        let (status, _) = app
            .json_request(
                Method::POST,
                &format!("/api/v1/documents/{doc_id}/members"),
                Some(&owner_token),
                Some(body),
            )
            .await;
        assert_eq!(status, 204, "fill member {} must succeed", i);
    }

    let (overflow_id, _) = app.create_user("doc-cap-overflow@test.com").await;
    let body = serde_json::json!({ "userId": overflow_id, "accessLevel": "EDIT" });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&owner_token),
            Some(body),
        )
        .await;
    assert_eq!(status, 429);
    assert_eq!(json["error"], "rate_limited");

    app.cleanup().await;
}

// ─── Phase 2: view-option enforcement & request-access ──────────

/// Set up a *link-only viewer*: alice owns a doc in her default
/// workspace W; bob is a member of W (the link audience) but has no
/// durable grant on the doc, so his only path in is the workspace
/// View-mode link. Leaves the link mode = `view` with all sub-options
/// off. Returns `(alice_token, bob_token, doc_id)`.
async fn setup_workspace_link_viewer(
    app: &common::TestApp,
    suffix: &str,
) -> (String, String, String) {
    let (alice_id, alice_token) = app.create_user(&format!("alice-{suffix}@test.com")).await;
    let (bob_id, bob_token) = app.create_user(&format!("bob-{suffix}@test.com")).await;

    // alice's default workspace (seeded at dev-login).
    let ws_id = app
        .state
        .user_repo
        .get_by_id(&alice_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .expect("alice has a default workspace");

    // bob joins W as a plain member — link audience, no doc/folder grant.
    app.state
        .workspace_repo
        .add_member(&ogrenotes_storage::models::workspace::WorkspaceMember {
            workspace_id: ws_id.clone(),
            user_id: bob_id,
            role: ogrenotes_storage::models::WorkspaceRole::Member,
            joined_at: 0,
        })
        .await
        .unwrap();

    // Doc created *in W* so the link branch applies (§5.1.1: a doc with
    // no workspace has an inert link).
    let (_, doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&alice_token),
            Some(serde_json::json!({
                "title": "Linked",
                "docType": "document",
                "workspaceId": ws_id,
            })),
        )
        .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();

    // Turn the link on at view, all sub-options off.
    let (s, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&alice_token),
            Some(serde_json::json!({ "linkSharingMode": "view" })),
        )
        .await;
    assert_eq!(s, 204);

    (alice_token, bob_token, doc_id)
}

async fn set_view_option(app: &common::TestApp, token: &str, doc_id: &str, opt: &str, on: bool) {
    let (s, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(token),
            Some(serde_json::json!({ "viewOptions": { opt: on } })),
        )
        .await;
    assert_eq!(s, 204, "PATCH viewOptions {opt}={on}");
}

/// A View-mode link viewer may comment only when `allow_comments` is on.
#[tokio::test]
async fn test_link_view_comment_gated_by_allow_comments() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (alice_token, bob_token, doc_id) = setup_workspace_link_viewer(&app, "cmt").await;

    let comment = serde_json::json!({ "threadType": "document", "message": "hi" });

    // allow_comments off → link viewer cannot comment.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&bob_token),
            Some(comment.clone()),
        )
        .await;
    assert_eq!(s, 403, "view-link viewer blocked without allow_comments");

    set_view_option(&app, &alice_token, &doc_id, "allowComments", true).await;

    // … now allowed.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&bob_token),
            Some(comment),
        )
        .await;
    assert_eq!(s, 201, "view-link viewer may comment once allow_comments is on");

    app.cleanup().await;
}

/// A View-mode link viewer may read edit history only when
/// `show_history` is on (exercises `enforce_view_link_option`).
#[tokio::test]
async fn test_link_view_history_gated_by_show_history() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (alice_token, bob_token, doc_id) = setup_workspace_link_viewer(&app, "hist").await;

    let (s, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/versions"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 403, "view-link viewer blocked from history without show_history");

    set_view_option(&app, &alice_token, &doc_id, "showHistory", true).await;

    let (s, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/versions"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 200, "view-link viewer may read history once show_history is on");

    // The owner (durable member) always sees history, regardless of the flag.
    set_view_option(&app, &alice_token, &doc_id, "showHistory", false).await;
    let (s, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/versions"),
            Some(&alice_token),
            None,
        )
        .await;
    assert_eq!(s, 200, "durable member (owner) unaffected by show_history");

    app.cleanup().await;
}

/// request-access is offered only when the View-mode link enables
/// `allow_request_access`; the owner can't request access to their own doc.
#[tokio::test]
async fn test_request_access_offered_only_when_enabled() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (alice_token, bob_token, doc_id) = setup_workspace_link_viewer(&app, "req").await;

    // Not offered yet → 403.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/request-access"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 403, "request-access not offered until allow_request_access is on");

    set_view_option(&app, &alice_token, &doc_id, "allowRequestAccess", true).await;

    // Now bob may request.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/request-access"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 204, "viewer may request access once enabled");

    // The owner already owns it → 400.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/request-access"),
            Some(&alice_token),
            None,
        )
        .await;
    assert_eq!(s, 400, "owner cannot request access to own doc");

    app.cleanup().await;
}

/// #110: GET /documents/{id} carries `canRequestAccess` so the frontend
/// knows whether to show the viewer-facing "Request edit access" affordance.
/// True only for a view-only viewer once the owner enables
/// `allow_request_access`; never for the owner.
#[tokio::test]
async fn test_get_document_can_request_access_flag() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (alice_token, bob_token, doc_id) =
        setup_workspace_link_viewer(&app, "canreq").await;

    // View link with allow_request_access OFF → viewer is not offered.
    let (s, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        body["canRequestAccess"].as_bool(),
        Some(false),
        "not offered until allow_request_access is on",
    );

    set_view_option(&app, &alice_token, &doc_id, "allowRequestAccess", true).await;

    // Now the view-only viewer is offered the affordance.
    let (s, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        body["canRequestAccess"].as_bool(),
        Some(true),
        "view-only viewer offered once enabled",
    );

    // The owner already has full access → never offered.
    let (s, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&alice_token),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        body["canRequestAccess"].as_bool(),
        Some(false),
        "owner is not offered request-access",
    );

    app.cleanup().await;
}

/// gap-001: a View-mode link-only viewer can read comment threads only
/// when the link grants `show_conversation` or `allow_comments`.
#[tokio::test]
async fn test_link_view_comment_reads_gated() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (alice_token, bob_token, doc_id) = setup_workspace_link_viewer(&app, "cmtread").await;

    // View link, all sub-options off → link-only viewer can't read threads.
    let (s, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 403, "link-only viewer blocked from reading the conversation");

    // Enabling allow_comments opens the conversation to read (and post).
    set_view_option(&app, &alice_token, &doc_id, "allowComments", true).await;
    let (s, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 200, "allow_comments lets the link viewer read the conversation");

    app.cleanup().await;
}

/// Phase 3: a global admin can override a doc's link settings; the change
/// is recorded as a `LinkSharingChanged` SecurityAudit row keyed on the
/// doc *owner* (subject) with `actor_id` = the admin. Non-admins get 403.
#[tokio::test]
async fn test_admin_override_link_settings() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, alice_token) = app.create_user("ovr-alice@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Owned", None).await;

    // A non-admin cannot use the override.
    let (_bob_id, bob_token) = app.create_user("ovr-bob@test.com").await;
    let (s, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/admin/documents/{doc_id}/link-settings"),
            Some(&bob_token),
            Some(serde_json::json!({ "linkSharingMode": "view" })),
        )
        .await;
    assert_eq!(s, 403, "non-admin cannot override link settings");

    // Promote a global admin and override alice's doc. (The token is
    // minted before set_admin, but AuthUser reads is_admin from the live
    // row on every request, so it carries admin authority regardless.)
    let (admin_id, admin_token) = app.create_user("ovr-admin@test.com").await;
    app.state.user_repo.set_admin(&admin_id, true).await.unwrap();

    let (s, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/admin/documents/{doc_id}/link-settings"),
            Some(&admin_token),
            Some(serde_json::json!({
                "linkSharingMode": "view",
                "viewOptions": { "allowComments": true },
            })),
        )
        .await;
    assert_eq!(s, 204, "admin override succeeds");

    // The change is visible to the owner.
    let (s, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&alice_token),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(json["linkSharingMode"], "view");
    assert_eq!(json["viewOptions"]["allowComments"], true);

    // SecurityAudit row: subject = owner (alice), actor = admin.
    let mut found = None;
    for _ in 0..10 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(&alice_id, 20)
            .await
            .unwrap();
        if let Some(r) = rows.into_iter().find(|r| {
            matches!(
                &r.action,
                ogrenotes_storage::models::security_audit::SecurityAuditAction::LinkSharingChanged {
                    doc_id: d, mode: Some(m), ..
                } if d == &doc_id && *m == ogrenotes_storage::models::LinkSharingMode::View
            )
        }) {
            found = Some(r);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let row = found.expect("admin override writes a LinkSharingChanged row for the owner");
    assert_eq!(row.user_id, alice_id, "subject is the doc owner");
    assert_eq!(row.actor_id, admin_id, "actor is the overriding admin");

    app.cleanup().await;
}

// ─── GET /documents/:id/members (list_doc_members) ──────────────

/// `GET /documents/:id/members` lists direct document members. The owner
/// (and any member with View) can read it; the listing surfaces each
/// member's name/email/accessLevel. This endpoint had no coverage.
#[tokio::test]
async fn test_list_doc_members_returns_shared_members() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_alice_id, alice_token) = app.create_user("ldm-alice@test.com").await;
    let (bob_id, bob_token) = app.create_user("ldm-bob@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Owned", None).await;

    // Share with Bob at EDIT.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" })),
        )
        .await;
    assert_eq!(s, 204);

    // Owner can list and sees Bob with his email + level.
    let (s, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&alice_token),
            None,
        )
        .await;
    assert_eq!(s, 200);
    let members = json["members"].as_array().expect("members array");
    let bob = members
        .iter()
        .find(|m| m["userId"] == bob_id)
        .expect("Bob appears in the member list");
    assert_eq!(bob["accessLevel"], "EDIT");
    assert_eq!(bob["email"], "ldm-bob@test.com");

    // The shared member (View access suffices) can also list.
    let (s, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 200, "a member with at least View can list members");

    app.cleanup().await;
}

/// A user with no access to the document cannot enumerate its members —
/// `list_doc_members` requires View, which a stranger lacks. The access
/// check denies with 403 Forbidden here.
#[tokio::test]
async fn test_list_doc_members_denied_to_non_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_alice_id, alice_token) = app.create_user("ldm2-alice@test.com").await;
    let (_carol_id, carol_token) = app.create_user("ldm2-carol@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Private", None).await;

    let (s, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&carol_token),
            None,
        )
        .await;
    assert_eq!(s, 403, "a non-member cannot list members");

    app.cleanup().await;
}
