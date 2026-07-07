// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P7 piece A — integration tests for bulk delete +
//! restore.
//!
//! Covers: auth gate, > 100-id cap, all-success → 200, partial
//! failure → 207 with per-id status, restore round-trip.

use axum::http::Method;
use serde_json::json;

mod common;

#[tokio::test]
async fn test_bulk_delete_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/delete",
            None,
            Some(json!({ "docIds": [] })),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_delete_rejects_too_many_ids() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-del-cap@test.com").await;
    let ids: Vec<String> = (0..101).map(|n| format!("doc-{n}")).collect();
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/delete",
            Some(&token),
            Some(json!({ "docIds": ids })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_delete_all_success_returns_200() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-del-ok@test.com").await;
    let a = app.create_doc(&token, "alpha", None).await;
    let b = app.create_doc(&token, "beta", None).await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/delete",
            Some(&token),
            Some(json!({ "docIds": [a, b] })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["succeeded"].as_u64().unwrap(), 2);
    assert_eq!(json["failed"].as_u64().unwrap(), 0);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_delete_unknown_id_per_entry_404() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-del-mix@test.com").await;
    let real = app.create_doc(&token, "real", None).await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/delete",
            Some(&token),
            Some(json!({ "docIds": [real.clone(), "ghost-id"] })),
        )
        .await;
    assert_eq!(status, 207);
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let success_count = results
        .iter()
        .filter(|r| r["status"].as_u64() == Some(200))
        .count();
    let nf_count = results
        .iter()
        .filter(|r| r["status"].as_u64() == Some(404))
        .count();
    assert_eq!(success_count, 1);
    assert_eq!(nf_count, 1);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_restore_round_trips_through_trash() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-restore@test.com").await;
    let a = app.create_doc(&token, "rest-a", None).await;
    let b = app.create_doc(&token, "rest-b", None).await;

    // First soft-delete both via bulk.
    let (delete_status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/delete",
            Some(&token),
            Some(json!({ "docIds": [a.clone(), b.clone()] })),
        )
        .await;
    assert_eq!(delete_status, 200);

    // Fetch /users/me to get the home folder id for the restore target.
    let (me_status, me_json) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(me_status, 200);
    let home_id = me_json["homeFolderId"].as_str().unwrap().to_string();

    let (restore_status, restore_json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/restore",
            Some(&token),
            Some(json!({
                "docIds": [a, b],
                "targetFolderId": home_id,
            })),
        )
        .await;
    assert_eq!(restore_status, 200);
    assert_eq!(restore_json["succeeded"].as_u64().unwrap(), 2);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_restore_rejects_bad_target_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-rest-badtarget@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/restore",
            Some(&token),
            Some(json!({
                "docIds": ["any-id"],
                "targetFolderId": "does-not-exist",
            })),
        )
        .await;
    // Up-front folder check fires before walking the doc list.
    assert!(status == 404 || status == 403, "got {status}");

    app.cleanup().await;
}

// ─── Bulk move (M-P7 piece B / D) ────────────────────────────────

#[tokio::test]
async fn test_bulk_move_moves_into_dest_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-move@test.com").await;
    let doc_a = app.create_doc(&token, "move-a", None).await;
    let doc_b = app.create_doc(&token, "move-b", None).await;

    // Create a destination folder via /folders, then move both
    // docs into it.
    let (folder_status, folder_json) = app
        .json_request(
            Method::POST,
            "/api/v1/folders",
            Some(&token),
            Some(json!({ "title": "Destination" })),
        )
        .await;
    assert_eq!(folder_status, 201);
    let dest_folder = folder_json["id"].as_str().unwrap().to_string();

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/move",
            Some(&token),
            Some(json!({
                "docIds": [doc_a.clone(), doc_b.clone()],
                "destFolderId": dest_folder.clone(),
            })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["succeeded"].as_u64().unwrap(), 2);
    assert_eq!(json["failed"].as_u64().unwrap(), 0);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_move_rejects_bad_dest_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-move-bad@test.com").await;
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/move",
            Some(&token),
            Some(json!({
                "docIds": ["whatever"],
                "destFolderId": "no-such-folder",
            })),
        )
        .await;
    assert!(status == 404 || status == 403, "got {status}");

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_move_rejects_too_many_ids() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-move-cap@test.com").await;
    let ids: Vec<String> = (0..101).map(|n| format!("doc-{n}")).collect();
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/move",
            Some(&token),
            Some(json!({
                "docIds": ids,
                "destFolderId": "ignored-too-large-anyway",
            })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

// ─── Bulk share (M-P7 piece B / D) ───────────────────────────────

#[tokio::test]
async fn test_bulk_share_rejects_grant_own() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let owner = app.create_user_token("bulk-share-grantown@test.com").await;
    let target = app.create_user_token("bulk-share-target@test.com").await;
    // Recipient id is derived from a /users/me round-trip on the
    // target's session. The response carries the user id under the
    // camelCase `userId` field (see UserResponse in routes/users.rs).
    let (_, me) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&target), None)
        .await;
    let target_id = me["userId"].as_str().unwrap().to_string();

    let doc = app.create_doc(&owner, "share-grantown", None).await;
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/share",
            Some(&owner),
            Some(json!({
                "docIds": [doc],
                "memberId": target_id,
                "accessLevel": "own",
            })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_share_rejects_share_with_self() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-share-self@test.com").await;
    let (_, me) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    let self_id = me["userId"].as_str().unwrap().to_string();

    let doc = app.create_doc(&token, "share-self", None).await;
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/share",
            Some(&token),
            Some(json!({
                "docIds": [doc],
                "memberId": self_id,
                "accessLevel": "view",
            })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_share_rejects_unknown_recipient() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-share-nouser@test.com").await;
    let doc = app.create_doc(&token, "share-nouser", None).await;
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/share",
            Some(&token),
            Some(json!({
                "docIds": [doc],
                "memberId": "never-existed-user-id",
                "accessLevel": "view",
            })),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_share_happy_path_adds_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let owner = app.create_user_token("bulk-share-owner2@test.com").await;
    let recipient = app.create_user_token("bulk-share-recipient@test.com").await;
    let (_, recipient_me) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&recipient), None)
        .await;
    let recipient_id = recipient_me["userId"].as_str().unwrap().to_string();

    let doc_a = app.create_doc(&owner, "share-a", None).await;
    let doc_b = app.create_doc(&owner, "share-b", None).await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/share",
            Some(&owner),
            Some(json!({
                "docIds": [doc_a, doc_b],
                "memberId": recipient_id,
                "accessLevel": "view",
            })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["succeeded"].as_u64().unwrap(), 2);

    app.cleanup().await;
}
