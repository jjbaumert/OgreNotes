// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for `POST /api/v1/imports/quip/{id}/start` and
//! `GET /api/v1/imports/quip/{id}` (Quip import Phase 1 Task 4).
//!
//! Drives the real router with a real `JobQueue` producer against shared
//! Redis (`TestApp::new()` wires it — see `common/mod.rs`), so these are
//! gated on `require_infra!`. Enqueue success is asserted indirectly via
//! the `202` response: the handler enqueues before returning it and maps
//! enqueue failures to `503`, so a `202` proves the job made it onto the
//! queue.

mod common;

use hyper::Method;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::import::{ImportRecord, ImportStatus};

/// Seed a `Scoping` import record with the given owner + selected roots,
/// returning its id. Mirrors the helper in
/// `test_quip_inventory_worker.rs:101` (not shared across test binaries).
async fn seed_scoping_import(app: &common::TestApp, owner: &str, roots: &[&str]) -> String {
    let import_id = format!("imp-{}", nanoid::nanoid!(8));
    let now = now_usec();
    let record = ImportRecord {
        import_id: import_id.clone(),
        owner_id: owner.to_string(),
        status: ImportStatus::Scoping,
        phase: 0,
        quip_user_id: None,
        target_folder_id: None,
        selected_roots: roots.iter().map(|s| s.to_string()).collect(),
        created_at: now,
        updated_at: now,
    };
    app.state.import_repo.create(&record).await.expect("seed import record");
    import_id
}

#[tokio::test]
async fn start_authorizes_target_folder_persists_scope_and_enqueues() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("owner1@test.com").await;
    let folder = app.create_folder(&token, "Dest", None).await;
    let import_id = seed_scoping_import(&app, &user_id, &[]).await;

    let (status, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/imports/quip/{import_id}/start"),
            Some(&token),
            Some(serde_json::json!({
                "selectedRootFolderIds": ["root"],
                "targetFolderId": folder,
            })),
        )
        .await;
    assert_eq!(status, 202, "start failed: {body}");
    assert_eq!(body["importId"], import_id);
    assert_eq!(body["status"], "running");

    let rec = app
        .state
        .import_repo
        .get(&import_id)
        .await
        .unwrap()
        .expect("import record exists");
    assert_eq!(rec.selected_roots, vec!["root".to_string()]);
    assert_eq!(rec.target_folder_id.as_deref(), Some(folder.as_str()));

    app.cleanup().await;
}

#[tokio::test]
async fn start_rejects_unauthorized_target_folder() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let (_other_id, other_token) = app.create_user("other@test.com").await;
    let their_folder = app.create_folder(&other_token, "Theirs", None).await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    let (status, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/imports/quip/{import_id}/start"),
            Some(&owner_token),
            Some(serde_json::json!({
                "selectedRootFolderIds": ["root"],
                "targetFolderId": their_folder,
            })),
        )
        .await;
    assert_eq!(
        status, 404,
        "check_folder_access hides unauthorized folders as 404: {body}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn get_status_returns_progress_and_is_owner_gated() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let (_other_id, other_token) = app.create_user("other@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    // owner sees it:
    let (status, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/imports/quip/{import_id}"),
            Some(&owner_token),
            None,
        )
        .await;
    assert_eq!(status, 200, "get_status failed: {body}");
    assert_eq!(body["status"], "scoping");
    assert_eq!(body["phase"], 0);
    assert_eq!(body["progress"]["total"], 0);
    assert_eq!(body["progress"]["done"], 0);
    assert_eq!(body["progress"]["stage"], "scoping");

    // a different user gets 404 (no existence disclosure):
    let (status, _body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/imports/quip/{import_id}"),
            Some(&other_token),
            None,
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}
