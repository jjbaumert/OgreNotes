// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #166: the orphaned-object sweep deletes only what provably has no
//! owner, and only once it is old enough.
//!
//! `now` is injected so an object staged a moment ago can be made to look
//! a day (or a week) old without waiting.

mod common;

use ogrenotes_api::blob_reconcile::{
    check, Root, Verdict, DOC_MIN_AGE_USEC, STAGING_MIN_AGE_USEC,
};
use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::import::{ImportRecord, ImportStatus};

async fn stage(app: &common::TestApp, key: &str) {
    app.state
        .doc_repo
        .s3()
        .put_object(key, b"orphan-candidate".to_vec())
        .await
        .unwrap_or_else(|e| panic!("stage {key}: {e}"));
}

async fn present(app: &common::TestApp, key: &str) -> bool {
    app.state.doc_repo.s3().object_exists(key).await.expect("head object")
}

/// A day and an hour from now: everything staged in this test is past the
/// document age gate.
fn a_day_later() -> i64 {
    now_usec() + DOC_MIN_AGE_USEC + 60 * 60 * 1_000_000
}

fn a_week_later() -> i64 {
    now_usec() + STAGING_MIN_AGE_USEC + 60 * 60 * 1_000_000
}

#[tokio::test]
async fn blobs_of_a_document_that_never_existed_are_deleted_once_old() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ghost = new_id();
    let key = format!("blobs/{ghost}/b1/photo.png");
    stage(&app, &key).await;

    assert_eq!(
        check(&app.state, Root::Blobs, &ghost, now_usec(), false).await,
        Verdict::TooRecent,
        "a fresh prefix may belong to a copy still being created",
    );
    assert!(present(&app, &key).await);

    assert_eq!(
        check(&app.state, Root::Blobs, &ghost, a_day_later(), false).await,
        Verdict::Orphan { deleted: true },
    );
    assert!(!present(&app, &key).await, "the orphan must be gone");

    app.cleanup().await;
}

#[tokio::test]
async fn dry_run_reports_an_orphan_but_deletes_nothing() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ghost = new_id();
    let key = format!("docs/{ghost}/snapshots/1.bin");
    stage(&app, &key).await;

    assert_eq!(
        check(&app.state, Root::Docs, &ghost, a_day_later(), true).await,
        Verdict::Orphan { deleted: false },
    );
    assert!(present(&app, &key).await, "dry-run must not delete");

    app.state.doc_repo.s3().delete_prefix(&format!("docs/{ghost}/")).await.unwrap();
    app.cleanup().await;
}

#[tokio::test]
async fn a_live_document_is_never_an_orphan_however_old() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("reconcile-live@test.com").await;
    let doc_id = app.create_doc(&token, "Keep me", None).await;
    let key = format!("blobs/{doc_id}/b1/photo.png");
    stage(&app, &key).await;

    for root in [Root::Blobs, Root::Docs] {
        assert_eq!(
            check(&app.state, root, &doc_id, a_week_later(), false).await,
            Verdict::Owned,
            "{root:?}",
        );
    }
    assert!(present(&app, &key).await);

    app.cleanup().await;
}

/// A soft-deleted document still has its row; its content belongs to
/// `trash_cleanup`, which honours the trash retention window.
#[tokio::test]
async fn a_trashed_document_is_left_to_trash_cleanup() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("reconcile-trash@test.com").await;
    let doc_id = app.create_doc(&token, "Trashed", None).await;
    app.state.doc_repo.soft_delete(&doc_id, now_usec()).await.unwrap();
    let key = format!("blobs/{doc_id}/b1/photo.png");
    stage(&app, &key).await;

    assert_eq!(
        check(&app.state, Root::Blobs, &doc_id, a_week_later(), false).await,
        Verdict::Owned,
    );
    assert!(present(&app, &key).await);

    app.cleanup().await;
}

fn quip_import(status: ImportStatus) -> ImportRecord {
    let now = now_usec();
    ImportRecord {
        import_id: new_id(),
        owner_id: new_id(),
        status,
        phase: 0,
        quip_user_id: None,
        target_folder_id: None,
        import_folder_id: None,
        selected_roots: vec![],
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn quip_staging_waits_for_its_import_to_finish() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let record = quip_import(ImportStatus::Running);
    app.state.import_repo.create(&record).await.unwrap();
    let key = format!("imports/{}/threads/t1.html", record.import_id);
    stage(&app, &key).await;

    assert_eq!(
        check(&app.state, Root::Imports, &record.import_id, a_week_later(), false).await,
        Verdict::Owned,
        "a running import's staging is its in-flight material",
    );
    assert!(present(&app, &key).await);

    app.state
        .import_repo
        .set_status(&record.import_id, ImportStatus::Failed)
        .await
        .unwrap();
    assert_eq!(
        check(&app.state, Root::Imports, &record.import_id, now_usec(), false).await,
        Verdict::TooRecent,
    );
    assert_eq!(
        check(&app.state, Root::Imports, &record.import_id, a_week_later(), false).await,
        Verdict::Orphan { deleted: true },
    );
    assert!(!present(&app, &key).await);

    app.cleanup().await;
}

/// DOCX/PDF staging is keyed by user id and has no import record. A day is
/// not enough; a week is.
#[tokio::test]
async fn upload_staging_without_an_import_record_is_deleted_after_a_week() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let user = new_id();
    let key = format!("imports/{user}/{}.docx", new_id());
    stage(&app, &key).await;

    assert_eq!(
        check(&app.state, Root::Imports, &user, a_day_later(), false).await,
        Verdict::TooRecent,
    );
    assert_eq!(
        check(&app.state, Root::Imports, &user, a_week_later(), false).await,
        Verdict::Orphan { deleted: true },
    );
    assert!(!present(&app, &key).await);

    app.cleanup().await;
}

#[tokio::test]
async fn a_degenerate_name_is_refused() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    for name in ["", "a/b"] {
        assert!(
            matches!(
                check(&app.state, Root::Blobs, name, a_week_later(), false).await,
                Verdict::Undecided(_)
            ),
            "{name:?}",
        );
    }
    app.cleanup().await;
}

/// The walk underneath the sweep: children come back one page at a time,
/// in order, with no repeats, even when one child's name is a prefix of
/// the next (`a` / `ab`).
#[tokio::test]
async fn child_listing_pages_through_every_child_once() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let parent = format!("reconcile-{}/", new_id());
    for key in ["a/1", "a/2", "ab/1", "b/1"] {
        stage(&app, &format!("{parent}{key}")).await;
    }

    let s3 = app.state.doc_repo.s3();
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let (names, more) = s3.list_child_names(&parent, after.as_deref(), 1).await.unwrap();
        seen.extend(names.iter().cloned());
        after = names.last().cloned().or(after);
        if !more {
            break;
        }
        assert!(seen.len() <= 3, "the walk must terminate: {seen:?}");
    }
    assert_eq!(seen, vec!["a", "ab", "b"]);

    s3.delete_prefix(&parent).await.unwrap();
    app.cleanup().await;
}

/// A whole dry-run pass over the bucket terminates and deletes nothing,
/// even with every object made to look a week old.
#[tokio::test]
async fn a_dry_run_sweep_deletes_nothing() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ghost = new_id();
    let key = format!("blobs/{ghost}/b1/photo.png");
    stage(&app, &key).await;

    let cursors = ogrenotes_api::blob_reconcile::Cursors::default();
    let report =
        ogrenotes_api::blob_reconcile::sweep(&app.state, a_week_later(), true, &cursors).await;
    assert_eq!(report.deleted, 0, "{report:?}");
    assert!(present(&app, &key).await);

    app.state.doc_repo.s3().delete_prefix(&format!("blobs/{ghost}/")).await.unwrap();
    app.cleanup().await;
}
