// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration test for the SecurityAudit retention worker's `sweep` path.
//!
//! The cutoff arithmetic + hour/enabled gates are unit-tested inline in
//! `crates/api/src/audit_retention.rs`; this drives the actual DESTRUCTIVE
//! delete against real DynamoDB to pin the end-to-end contract — rows older
//! than the cutoff are removed, rows at-or-after the cutoff survive (the
//! delete bound is a strict `<`). Mirrors `test_trash_cleanup.rs`.

mod common;

use ogrenotes_storage::models::security_audit::{SecurityAudit, SecurityAuditAction};

fn audit_row(user_id: &str, audit_id: &str, created_at: i64) -> SecurityAudit {
    SecurityAudit {
        audit_id: audit_id.to_string(),
        user_id: user_id.to_string(),
        actor_id: user_id.to_string(),
        action: SecurityAuditAction::LoginSuccess,
        created_at,
    }
}

#[tokio::test]
async fn sweep_deletes_rows_older_than_cutoff_and_keeps_the_rest() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, _) = app.create_user("audit-retention@test.com").await;

    let cutoff: i64 = 1_700_000_000_000_000;
    // Two rows before the cutoff (must be deleted), one exactly at the cutoff
    // and one after it (must survive — delete is a strict `< cutoff`).
    let rows = [
        audit_row(&user_id, "old-1", cutoff - 2_000_000),
        audit_row(&user_id, "old-2", cutoff - 1),
        audit_row(&user_id, "at-cutoff", cutoff),
        audit_row(&user_id, "after", cutoff + 1_000_000),
    ];
    for r in &rows {
        app.state.security_audit_repo.create(r).await.unwrap();
    }

    // Sanity: the four seeded rows are present before the sweep. (The
    // dev-login that provisioned the user also wrote its own LoginSuccess
    // row, dated 2026 — well after the cutoff — so we assert on our seeded
    // ids rather than a total count.)
    let before = app.state.security_audit_repo.list_for_user(&user_id, 50).await.unwrap();
    let before_ids: std::collections::HashSet<&str> =
        before.iter().map(|r| r.audit_id.as_str()).collect();
    for id in ["old-1", "old-2", "at-cutoff", "after"] {
        assert!(before_ids.contains(id), "{id} should exist pre-sweep");
    }

    // Run one retention pass directly (no scheduler, no atomics).
    ogrenotes_api::audit_retention::sweep(&app.state, cutoff)
        .await
        .unwrap();

    let after = app.state.security_audit_repo.list_for_user(&user_id, 50).await.unwrap();
    let surviving: std::collections::HashSet<&str> =
        after.iter().map(|r| r.audit_id.as_str()).collect();

    assert!(!surviving.contains("old-1"), "row before cutoff must be deleted");
    assert!(!surviving.contains("old-2"), "row before cutoff must be deleted");
    assert!(
        surviving.contains("at-cutoff"),
        "row exactly at the cutoff must survive (delete bound is strict `<`)"
    );
    assert!(surviving.contains("after"), "row after the cutoff must survive");

    app.cleanup().await;
}
