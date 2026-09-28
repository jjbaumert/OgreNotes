// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! The search index survives a lost disk: snapshots round-trip through S3,
//! and a rebuild restores what the index lost.

mod common;

use hyper::Method;
use ogrenotes_api::search_backup::{self, SNAPSHOT_KEY};
use ogrenotes_search::{SearchDocument, SearchIndex, SearchQuery};

fn q(text: &str) -> SearchQuery {
    SearchQuery { text: text.into(), doc_type: None, owner_id: None, folder_id: None, limit: 50, offset: 0 }
}

/// Snapshot an on-disk index to the bucket, then restore it into an empty
/// directory the way server startup does.
#[tokio::test]
async fn a_snapshot_restores_into_an_empty_directory() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let s3 = app.state.doc_repo.s3();

    let src = tempfile::tempdir().unwrap();
    let index = SearchIndex::open_or_create(src.path()).unwrap();
    index
        .index_document(&SearchDocument {
            doc_id: "d1".into(),
            title: "Backed up".into(),
            body: "the capybara survives".into(),
            owner_id: "u1".into(),
            doc_type: "document".into(),
            folder_id: None,
            workspace_id: None,
            updated_at: 1,
            created_at: 1,
        })
        .unwrap();
    let snapshot = index.snapshot().unwrap().unwrap();
    s3.put_object(SNAPSHOT_KEY, search_backup::encode(&snapshot, 4242).unwrap())
        .await
        .unwrap();

    let dst = tempfile::tempdir().unwrap();
    let dir = dst.path().join("index");
    assert_eq!(search_backup::restore_if_empty(s3, &dir).await, Some(4242));
    let restored = SearchIndex::open_or_create(&dir).unwrap();
    assert_eq!(restored.search(&q("capybara")).unwrap().len(), 1);

    // An index that already exists is never overwritten.
    assert_eq!(search_backup::restore_if_empty(s3, &dir).await, None);

    s3.delete_object(SNAPSHOT_KEY).await.unwrap();
    let empty = dst.path().join("fresh");
    assert_eq!(
        search_backup::restore_if_empty(s3, &empty).await,
        None,
        "no snapshot: nothing restored (the caller rebuilds)"
    );

    app.cleanup().await;
}

/// A document missing from the index comes back after a full re-index; a
/// trashed one goes away.
#[tokio::test]
async fn a_full_reindex_restores_lost_entries_and_drops_trashed_ones() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("reindex@test.com").await;
    let kept = app.create_doc(&token, "Pangolin notes", None).await;
    let trashed = app.create_doc(&token, "Pangolin trash", None).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Simulate a lost index, and trash one document.
    app.state.search_index.delete_document(&kept).unwrap();
    app.state.search_index.delete_document(&trashed).unwrap();
    app.state.doc_repo.soft_delete(&trashed, ogrenotes_common::time::now_usec()).await.unwrap();
    assert!(app.state.search_index.search(&q("pangolin")).unwrap().is_empty());

    let report = search_backup::reindex(&app.state, None).await.unwrap();
    assert!(report.indexed >= 1 && report.removed >= 1, "{report:?}");
    let hits: Vec<String> = app
        .state
        .search_index
        .search(&q("pangolin"))
        .unwrap()
        .into_iter()
        .map(|h| h.doc_id)
        .collect();
    assert!(hits.contains(&kept), "{hits:?}");
    assert!(!hits.contains(&trashed), "{hits:?}");

    app.cleanup().await;
}

#[tokio::test]
async fn only_admins_can_start_a_reindex() {
    common::require_infra!();
    let app = common::TestApp::new_with_admin_emails(vec!["boss@test.com".into()]).await;
    let user = app.create_user_token("user@test.com").await;
    let (status, _) = app
        .json_request(Method::POST, "/api/v1/admin/search/reindex", Some(&user), None)
        .await;
    assert_eq!(status, 403);

    let admin = app.create_user_token("boss@test.com").await;
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/admin/search/reindex", Some(&admin), None)
        .await;
    assert_eq!(status, 202, "{json}");
    assert_eq!(json["started"], true);

    // Let the background task finish before tearing the tables down.
    for _ in 0..50 {
        if !search_backup::is_reindexing() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    app.cleanup().await;
}
