// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Losing the connection to storage fails clearly and loses nothing.
//!
//! The probe itself can't be made to fail against the local stack, so
//! these drive `StorageHealth` directly — the same state the probe writes.

mod common;

use hyper::Method;

#[tokio::test]
async fn while_storage_is_unreachable_the_api_answers_503_at_once() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("status@test.com").await;

    let (status, json) = app.json_request(Method::GET, "/api/v1/status", None, None).await;
    assert_eq!(status, 200);
    assert_eq!(json["storage"], "ok");

    app.state.storage_health.record(false);

    let (status, json) = app
        .json_request(Method::GET, "/api/v1/folders", Some(&token), None)
        .await;
    assert_eq!(status, 503, "{json}");
    assert_eq!(json["error"], "storage_unavailable", "the code the frontend matches on");

    // How clients learn the state stays reachable.
    let (status, json) = app.json_request(Method::GET, "/api/v1/status", None, None).await;
    assert_eq!(status, 200);
    assert_eq!(json["storage"], "unreachable");
    assert!(json["since"].as_i64().unwrap() > 0);
    let (status, _) = app
        .raw_request(
            hyper::Request::builder()
                .uri("/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, 200);

    app.state.storage_health.record(true);
    let (status, _) = app
        .json_request(Method::GET, "/api/v1/folders", Some(&token), None)
        .await;
    assert_ne!(status, 503, "requests flow again once storage is back");

    app.cleanup().await;
}

/// An edit whose save failed lives only in the room's memory. When
/// storage returns, the recovery pass snapshots it and clears the mark.
#[tokio::test]
async fn edits_that_failed_to_save_are_written_out_on_recovery() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("recover@test.com").await;
    let doc_id = app.create_doc(&token, "Recover", None).await;
    let before = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().snapshot_version;

    let room = app
        .state
        .room_registry
        .get_or_insert(&doc_id, ogrenotes_collab::document::OgreDoc::new());
    room.mark_unsaved();
    assert!(
        !app.state.room_registry.remove_if_empty(&doc_id).await,
        "an unsaved room must outlive its last client"
    );

    ogrenotes_api::storage_health::persist_unsaved_rooms(&app.state).await;

    let after = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().snapshot_version;
    assert_eq!(after, before + 1, "the room was snapshotted");
    assert!(!room.is_unsaved(), "and is no longer marked");
    assert!(app.state.room_registry.unsaved_rooms().is_empty());

    app.cleanup().await;
}
