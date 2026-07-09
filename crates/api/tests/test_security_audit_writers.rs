// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E6 audit-log writer tests.
//!
//! Piece A — SecurityAudit writers:
//!   - LoginSuccess on dev-login.
//!   - LoginFailure { reason: "disabled" } on a disabled user.
//!   - ShareRevoked when a doc member is removed (with actor != subject).
//!   - DocDeleted { hard: false } on soft-delete.
//!
//! Piece B — Activity::Delete + symmetric hard-delete audit:
//!   - Activity::Delete row lands on soft-delete (survives because the
//!     doc row stays under PK=DOC#<id>).
//!   - DocDeleted { hard: true } on user-initiated purge.
//!
//! The session-revoke / refresh-reuse path is covered by the
//! existing `test_auth.rs` reuse scenario — we extend that with an
//! audit assertion rather than duplicating the rotation dance.

mod common;

use hyper::Method;
use ogrenotes_storage::models::activity::ActivityEventType;
use ogrenotes_storage::models::security_audit::{SecurityAudit, SecurityAuditAction};
use ogrenotes_storage::models::LinkSharingMode;

/// Poll the SecurityAudit table for a matching row. The writer
/// fires via tokio::spawn so the response can race the DDB write.
/// 10 × 20ms = 200ms upper bound (same pattern as SCIM piece F).
/// Returns the full row so callers can assert on `actor_id` as well
/// as the action payload.
async fn wait_for_audit_row(
    app: &common::TestApp,
    user_id: &str,
    matcher: impl Fn(&SecurityAuditAction) -> bool,
) -> SecurityAudit {
    for _ in 0..10 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(user_id, 20)
            .await
            .unwrap();
        if let Some(row) = rows.into_iter().find(|r| matcher(&r.action)) {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("expected SecurityAudit row for user {user_id} within 200ms");
}

#[tokio::test]
async fn dev_login_success_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/dev-login",
            None,
            Some(serde_json::json!({ "email": "audit-login@test.com" })),
        )
        .await;
    assert_eq!(status, 200);
    let user_id = json["userId"].as_str().unwrap().to_string();

    let row = wait_for_audit_row(&app, &user_id, |a| {
        matches!(a, SecurityAuditAction::LoginSuccess)
    })
    .await;
    assert!(matches!(row.action, SecurityAuditAction::LoginSuccess));
    assert_eq!(row.actor_id, user_id, "self-event: actor == subject");

    app.cleanup().await;
}

#[tokio::test]
async fn dev_login_for_disabled_user_writes_failure_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // First login to provision the user, then disable them via the
    // repo (no admin endpoint in scope for this test).
    let (_, json) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/dev-login",
            None,
            Some(serde_json::json!({ "email": "audit-disabled@test.com" })),
        )
        .await;
    let user_id = json["userId"].as_str().unwrap().to_string();
    app.state
        .user_repo
        .set_disabled(&user_id, true)
        .await
        .unwrap();

    // Second login attempt — should 403 and audit LoginFailure.
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/dev-login",
            None,
            Some(serde_json::json!({ "email": "audit-disabled@test.com" })),
        )
        .await;
    assert_eq!(status, 403);

    let row = wait_for_audit_row(&app, &user_id, |a| {
        matches!(a, SecurityAuditAction::LoginFailure { reason } if reason == "disabled")
    })
    .await;
    match row.action {
        SecurityAuditAction::LoginFailure { reason } => {
            assert_eq!(reason, "disabled");
        }
        other => panic!("unexpected action: {other:?}"),
    }
    assert_eq!(row.actor_id, user_id, "self-event: actor == subject");

    app.cleanup().await;
}

#[tokio::test]
async fn share_revoke_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Owner creates a doc, adds Bob as a member, then revokes.
    let (alice_id, alice_token) = app.create_user("audit-share-alice@test.com").await;
    let (bob_id, _) = app.create_user("audit-share-bob@test.com").await;

    let (_, doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&alice_token),
            Some(serde_json::json!({ "title": "Shared", "docType": "document" })),
        )
        .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" })),
        )
        .await;
    assert!(status == 200 || status == 204 || status == 201, "add returned {status}");

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/members/{bob_id}"),
            Some(&alice_token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Audit row is keyed on the TARGET (Bob) per the writer
    // contract — the subject of "what happened to your access" —
    // but the actor must be Alice (the revoker), not Bob himself.
    let row = wait_for_audit_row(&app, &bob_id, |a| {
        matches!(a, SecurityAuditAction::ShareRevoked { doc_id: d, target: t } if d == &doc_id && t == &bob_id)
    })
    .await;
    assert!(matches!(row.action, SecurityAuditAction::ShareRevoked { .. }));
    assert_eq!(row.user_id, bob_id, "subject is the removed member");
    assert_eq!(row.actor_id, alice_id, "actor is the revoker");

    app.cleanup().await;
}

#[tokio::test]
async fn soft_delete_writes_doc_deleted_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("audit-delete@test.com").await;

    let (_, doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&token),
            Some(serde_json::json!({ "title": "Doomed", "docType": "document" })),
        )
        .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &user_id, |a| {
        matches!(
            a,
            SecurityAuditAction::DocDeleted { doc_id: d, hard: false } if d == &doc_id
        )
    })
    .await;
    match row.action {
        SecurityAuditAction::DocDeleted { hard, .. } => assert!(!hard, "soft-delete must NOT be hard"),
        other => panic!("unexpected action: {other:?}"),
    }
    assert_eq!(row.actor_id, user_id, "self-event: actor == subject");

    app.cleanup().await;
}

/// Piece B — soft-delete writes an Activity::Delete row keyed
/// PK=DOC#<id>. The row survives because soft_delete only flips
/// is_deleted; the underlying DDB row stays. (hard_delete would
/// sweep it.)
#[tokio::test]
async fn soft_delete_writes_activity_delete_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("activity-delete@test.com").await;

    let (_, doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&token),
            Some(serde_json::json!({ "title": "Doomed", "docType": "document" })),
        )
        .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Poll the activity feed for the Delete row.
    let mut found = None;
    for _ in 0..10 {
        let rows = app
            .state
            .activity_repo
            .list(&doc_id, 20)
            .await
            .unwrap();
        if let Some(row) = rows.into_iter().find(|r| r.event_type == ActivityEventType::Delete) {
            found = Some(row);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let row = found.expect("expected Activity::Delete row within 200ms");
    assert_eq!(row.doc_id, doc_id);
    assert_eq!(row.actor_id, user_id);

    app.cleanup().await;
}

/// Piece B — user-initiated purge (DELETE /documents/:id/purge on a
/// trashed doc) writes SecurityAudit::DocDeleted { hard: true }. The
/// doc row itself is gone after hard_delete, so this audit row is
/// the only forensic trail of the destructive action.
#[tokio::test]
async fn purge_writes_hard_doc_deleted_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("audit-purge@test.com").await;

    let (_, doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&token),
            Some(serde_json::json!({ "title": "Doomed", "docType": "document" })),
        )
        .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();

    // Soft-delete first (purge requires the doc to be in trash).
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Now purge.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/purge"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &user_id, |a| {
        matches!(
            a,
            SecurityAuditAction::DocDeleted { doc_id: d, hard: true } if d == &doc_id
        )
    })
    .await;
    match row.action {
        SecurityAuditAction::DocDeleted { hard, .. } => assert!(hard, "purge must be hard"),
        other => panic!("unexpected action: {other:?}"),
    }
    assert_eq!(row.actor_id, user_id, "self-event: actor == subject");

    app.cleanup().await;
}

/// Phase 0 (link sharing) — changing a document's link-sharing settings
/// via `PATCH .../link-settings` writes SecurityAudit::LinkSharingChanged.
/// Regression guard: this owner action previously emitted no audit row
/// at all, despite every other share mutation auditing — the gap the
/// link-sharing design's §5.6 calls out. Self-event: the owner is both
/// subject and actor; `mode` carries the new enabling level.
#[tokio::test]
async fn link_settings_change_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("audit-linkshare@test.com").await;

    let (_, doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&token),
            Some(serde_json::json!({ "title": "Linked", "docType": "document" })),
        )
        .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();

    // Turn link sharing on at View — the owner action that previously
    // emitted no audit row.
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token),
            Some(serde_json::json!({ "linkSharingMode": "view" })),
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &user_id, |a| {
        matches!(
            a,
            SecurityAuditAction::LinkSharingChanged { doc_id: d, mode: Some(m), .. }
                if d == &doc_id && *m == LinkSharingMode::View
        )
    })
    .await;
    match row.action {
        SecurityAuditAction::LinkSharingChanged { mode, .. } => {
            assert_eq!(mode, Some(LinkSharingMode::View), "audit records the new mode");
        }
        other => panic!("unexpected action: {other:?}"),
    }
    assert_eq!(row.user_id, user_id, "subject is the doc owner");
    assert_eq!(row.actor_id, user_id, "self-event: actor == subject");

    app.cleanup().await;
}

/// Phase 0 (link sharing) — *disabling* link sharing (mode `none`) also
/// writes an audit row, with `mode: None`. Complements
/// `link_settings_change_writes_audit_row` (enable path): the handler's
/// mode-mapping collapses both `LinkSharingMode::None` and an absent
/// mode to a null audit `mode`, and the design's §5.6 lists disable as a
/// distinct audited event. Exercises the third branch of the handler
/// match end-to-end, not just the model roundtrip.
#[tokio::test]
async fn link_settings_disable_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("audit-linkshare-off@test.com").await;

    let (_, doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&token),
            Some(serde_json::json!({ "title": "Linked", "docType": "document" })),
        )
        .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();

    // Enable at view, then disable — the disable is the event under test.
    for mode in ["view", "none"] {
        let (status, _) = app
            .json_request(
                Method::PATCH,
                &format!("/api/v1/documents/{doc_id}/link-settings"),
                Some(&token),
                Some(serde_json::json!({ "linkSharingMode": mode })),
            )
            .await;
        assert_eq!(status, 204, "PATCH link-settings mode={mode}");
    }

    // The disable row carries `mode: None` (null in the detail blob).
    let row = wait_for_audit_row(&app, &user_id, |a| {
        matches!(
            a,
            SecurityAuditAction::LinkSharingChanged { doc_id: d, mode: None, .. } if d == &doc_id
        )
    })
    .await;
    match row.action {
        SecurityAuditAction::LinkSharingChanged { mode, .. } => {
            assert_eq!(mode, None, "disable records a null mode");
        }
        other => panic!("unexpected action: {other:?}"),
    }
    assert_eq!(row.actor_id, user_id, "self-event: actor == subject");

    app.cleanup().await;
}

/// gap-004: a *sub-option-only* PATCH (no mode change) is also audited —
/// the resulting view-options are recorded, so enabling allow_comments on
/// a live link leaves a forensic trail.
#[tokio::test]
async fn link_settings_view_option_change_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("audit-linkshare-opt@test.com").await;

    let (_, doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&token),
            Some(serde_json::json!({ "title": "Linked", "docType": "document" })),
        )
        .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();

    // Enable a view link, then flip a sub-option (a viewOptions-only PATCH).
    for body in [
        serde_json::json!({ "linkSharingMode": "view" }),
        serde_json::json!({ "viewOptions": { "allowComments": true } }),
    ] {
        let (status, _) = app
            .json_request(
                Method::PATCH,
                &format!("/api/v1/documents/{doc_id}/link-settings"),
                Some(&token),
                Some(body),
            )
            .await;
        assert_eq!(status, 204);
    }

    // The sub-option toggle produced an audit row with the resulting
    // view-options (allow_comments = true) and the still-current mode.
    let row = wait_for_audit_row(&app, &user_id, |a| {
        matches!(
            a,
            SecurityAuditAction::LinkSharingChanged { doc_id: d, view_options, .. }
                if d == &doc_id && view_options.allow_comments
        )
    })
    .await;
    match row.action {
        SecurityAuditAction::LinkSharingChanged { mode, view_options, .. } => {
            assert!(view_options.allow_comments, "sub-option change is recorded");
            assert_eq!(mode, Some(LinkSharingMode::View), "resulting mode is the live view link");
        }
        other => panic!("unexpected action: {other:?}"),
    }
    assert_eq!(row.actor_id, user_id, "self-event: actor == subject");

    app.cleanup().await;
}

/// Granting a member access to a document writes
/// `SecurityAudit::ShareGranted`. Mirror of `share_revoke_writes_audit_row`
/// for the grant side — the row is keyed on the *target* (Bob, the subject
/// of "you were granted access") while `actor_id` is the granter (Alice),
/// and `level` records the wire-shape access label. The grant audit had no
/// coverage despite the revoke side being tested.
#[tokio::test]
async fn doc_share_grant_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, alice_token) = app.create_user("audit-grant-alice@test.com").await;
    let (bob_id, _) = app.create_user("audit-grant-bob@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Shared", None).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" })),
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &bob_id, |a| {
        matches!(
            a,
            SecurityAuditAction::ShareGranted { doc_id: d, target: t, level }
                if d == &doc_id && t == &bob_id && level == "EDIT"
        )
    })
    .await;
    match row.action {
        SecurityAuditAction::ShareGranted { level, .. } => {
            assert_eq!(level, "EDIT", "audit records the granted level");
        }
        other => panic!("unexpected action: {other:?}"),
    }
    assert_eq!(row.user_id, bob_id, "subject is the granted member");
    assert_eq!(row.actor_id, alice_id, "actor is the granter");

    app.cleanup().await;
}

/// Changing a document member's access level (PATCH on the members
/// endpoint) writes `SecurityAudit::ShareUpdated` with the *new* level.
/// Share at VIEW, then raise to EDIT — the row records EDIT.
#[tokio::test]
async fn doc_share_update_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, alice_token) = app.create_user("audit-update-alice@test.com").await;
    let (bob_id, _) = app.create_user("audit-update-bob@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Shared", None).await;

    // Share at VIEW first.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" })),
        )
        .await;
    assert_eq!(status, 204);

    // Raise to EDIT — the event under test.
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/members/{bob_id}"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" })),
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &bob_id, |a| {
        matches!(
            a,
            SecurityAuditAction::ShareUpdated { doc_id: d, target: t, level }
                if d == &doc_id && t == &bob_id && level == "EDIT"
        )
    })
    .await;
    match row.action {
        SecurityAuditAction::ShareUpdated { level, .. } => {
            assert_eq!(level, "EDIT", "audit records the new level");
        }
        other => panic!("unexpected action: {other:?}"),
    }
    assert_eq!(row.user_id, bob_id, "subject is the member whose level changed");
    assert_eq!(row.actor_id, alice_id, "actor is the doc owner");

    app.cleanup().await;
}

/// Granting a member access to a *folder* writes `SecurityAudit::ShareGranted`
/// with `doc_id` carrying the folder id (the audit schema doesn't distinguish
/// docs from folders). Complements the doc-side grant test — the folder
/// writer is a separate call site.
#[tokio::test]
async fn folder_share_grant_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, alice_token) = app.create_user("audit-fgrant-alice@test.com").await;
    let (bob_id, _) = app.create_user("audit-fgrant-bob@test.com").await;
    let folder_id = app.create_folder(&alice_token, "Shared", None).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" })),
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &bob_id, |a| {
        matches!(
            a,
            SecurityAuditAction::ShareGranted { doc_id: d, target: t, level }
                if d == &folder_id && t == &bob_id && level == "VIEW"
        )
    })
    .await;
    assert!(matches!(row.action, SecurityAuditAction::ShareGranted { .. }));
    assert_eq!(row.user_id, bob_id, "subject is the granted member");
    assert_eq!(row.actor_id, alice_id, "actor is the folder owner");

    app.cleanup().await;
}

/// Removing a member from a *folder* writes `SecurityAudit::ShareRevoked`.
/// The doc-side revoke is covered by `share_revoke_writes_audit_row`; the
/// folder `remove_member` handler is a distinct writer that was untested.
#[tokio::test]
async fn folder_share_revoke_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, alice_token) = app.create_user("audit-frevoke-alice@test.com").await;
    let (bob_id, _) = app.create_user("audit-frevoke-bob@test.com").await;
    let folder_id = app.create_folder(&alice_token, "Shared", None).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" })),
        )
        .await;
    assert_eq!(status, 204);

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_id}/members/{bob_id}"),
            Some(&alice_token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &bob_id, |a| {
        matches!(
            a,
            SecurityAuditAction::ShareRevoked { doc_id: d, target: t }
                if d == &folder_id && t == &bob_id
        )
    })
    .await;
    assert!(matches!(row.action, SecurityAuditAction::ShareRevoked { .. }));
    assert_eq!(row.user_id, bob_id, "subject is the removed member");
    assert_eq!(row.actor_id, alice_id, "actor is the folder owner (the revoker)");

    app.cleanup().await;
}

/// #140 edit-lock — toggling a document's lock via `PUT .../lock` writes
/// `SecurityAudit::DocLockToggled` with the resulting state. A locked doc
/// is a doc-wide write-authority change (read-only for everyone including
/// editors), so both transitions must leave a forensic trail; the no-op
/// path (same state re-asserted) must NOT write a redundant row. This
/// writer had no coverage — the functional lock tests in
/// `test_documents.rs` never look at the audit table.
#[tokio::test]
async fn doc_lock_toggle_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("audit-lock@test.com").await;
    let doc_id = app.create_doc(&token, "Lockable", None).await;

    // Lock — the event under test.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/lock"),
            Some(&token),
            Some(serde_json::json!({ "locked": true })),
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &user_id, |a| {
        matches!(
            a,
            SecurityAuditAction::DocLockToggled { doc_id: d, locked: true } if d == &doc_id
        )
    })
    .await;
    assert_eq!(row.user_id, user_id, "subject is the doc owner");
    assert_eq!(row.actor_id, user_id, "self-event: owner-only toggle");

    // Unlock — the reverse transition is audited too (restoring write
    // authority is as forensically interesting as removing it).
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/lock"),
            Some(&token),
            Some(serde_json::json!({ "locked": false })),
        )
        .await;
    assert_eq!(status, 204);

    wait_for_audit_row(&app, &user_id, |a| {
        matches!(
            a,
            SecurityAuditAction::DocLockToggled { doc_id: d, locked: false } if d == &doc_id
        )
    })
    .await;

    // No-op toggle (already unlocked): the handler returns 204 without
    // writing or auditing. Both real transitions have already been
    // polled to completion above, so a fixed grace period then an exact
    // row count is race-free — there is no third writer in flight.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/lock"),
            Some(&token),
            Some(serde_json::json!({ "locked": false })),
        )
        .await;
    assert_eq!(status, 204, "no-op toggle still succeeds");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let rows = app
        .state
        .security_audit_repo
        .list_for_user(&user_id, 20)
        .await
        .unwrap();
    let lock_rows = rows
        .iter()
        .filter(|r| matches!(&r.action, SecurityAuditAction::DocLockToggled { .. }))
        .count();
    assert_eq!(lock_rows, 2, "no-op toggle must not write a redundant audit row");

    app.cleanup().await;
}
