// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E7 item 9 — integration tests for the trash-cleanup
//! worker's `sweep` path. The hourly tick + UTC-hour gate is unit-
//! tested in `crates/api/src/trash_cleanup.rs`; this file drives
//! the actual purge against real DynamoDB to confirm the end-to-end
//! contract:
//!
//!   - eligible-for-purge: a soft-deleted doc with `deleted_at <
//!     cutoff` is found via the GSI7 query, hard-deleted, and an
//!     audit row lands.
//!   - dry-run: the worker logs which docs WOULD be purged but the
//!     destructive ops are skipped.
//!   - ineligible: a soft-deleted doc with `deleted_at >= cutoff`
//!     survives the sweep.
//!   - non-deleted: docs that aren't soft-deleted are never returned
//!     by the GSI query (sparse-index semantics).

mod common;

use hyper::Method;
use ogrenotes_storage::models::security_audit::SecurityAuditAction;

/// Soft-delete a doc via the API, then poll `list_eligible_for_purge`
/// until the GSI catches up. DynamoDB GSI writes are eventually
/// consistent; in practice they land within ~100ms but tests
/// observed values up to 300ms.
async fn wait_for_gsi(app: &common::TestApp, doc_id: &str, cutoff_usec: i64) {
    for _ in 0..30 {
        let rows = app
            .state
            .doc_repo
            .list_eligible_for_purge(cutoff_usec, 50)
            .await
            .unwrap();
        if rows.iter().any(|m| m.doc_id == doc_id) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("doc {doc_id} did not appear on GSI7 within 1.5s");
}

#[tokio::test]
async fn sweep_hard_deletes_eligible_doc_and_writes_audit() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("trash-sweep@test.com").await;
    let doc_id = app.create_doc(&token, "Soon-to-be-purged", None).await;

    // Soft-delete (DELETE /documents/:id moves to trash).
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // The soft-delete writes deleted_at = now_usec(). A cutoff
    // anywhere in the future puts the doc inside the eligible set.
    let cutoff = ogrenotes_common::time::now_usec() + 60_000_000; // +60s
    wait_for_gsi(&app, &doc_id, cutoff).await;

    // Run one sweep tick directly (no scheduler, no atomics).
    ogrenotes_api::trash_cleanup::sweep(&app.state, cutoff)
        .await
        .unwrap();

    // Doc row is gone — hard_delete sweeps PK=DOC#<id>.
    let still_there = app.state.doc_repo.get(&doc_id).await.unwrap();
    assert!(
        still_there.is_none(),
        "doc must be hard-deleted after sweep, got: {still_there:?}"
    );

    // Audit row keyed on the affected user's PK with hard=true.
    let mut found = None;
    for _ in 0..10 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(&user_id, 20)
            .await
            .unwrap();
        if let Some(row) = rows.into_iter().find(|r| {
            matches!(
                &r.action,
                SecurityAuditAction::DocDeleted { doc_id: d, hard: true } if d == &doc_id
            )
        }) {
            found = Some(row);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let row = found.expect("expected DocDeleted hard:true audit row within 200ms");
    assert_eq!(row.user_id, user_id, "subject = the doc's owner");
    assert_eq!(
        row.actor_id, "trash_cleanup_worker",
        "actor identifies the scheduled job"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn sweep_dry_run_skips_destructive_ops() {
    common::require_infra!();
    let mut app = common::TestApp::new().await;
    // Flip the dry-run knob on this specific test's state. The
    // config lives behind Arc, so we rebuild the AppState's config
    // before driving sweep.
    let mut config = (*app.state.config).clone();
    config.trash_cleanup_dry_run = true;
    app.state.config = std::sync::Arc::new(config);

    let (user_id, token) = app.create_user("trash-dryrun@test.com").await;
    let doc_id = app.create_doc(&token, "Doom-spared by dry-run", None).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let cutoff = ogrenotes_common::time::now_usec() + 60_000_000;
    wait_for_gsi(&app, &doc_id, cutoff).await;

    ogrenotes_api::trash_cleanup::sweep(&app.state, cutoff)
        .await
        .unwrap();

    // Doc row must still exist (soft-deleted but not purged).
    let meta = app
        .state
        .doc_repo
        .get(&doc_id)
        .await
        .unwrap()
        .expect("dry-run must NOT hard-delete");
    assert!(meta.is_deleted, "doc must still be soft-deleted");

    // No hard:true audit row should land.
    let rows = app
        .state
        .security_audit_repo
        .list_for_user(&user_id, 20)
        .await
        .unwrap();
    let has_hard = rows.iter().any(|r| {
        matches!(
            &r.action,
            SecurityAuditAction::DocDeleted { hard: true, .. }
        )
    });
    assert!(!has_hard, "dry-run must not emit a hard-delete audit row");

    app.cleanup().await;
}

#[tokio::test]
async fn sweep_leaves_ineligible_recent_deletes_alone() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("trash-recent@test.com").await;
    let doc_id = app.create_doc(&token, "Recently trashed", None).await;
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Cutoff in the past — the doc's deleted_at (now-ish) is
    // newer than the cutoff, so it's ineligible.
    let cutoff = ogrenotes_common::time::now_usec() - 60_000_000; // -60s
    ogrenotes_api::trash_cleanup::sweep(&app.state, cutoff)
        .await
        .unwrap();

    let meta = app
        .state
        .doc_repo
        .get(&doc_id)
        .await
        .unwrap()
        .expect("ineligible doc must survive sweep");
    assert!(meta.is_deleted, "doc must still be soft-deleted");

    app.cleanup().await;
}

#[tokio::test]
async fn sweep_ignores_active_non_deleted_docs() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("trash-active@test.com").await;
    // Create a doc but DO NOT soft-delete it.
    let doc_id = app.create_doc(&token, "Active doc", None).await;

    // GSI query with any cutoff must NOT return this doc — the
    // sparse-index semantics say only rows with `is_deleted_gsi`
    // populated are in the index, and a never-deleted doc never
    // had that attribute written.
    let cutoff = ogrenotes_common::time::now_usec() + 60_000_000;
    let eligible = app
        .state
        .doc_repo
        .list_eligible_for_purge(cutoff, 50)
        .await
        .unwrap();
    assert!(
        eligible.iter().all(|m| m.doc_id != doc_id),
        "active doc must not appear in GSI7 query, got: {eligible:?}"
    );

    // For the same reason a sweep is a no-op on the active doc.
    ogrenotes_api::trash_cleanup::sweep(&app.state, cutoff)
        .await
        .unwrap();
    let meta = app.state.doc_repo.get(&doc_id).await.unwrap();
    assert!(meta.is_some(), "active doc must survive sweep");

    app.cleanup().await;
}
