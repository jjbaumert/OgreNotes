// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use aws_sdk_dynamodb::types::AttributeValue;
use hyper::Method;

#[tokio::test]
async fn test_admin_list_requires_admin() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("regular@test.com").await;

    let (status, _) = app
        .json_request(Method::GET, "/api/v1/admin/users", Some(&token), None)
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_admin_list_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(Method::GET, "/api/v1/admin/users", None, None)
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_admin_disable_requires_admin() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("regular@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{user_id}/disable"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_admin_promote_requires_admin() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("regular@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{user_id}/promote"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_admin_cannot_demote_self() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Create a user and manually promote them to admin via the repo
    let (admin_id, _) = app.create_user("admin@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;

    // Re-login to get a token with is_admin=true
    let (_, admin_token) = app.create_user("admin@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{admin_id}/demote"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_admin_cannot_disable_self() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("admin@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{admin_id}/disable"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

// ─── Admin audit log (issue #32) ───────────────────────────────────
//
// Each privileged admin mutation must emit a row to the AdminAudit
// table keyed by the *target* user, with the actor's id captured in
// `actor_id` and the action recorded as the typed enum variant. The
// audit write is fire-and-forget (tokio::spawn), so the integration
// tests poll for the row with a short retry budget rather than
// requiring deterministic ordering.

use ogrenotes_storage::models::admin_audit::AdminAuditAction;

async fn wait_for_audit_row(
    app: &common::TestApp,
    target_user_id: &str,
    expected_action: AdminAuditAction,
    expected_actor_id: &str,
) -> ogrenotes_storage::models::admin_audit::AdminAudit {
    // The handler returns 204 before the spawned audit write has reached
    // DDB, so poll for up to ~2s. In practice the row lands well under
    // 100 ms; the budget is generous to avoid CI flakes.
    for _ in 0..40 {
        let rows = app
            .state
            .admin_audit_repo
            .list_for_user(target_user_id, 10)
            .await
            .expect("list_for_user");
        if let Some(found) = rows
            .into_iter()
            .find(|r| r.action == expected_action && r.actor_id == expected_actor_id)
        {
            return found;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "no AdminAudit row appeared for target={target_user_id} action={:?} actor={expected_actor_id} within 2s",
        expected_action
    );
}

#[tokio::test]
async fn admin_disable_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Promote one user to admin, the other is the target.
    let (admin_id, _) = app.create_user("admin-aud-disable@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-aud-disable@test.com").await;
    let (target_id, _) = app.create_user("victim-disable@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{target_id}/disable"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(
        &app,
        &target_id,
        AdminAuditAction::Disable,
        &admin_id,
    )
    .await;
    assert_eq!(row.target_user_id, target_id);
    assert_eq!(row.actor_id, admin_id);
    assert_eq!(row.action, AdminAuditAction::Disable);
    // detail is `{}` for Disable; SetAskPolicy is the only action that
    // carries non-empty detail.
    assert_eq!(row.detail, "{}");

    app.cleanup().await;
}

#[tokio::test]
async fn admin_promote_and_demote_each_write_their_own_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("admin-aud-promote@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-aud-promote@test.com").await;
    let (target_id, _) = app.create_user("victim-promote@test.com").await;

    // Promote
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{target_id}/promote"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 204);
    let promote_row = wait_for_audit_row(
        &app, &target_id, AdminAuditAction::Promote, &admin_id,
    )
    .await;

    // Demote
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{target_id}/demote"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 204);
    let demote_row = wait_for_audit_row(
        &app, &target_id, AdminAuditAction::Demote, &admin_id,
    )
    .await;

    // Two distinct rows, chronologically ordered.
    assert_ne!(promote_row.audit_id, demote_row.audit_id);
    assert!(demote_row.created_at >= promote_row.created_at);

    app.cleanup().await;
}

#[tokio::test]
async fn admin_audit_list_by_actor_spans_targets() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // One admin acts on two different targets. The target-keyed PK can
    // only answer "what happened to user X"; #49's GSI8 answers the
    // incident-response question "what did admin Y do, across everyone".
    let (admin_id, _) = app.create_user("admin-aud-actor@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-aud-actor@test.com").await;
    let (target_a, _) = app.create_user("victim-actor-a@test.com").await;
    let (target_b, _) = app.create_user("victim-actor-b@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{target_a}/disable"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 204);
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{target_b}/promote"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Poll the actor index until both fire-and-forget rows have landed.
    let mut rows = Vec::new();
    for _ in 0..40 {
        rows = app
            .state
            .admin_audit_repo
            .list_by_actor(&admin_id, 0, 50)
            .await
            .expect("list_by_actor");
        if rows.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert!(
        rows.len() >= 2,
        "actor index should surface both of the admin's actions; got {}",
        rows.len()
    );
    // Every row is attributed to this actor...
    assert!(rows.iter().all(|r| r.actor_id == admin_id));
    // ...and the two actions span two distinct targets — the thing the
    // target-keyed PK can't do without a scan.
    let targets: std::collections::HashSet<_> =
        rows.iter().map(|r| r.target_user_id.clone()).collect();
    assert!(
        targets.contains(&target_a) && targets.contains(&target_b),
        "actor index must span both targets; got {targets:?}"
    );
    // Newest-first, matching list_for_user's ordering contract.
    assert!(
        rows.windows(2).all(|w| w[0].created_at >= w[1].created_at),
        "list_by_actor must return newest-first"
    );

    // The `since` lower bound excludes everything older than it.
    let future = rows.iter().map(|r| r.created_at).max().unwrap() + 1;
    let none = app
        .state
        .admin_audit_repo
        .list_by_actor(&admin_id, future, 50)
        .await
        .expect("list_by_actor since-filter");
    assert!(none.is_empty(), "since-filter must exclude rows before it");

    app.cleanup().await;
}

#[tokio::test]
async fn admin_set_ask_policy_records_prev_and_new_in_detail() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("admin-aud-ask@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-aud-ask@test.com").await;
    let (target_id, _) = app.create_user("victim-ask@test.com").await;

    // Drive a SystemOrByok → Disabled transition. Dev-login
    // auto-opens the policy to SystemOrByok (see
    // routes/auth.rs::dev_login), so newly-created users in this
    // test harness start there. Toggling to `Disabled` exercises
    // the {from, to} schema with two distinct values; a test on
    // a default re-affirmation would silently pass even if the
    // audit detail was buggy.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/admin/users/{target_id}/ask-policy"),
            Some(&admin_token),
            Some(serde_json::json!({ "policy": "disabled" })),
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(
        &app,
        &target_id,
        AdminAuditAction::SetAskPolicy,
        &admin_id,
    )
    .await;
    let detail: serde_json::Value = serde_json::from_str(&row.detail).unwrap();
    // Audit must record both prior and new state so an investigator
    // reading a single row can tell a real toggle from a re-affirmation.
    assert_eq!(
        detail.get("from").and_then(|v| v.as_str()),
        Some("system_or_byok"),
        "SetAskPolicy audit must record prior policy (dev-login default = SystemOrByok)"
    );
    assert_eq!(
        detail.get("to").and_then(|v| v.as_str()),
        Some("disabled"),
        "SetAskPolicy audit must record new policy"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn admin_audit_does_not_record_failed_authorization() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // A non-admin user tries to disable someone — `require_admin` rejects
    // with 403 before any state mutation. There must be NO audit row.
    let (target_id, _) = app.create_user("victim-noadmin@test.com").await;
    let (regular_id, regular_token) = app.create_user("regular@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{target_id}/disable"),
            Some(&regular_token),
            None,
        )
        .await;
    assert_eq!(status, 403);

    // The current code path exits at `require_admin` BEFORE
    // `record_admin_action` is reached, so no spawn can ever exist for
    // a 403-rejected request — the assertion below is structurally
    // guaranteed, not racing a spawn. The 200ms sleep is defensive
    // padding against a future regression where someone moves the
    // audit emit before the auth check; in that buggy world the
    // spawned write would land within this window and fail the test.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let rows = app
        .state
        .admin_audit_repo
        .list_for_user(&target_id, 10)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "audit row must not be written when require_admin rejects (would log unsuccessful attempts as if they succeeded)"
    );
    let _ = regular_id;

    app.cleanup().await;
}

// ─── Phase 4 M-E2 — email_prefix, audit endpoint, rate limit ──

#[tokio::test]
async fn admin_users_email_prefix_filters_results() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("admin-prefix@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-prefix@test.com").await;

    // Seed two distinct prefixes so the filter actually has to choose.
    app.create_user("alice@test.com").await;
    app.create_user("bob@test.com").await;

    let (status, json) = app
        .json_request(
            hyper::Method::GET,
            "/api/v1/admin/users?emailPrefix=alice",
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().expect("users array");
    assert!(
        users.iter().all(|u| u["email"].as_str().unwrap_or("").starts_with("alice")),
        "every returned user must start with the prefix, got: {users:?}"
    );
    assert!(
        users.iter().any(|u| u["email"] == "alice@test.com"),
        "alice must be in the filtered set"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn admin_audit_endpoint_requires_target() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("admin-audit-req@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-audit-req@test.com").await;

    // No target → 400. Both audit tables are user-keyed; a global scan
    // is the v2 carry-forward, not the M-E2 surface.
    let (status, _) = app
        .json_request(
            hyper::Method::GET,
            "/api/v1/admin/audit",
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn admin_audit_endpoint_merges_admin_and_security_rows() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Admin + a target user. The disable below emits an AdminAudit row
    // keyed on the target; we additionally hand-write a SecurityAudit
    // row for the same target to verify the merge.
    let (admin_id, _) = app.create_user("admin-merge@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-merge@test.com").await;

    let (target_id, _) = app.create_user("merge-target@test.com").await;

    let (status, _) = app
        .json_request(
            hyper::Method::POST,
            &format!("/api/v1/admin/users/{target_id}/disable"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Hand-write a SecurityAudit row — until M-E3 ships the MFA
    // writers, the repo is the only path. Direct write also exercises
    // the AppState wiring.
    let sec_row = ogrenotes_storage::models::security_audit::SecurityAudit {
        audit_id: "test-sec-1".to_string(),
        user_id: target_id.clone(),
        actor_id: target_id.clone(),
        action: ogrenotes_storage::models::security_audit::SecurityAuditAction::LoginFailure {
            reason: "bad_password".to_string(),
        },
        created_at: ogrenotes_common::time::now_usec(),
    };
    app.state.security_audit_repo.create(&sec_row).await.unwrap();

    // Poll: the AdminAudit write is spawned, so allow a beat.
    let mut admin_kinds: Vec<String> = Vec::new();
    let mut security_kinds: Vec<String> = Vec::new();
    for _ in 0..40 {
        let (status, json) = app
            .json_request(
                hyper::Method::GET,
                &format!("/api/v1/admin/audit?target={target_id}"),
                Some(&admin_token),
                None,
            )
            .await;
        assert_eq!(status, 200);
        let entries = json["entries"].as_array().expect("entries array");
        admin_kinds = entries
            .iter()
            .filter(|e| e["source"] == "admin")
            .map(|e| e["kind"].as_str().unwrap_or("").to_string())
            .collect();
        security_kinds = entries
            .iter()
            .filter(|e| e["source"] == "security")
            .map(|e| e["kind"].as_str().unwrap_or("").to_string())
            .collect();
        if admin_kinds.iter().any(|k| k == "disable")
            && security_kinds.iter().any(|k| k == "loginFailure")
        {
            return app.cleanup().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "merged audit endpoint never returned both kinds; admin={admin_kinds:?} security={security_kinds:?}"
    );
}

#[tokio::test]
async fn admin_audit_kind_filter_narrows_results() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("admin-kind-filter@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-kind-filter@test.com").await;
    let (target_id, _) = app.create_user("kind-target@test.com").await;

    // Two distinct security kinds on the same target — the filter
    // must pick one and drop the other.
    use ogrenotes_storage::models::security_audit::{SecurityAudit, SecurityAuditAction};
    for kind in [
        SecurityAuditAction::LoginSuccess,
        SecurityAuditAction::LoginFailure { reason: "bad_password".to_string() },
    ] {
        let row = SecurityAudit {
            audit_id: nanoid::nanoid!(8),
            user_id: target_id.clone(),
            actor_id: target_id.clone(),
            action: kind,
            created_at: ogrenotes_common::time::now_usec(),
        };
        app.state.security_audit_repo.create(&row).await.unwrap();
    }

    let (status, json) = app
        .json_request(
            hyper::Method::GET,
            &format!("/api/v1/admin/audit?target={target_id}&kind=loginFailure"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let entries = json["entries"].as_array().expect("entries");
    assert!(!entries.is_empty(), "filtered set must include the loginFailure row");
    assert!(
        entries.iter().all(|e| e["kind"] == "loginFailure"),
        "every entry must match the kind filter, got: {entries:?}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn admin_audit_actor_filter_returns_only_matching_rows() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("admin-actor-filter@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-actor-filter@test.com").await;
    let (target_id, _) = app.create_user("actor-filter-target@test.com").await;

    // Same target, two distinct actor_ids on the SecurityAudit rows.
    use ogrenotes_storage::models::security_audit::{SecurityAudit, SecurityAuditAction};
    let mut rows = Vec::new();
    for actor in ["actor-alpha", "actor-beta"] {
        let row = SecurityAudit {
            audit_id: nanoid::nanoid!(8),
            user_id: target_id.clone(),
            actor_id: actor.to_string(),
            action: SecurityAuditAction::LoginSuccess,
            created_at: ogrenotes_common::time::now_usec(),
        };
        app.state.security_audit_repo.create(&row).await.unwrap();
        rows.push(row);
    }

    let (status, json) = app
        .json_request(
            hyper::Method::GET,
            &format!("/api/v1/admin/audit?target={target_id}&actor=actor-alpha"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let entries = json["entries"].as_array().expect("entries");
    assert!(!entries.is_empty(), "alpha row must be returned");
    assert!(
        entries.iter().all(|e| e["actorId"] == "actor-alpha"),
        "every entry must match the actor filter, got: {entries:?}"
    );
    assert!(
        entries.iter().all(|e| e["actorId"] != "actor-beta"),
        "beta rows must be filtered out"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn admin_audit_time_range_filter_narrows_results() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("admin-time-filter@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-time-filter@test.com").await;
    let (target_id, _) = app.create_user("time-filter-target@test.com").await;

    // Two rows with distinguishable timestamps. The middle of the gap
    // becomes the `from` / `to` boundary: only the newer row should
    // pass `from = mid`, only the older should pass `to = mid`.
    use ogrenotes_storage::models::security_audit::{SecurityAudit, SecurityAuditAction};
    let now = ogrenotes_common::time::now_usec();
    let old_ts = now - 1_000_000; // 1 second earlier
    let new_ts = now;
    let mid_ts = (old_ts + new_ts) / 2;

    for (audit_id, ts) in [("old-row", old_ts), ("new-row", new_ts)] {
        let row = SecurityAudit {
            audit_id: audit_id.to_string(),
            user_id: target_id.clone(),
            actor_id: target_id.clone(),
            action: SecurityAuditAction::LoginSuccess,
            created_at: ts,
        };
        app.state.security_audit_repo.create(&row).await.unwrap();
    }

    // from = mid → only the newer row passes the >= comparison.
    let (status, json) = app
        .json_request(
            hyper::Method::GET,
            &format!("/api/v1/admin/audit?target={target_id}&from={mid_ts}"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let entries = json["entries"].as_array().expect("entries");
    let ids: Vec<&str> = entries
        .iter()
        .map(|e| e["auditId"].as_str().unwrap_or(""))
        .collect();
    assert!(ids.contains(&"new-row"), "new-row must pass `from`, got {ids:?}");
    assert!(!ids.contains(&"old-row"), "old-row must NOT pass `from`, got {ids:?}");

    // to = mid → only the older row passes the < comparison.
    let (status, json) = app
        .json_request(
            hyper::Method::GET,
            &format!("/api/v1/admin/audit?target={target_id}&to={mid_ts}"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let entries = json["entries"].as_array().expect("entries");
    let ids: Vec<&str> = entries
        .iter()
        .map(|e| e["auditId"].as_str().unwrap_or(""))
        .collect();
    assert!(ids.contains(&"old-row"), "old-row must pass `to`, got {ids:?}");
    assert!(!ids.contains(&"new-row"), "new-row must NOT pass `to`, got {ids:?}");

    app.cleanup().await;
}

/// Phase 4 M-E6 piece C closure — earlier tests in this file write
/// SecurityAudit rows directly via the repo to exercise the endpoint
/// in isolation. This one closes the loop end-to-end: a real
/// ShareRevoked writer flow (Alice removes Bob from her doc) must
/// surface through `GET /admin/audit?target=<bob>` with the source
/// discriminator set to `security`, the kind set to `shareRevoked`,
/// the actor_id set to Alice (the gap-001 fix — actor != subject),
/// and the detail payload carrying the doc id.
#[tokio::test]
async fn admin_audit_surfaces_share_revoke_writer_end_to_end() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Admin observer + two ordinary users for the share-revoke flow.
    let (admin_id, _) = app.create_user("admin-share-e2e@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-share-e2e@test.com").await;

    let (alice_id, alice_token) = app.create_user("share-e2e-alice@test.com").await;
    let (bob_id, _) = app.create_user("share-e2e-bob@test.com").await;

    // Alice creates a doc, shares to Bob, then revokes.
    let (_, doc) = app
        .json_request(
            hyper::Method::POST,
            "/api/v1/documents",
            Some(&alice_token),
            Some(serde_json::json!({ "title": "ShareE2E", "docType": "document" })),
        )
        .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();

    let (status, _) = app
        .json_request(
            hyper::Method::POST,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "EDIT" })),
        )
        .await;
    assert!(status == 200 || status == 204 || status == 201, "add returned {status}");

    let (status, _) = app
        .json_request(
            hyper::Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/members/{bob_id}"),
            Some(&alice_token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Poll the admin endpoint as the admin. The writer fires via
    // tokio::spawn so the response can race the DDB write.
    for _ in 0..20 {
        let (status, json) = app
            .json_request(
                hyper::Method::GET,
                &format!("/api/v1/admin/audit?target={bob_id}"),
                Some(&admin_token),
                None,
            )
            .await;
        assert_eq!(status, 200);
        let entries = json["entries"].as_array().expect("entries array");
        if let Some(entry) = entries.iter().find(|e| e["kind"] == "shareRevoked") {
            assert_eq!(entry["source"], "security", "must come from SecurityAudit table");
            assert_eq!(entry["targetUserId"], bob_id, "subject = the removed member");
            assert_eq!(
                entry["actorId"], alice_id,
                "actor = the revoker (gap-001 fix; pre-fix this would equal bob_id)",
            );
            assert_eq!(entry["detail"]["docId"], doc_id);
            return app.cleanup().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("shareRevoked row never surfaced through GET /admin/audit within 1s");
}

// ─── Regression: admin user list under a non-PROFILE-heavy table ────
//
// DynamoDB applies a Scan's `Limit` to items examined *before* the
// `SK = PROFILE` filter. Once folder / session / CRDT op-log rows
// outnumber the user rows, a single-page scan returned an empty admin
// user list even though users existed. `UserRepo::list_all` now loops to
// fill the page. These tests seed many non-PROFILE rows ahead of a small
// page limit and assert every user still surfaces.

/// Seed `n` dummy non-PROFILE rows (shaped like CRDT op-log entries).
async fn seed_non_profile_rows(app: &common::TestApp, prefix: &str, n: usize) {
    for i in 0..n {
        app.dynamo_client()
            .put_item()
            .table_name(&app.table_name)
            .item("PK", AttributeValue::S(format!("DOC#{prefix}-{i}")))
            .item("SK", AttributeValue::S(format!("UPDATE#{i:06}")))
            .send()
            .await
            .expect("seed non-profile row");
    }
}

#[tokio::test]
async fn test_admin_list_returns_all_users_past_scan_limit() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // One admin (to call the endpoint) plus two more users: three
    // PROFILE rows total.
    let (admin_id, _) = app.create_user("list-admin@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    // Re-login so the token carries is_admin = true.
    let (_, admin_token) = app.create_user("list-admin@test.com").await;
    app.create_user("list-alice@test.com").await;
    app.create_user("list-bob@test.com").await;

    // Far more non-PROFILE rows than the page limit, so a naive
    // single-page filtered scan would contain no users at all.
    seed_non_profile_rows(&app, "scanfill", 60).await;

    // Small page limit: the pre-fix code read only `limit` items before
    // filtering and returned none of the three users.
    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/admin/users?limit=3",
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 200, "list failed: {json}");

    let emails: std::collections::HashSet<String> = json["users"]
        .as_array()
        .expect("users array")
        .iter()
        .map(|u| u["email"].as_str().unwrap().to_string())
        .collect();

    for expected in [
        "list-admin@test.com",
        "list-alice@test.com",
        "list-bob@test.com",
    ] {
        assert!(emails.contains(expected), "missing {expected}; got {emails:?}");
    }

    app.cleanup().await;
}

#[tokio::test]
async fn test_admin_list_pagination_covers_all_users() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("page-admin@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("page-admin@test.com").await;
    app.create_user("page-alice@test.com").await;
    app.create_user("page-bob@test.com").await;
    // total = 3 users

    seed_non_profile_rows(&app, "pagefill", 40).await;

    // Walk pages of size 2 via the cursor, collecting emails. The cursor
    // is a user PK ("USER#<id>"); its '#' must be percent-encoded in the
    // query string.
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let path = match &cursor {
            Some(c) => format!(
                "/api/v1/admin/users?limit=2&cursor={}",
                c.replace('#', "%23")
            ),
            None => "/api/v1/admin/users?limit=2".to_string(),
        };
        let (status, json) = app
            .json_request(Method::GET, &path, Some(&admin_token), None)
            .await;
        assert_eq!(status, 200, "page failed: {json}");
        for u in json["users"].as_array().unwrap() {
            seen.push(u["email"].as_str().unwrap().to_string());
        }
        match json["nextCursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }

    let unique: std::collections::HashSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), 3, "expected 3 unique users, saw {seen:?}");
    for expected in [
        "page-admin@test.com",
        "page-alice@test.com",
        "page-bob@test.com",
    ] {
        assert!(
            unique.iter().any(|e| e.as_str() == expected),
            "missing {expected}: {seen:?}"
        );
    }

    app.cleanup().await;
}

/// Gap #4 of the test-coverage plan. Covers commit 89ca740 which
/// shipped the force-compact endpoint with zero coverage. The
/// endpoint is the operational recovery path for docs that have
/// accumulated degenerate UPDATE# rows from the pre-d92dac4
/// bridge bug — without this test, any future refactor of the
/// compaction flow could silently change the contract (auth
/// gating, response shape, or side effects on the store).
///
/// One test covers all three response paths plus the side
/// effects, so the contract is locked down end-to-end.
#[tokio::test]
async fn test_force_compact_document_admin_only_and_bumps_snapshot() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (owner_id, owner_token) = app.create_user("doc-owner@test.com").await;
    let doc_id = app.create_doc(&owner_token, "Compact Me", None).await;
    let path = format!("/api/v1/admin/documents/{doc_id}/compact");

    // 1. No bearer → 401.
    let (status, _) = app
        .json_request(Method::POST, &path, None, None)
        .await;
    assert_eq!(status, 401, "no-auth POST must be 401");

    // 2. Bearer for a non-admin user → 403.
    let regular_token = app.create_user_token("regular@test.com").await;
    let (status, _) = app
        .json_request(Method::POST, &path, Some(&regular_token), None)
        .await;
    assert_eq!(status, 403, "non-admin POST must be 403");

    // 3. Admin path: seed N degenerate UPDATE# rows, populate the
    //    room registry so force-compact can read the in-memory
    //    state, call the endpoint, assert response shape + side
    //    effects.
    let (admin_id, _) = app.create_user("admin@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    // Re-login to get a token whose claims carry is_admin=true.
    let (_, admin_token) = app.create_user("admin@test.com").await;

    const N_ROWS: usize = 20;
    let mut truth = ogrenotes_collab::document::OgreDoc::new();
    for i in 0..N_ROWS {
        let baseline_sv = truth.state_vector();
        {
            use yrs::{Transact, WriteTxn, types::xml::{XmlFragment, XmlOut}};
            let doc = truth.inner();
            let mut txn = doc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");
            let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) else {
                panic!("expected initial paragraph element");
            };
            para.insert(
                &mut txn,
                0,
                yrs::types::xml::XmlTextPrelim::new(&format!("seed-row-{i}-")),
            );
        }
        let diff = truth.encode_diff(&baseline_sv).expect("encode_diff");
        let update = ogrenotes_storage::models::document::DocUpdate {
            doc_id: doc_id.clone(),
            clock: format!("{}_{:08}", ogrenotes_common::time::now_usec(), i),
            update_bytes: diff,
            user_id: "owner".to_string(),
            created_at: ogrenotes_common::time::now_usec(),
            client_version: None,
        };
        app.state
            .doc_repo
            .append_update(&update)
            .await
            .expect("append_update");
    }

    // Pre-condition: the seeded rows are visible.
    let pending_before = app
        .state
        .doc_repo
        .get_pending_updates(&doc_id, usize::MAX)
        .await
        .expect("get_pending_updates");
    assert_eq!(pending_before.len(), N_ROWS);

    // Force-compact requires the room to be in the registry; the
    // periodic compaction worker populates it via WS connect, but
    // here we seed it directly with the in-memory truth so the
    // snapshot reflects the cumulative UPDATE# state.
    let _ = app.state.room_registry.get_or_insert(&doc_id, truth);

    let (status, body) = app
        .json_request(Method::POST, &path, Some(&admin_token), None)
        .await;
    assert_eq!(status, 200, "admin POST must be 200, got body {body:?}");

    // Response shape per bb22494 (camelCase via
    // serde(rename_all_fields = "camelCase")).
    assert_eq!(body["result"], "compacted");
    assert_eq!(body["snapshotVersion"], 2);
    assert_eq!(body["updatesPruned"], N_ROWS as u64);

    // Post-condition: the rows are gone.
    let pending_after = app
        .state
        .doc_repo
        .get_pending_updates(&doc_id, usize::MAX)
        .await
        .expect("get_pending_updates");
    assert!(
        pending_after.is_empty(),
        "all {N_ROWS} seeded rows should be pruned, found {} remaining",
        pending_after.len(),
    );

    // The privileged compaction is durably audited: a DocCompacted
    // SecurityAudit row keyed on the doc owner (subject) with the admin as
    // actor, so it surfaces in GET /admin/audit. Writer is fire-and-forget.
    let mut found = None;
    for _ in 0..20 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(&owner_id, 20)
            .await
            .unwrap();
        if let Some(r) = rows.into_iter().find(|r| {
            matches!(
                &r.action,
                ogrenotes_storage::models::security_audit::SecurityAuditAction::DocCompacted { doc_id: d }
                    if d == &doc_id
            )
        }) {
            found = Some(r);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let row = found.expect("force-compact must write a DocCompacted audit row for the owner");
    assert_eq!(row.user_id, owner_id, "subject is the document owner");
    assert_eq!(row.actor_id, admin_id, "actor is the admin who compacted");

    app.cleanup().await;
}

/// Regression: when the last WS client leaves a doc that has accumulated
/// UPDATE# rows, the disconnect path must snapshot + prune the op log (not
/// just drop the room). Without this a WS-only-edited doc's op log grows
/// without bound, because once the room is removed the periodic compactor
/// never sees it.
#[tokio::test]
async fn compact_or_remove_on_empty_compacts_when_pending() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_owner_id, owner_token) = app.create_user("compact-pending@test.com").await;
    let doc_id = app.create_doc(&owner_token, "Pending", None).await;

    // Seed a few UPDATE# rows plus a room whose in-memory state reflects them.
    const N: usize = 3;
    let mut truth = ogrenotes_collab::document::OgreDoc::new();
    for i in 0..N {
        let baseline = truth.state_vector();
        {
            use yrs::{types::xml::{XmlFragment, XmlOut}, Transact, WriteTxn};
            let doc = truth.inner();
            let mut txn = doc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");
            let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) else {
                panic!("expected initial paragraph element");
            };
            para.insert(&mut txn, 0, yrs::types::xml::XmlTextPrelim::new(&format!("row-{i}-")));
        }
        let diff = truth.encode_diff(&baseline).expect("encode_diff");
        app.state
            .doc_repo
            .append_update(&ogrenotes_storage::models::document::DocUpdate {
                doc_id: doc_id.clone(),
                clock: format!("{}_{:08}", ogrenotes_common::time::now_usec(), i),
                update_bytes: diff,
                user_id: "owner".to_string(),
                created_at: ogrenotes_common::time::now_usec(),
                client_version: None,
            })
            .await
            .expect("append_update");
    }
    let _ = app.state.room_registry.get_or_insert(&doc_id, truth);

    // Pre: pending rows present, snapshot at the create-time version.
    assert!(app.state.doc_repo.has_pending_updates(&doc_id).await.unwrap());
    let v_before = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().snapshot_version;

    ogrenotes_api::compaction::compact_or_remove_on_empty(
        &app.state.room_registry,
        &app.state.doc_repo,
        &doc_id,
    )
    .await;

    // Post: op log pruned, snapshot version bumped, empty room dropped.
    assert!(
        !app.state.doc_repo.has_pending_updates(&doc_id).await.unwrap(),
        "pending UPDATE# rows must be pruned by the disconnect compaction"
    );
    let v_after = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().snapshot_version;
    assert_eq!(v_after, v_before + 1, "a snapshot must be written");
    assert!(
        app.state.room_registry.get(&doc_id).is_none(),
        "empty room must be removed"
    );

    app.cleanup().await;
}

/// Regression: a read-only open/close (room present, no UPDATE# rows) must
/// NOT write a snapshot — otherwise every doc view would churn a new
/// SNAPSHOT# version row and re-write an identical S3 blob. The empty room
/// is still dropped.
#[tokio::test]
async fn compact_or_remove_on_empty_skips_snapshot_when_clean() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_owner_id, owner_token) = app.create_user("compact-clean@test.com").await;
    let doc_id = app.create_doc(&owner_token, "Clean", None).await;

    // A room with no pending updates (nothing appended).
    let _ = app
        .state
        .room_registry
        .get_or_insert(&doc_id, ogrenotes_collab::document::OgreDoc::new());
    assert!(!app.state.doc_repo.has_pending_updates(&doc_id).await.unwrap());
    let v_before = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().snapshot_version;

    ogrenotes_api::compaction::compact_or_remove_on_empty(
        &app.state.room_registry,
        &app.state.doc_repo,
        &doc_id,
    )
    .await;

    let v_after = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().snapshot_version;
    assert_eq!(
        v_after, v_before,
        "no snapshot should be written when there are no pending updates"
    );
    assert!(
        app.state.room_registry.get(&doc_id).is_none(),
        "empty room must still be removed"
    );

    app.cleanup().await;
}

/// Regression for the must-fix found reviewing the disconnect-compaction
/// change: if the pending-update check itself fails (DynamoDB unavailable),
/// the disconnect path must LEAVE the room in the registry so the periodic
/// compactor can retry. Evicting it would orphan the op log — the exact bug
/// compact_or_remove_on_empty exists to prevent.
#[tokio::test]
async fn compact_or_remove_on_empty_leaves_room_when_pending_check_fails() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // A DocRepo pointed at a table that doesn't exist → has_pending_updates
    // returns Err (ResourceNotFoundException). Reuses the working clients so
    // the failure is a fast real response, not a connect-timeout retry storm.
    // (has_pending_updates never touches S3, so the cloned S3 client is inert.)
    let broken_repo = ogrenotes_storage::repo::doc_repo::DocRepo::new(
        ogrenotes_storage::dynamo::DynamoClient::new(
            app.dynamo_client().clone(),
            "nonexistent-table-xyz".to_string(),
        ),
        app.state.doc_repo.s3().clone(),
    );

    let registry = ogrenotes_collab::room::RoomRegistry::new();
    let _ = registry.get_or_insert("doc-err-path", ogrenotes_collab::document::OgreDoc::new());

    ogrenotes_api::compaction::compact_or_remove_on_empty(&registry, &broken_repo, "doc-err-path")
        .await;

    assert!(
        registry.get("doc-err-path").is_some(),
        "room must survive a pending-update query failure so the periodic compactor can retry"
    );

    app.cleanup().await;
}

/// `POST /admin/users/{id}/enable` reactivates a disabled user and writes an
/// `Enable` AdminAudit row. Disable/promote/demote/set-ask all had audit
/// tests; enable did not.
#[tokio::test]
async fn admin_enable_reactivates_and_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, admin_token) = app.create_user("admin-enable@test.com").await;
    app.state.user_repo.set_admin(&admin_id, true).await.unwrap();
    let (target_id, _) = app.create_user("victim-enable@test.com").await;

    // Disable first so enable is a real state change.
    app.state.user_repo.set_disabled(&target_id, true).await.unwrap();

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{target_id}/enable"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // User is re-enabled.
    let user = app.state.user_repo.get_by_id(&target_id).await.unwrap().unwrap();
    assert!(!user.is_disabled, "enable must clear the disabled flag");

    // Audit row.
    let row = wait_for_audit_row(&app, &target_id, AdminAuditAction::Enable, &admin_id).await;
    assert_eq!(row.target_user_id, target_id);
    assert_eq!(row.actor_id, admin_id);
    assert_eq!(row.action, AdminAuditAction::Enable);

    app.cleanup().await;
}

/// `GET /admin/users/{id}` requires admin, 404s on an unknown id, and returns
/// the user's admin view on success. The single-user GET had no coverage.
#[tokio::test]
async fn admin_get_user_gates_and_returns_user() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, admin_token) = app.create_user("admin-getuser@test.com").await;
    app.state.user_repo.set_admin(&admin_id, true).await.unwrap();
    let (target_id, _) = app.create_user("getuser-target@test.com").await;
    let regular_token = app.create_user_token("getuser-regular@test.com").await;

    // Non-admin is forbidden.
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/admin/users/{target_id}"),
            Some(&regular_token),
            None,
        )
        .await;
    assert_eq!(status, 403, "non-admin cannot read a user via the admin API");

    // Unknown id is 404.
    let (status, _) = app
        .json_request(
            Method::GET,
            "/api/v1/admin/users/nonexistent-user-id",
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 404);

    // Admin happy path.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/admin/users/{target_id}"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["id"].as_str().unwrap(), target_id);
    assert_eq!(json["email"].as_str().unwrap(), "getuser-target@test.com");
    assert_eq!(json["isDisabled"], false);

    app.cleanup().await;
}

/// `GET /admin/metrics` requires admin and returns a JSON snapshot object.
/// The endpoint had no coverage of its admin gate or response shape.
#[tokio::test]
async fn admin_metrics_snapshot_gated_and_returns_json() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let regular_token = app.create_user_token("metrics-regular@test.com").await;
    let (admin_id, admin_token) = app.create_user("metrics-admin@test.com").await;
    app.state.user_repo.set_admin(&admin_id, true).await.unwrap();

    // Non-admin forbidden.
    let (status, _) = app
        .json_request(Method::GET, "/api/v1/admin/metrics", Some(&regular_token), None)
        .await;
    assert_eq!(status, 403);

    // Admin gets a JSON object.
    let (status, json) = app
        .json_request(Method::GET, "/api/v1/admin/metrics", Some(&admin_token), None)
        .await;
    assert_eq!(status, 200);
    assert!(json.is_object(), "metrics snapshot must be a JSON object, got: {json}");

    app.cleanup().await;
}

/// The admin repair endpoint canonicalizes invalid LiveApp
/// attributes on a doc and emits a SecurityAudit row. This test
/// exercises the happy path: a document has a corrupt card color;
/// admin invokes repair; the response reports at least one
/// touched node and the doc's LiveApp attrs subsequently pass
/// walk_liveapp_violations cleanly.
#[tokio::test]
async fn admin_repair_liveapp_attrs_canonicalizes_and_returns_report() {
    use hyper::Method;
    use ogrenotes_collab::document::OgreDoc;
    use ogrenotes_collab::schema::NodeType;
    use yrs::types::xml::{Xml, XmlElementPrelim, XmlFragment};
    use yrs::{Transact, WriteTxn};

    common::require_infra!();
    let app = common::TestApp::new().await;

    // Owner creates a doc.
    let (owner_id, owner_token) = app.create_user("liveapp-repair-owner@test.com").await;
    let doc_id = app
        .create_doc(&owner_token, "Repair Target", None)
        .await;

    // Build a doc bytes payload with a Kanban card that has an
    // invalid color, then upload via put_content with the gate
    // exempted so it lands.
    let doc = OgreDoc::new();
    {
        let mut txn = doc.inner().transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        let n = frag.len(&txn);
        if n > 0 {
            frag.remove_range(&mut txn, 0, n);
        }
        let board = frag.insert(
            &mut txn,
            0,
            XmlElementPrelim::empty(NodeType::Kanban.tag_name()),
        );
        let col = board.insert(
            &mut txn,
            0,
            XmlElementPrelim::empty(NodeType::KanbanColumn.tag_name()),
        );
        col.insert_attribute(&mut txn, "title", "To Do");
        let card = col.insert(
            &mut txn,
            0,
            XmlElementPrelim::empty(NodeType::KanbanCard.tag_name()),
        );
        card.insert_attribute(&mut txn, "title", "Fix");
        card.insert_attribute(&mut txn, "color", "chartreuse");
    }
    let state_bytes = doc.to_state_bytes();

    // Land the bad state via put_content while the doc is exempt.
    let mut app_ex = app;
    app_ex.set_liveapp_gate_exempt_doc_ids(&[&doc_id]);
    let (status, _) = app_ex
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&owner_token),
            state_bytes,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);
    // Clear the exemption so repair proves it works without it.
    app_ex.set_liveapp_gate_exempt_doc_ids(&[]);

    // Create an admin and hit the repair endpoint.
    let (admin_id, _) = app_ex.create_user("liveapp-repair-admin@test.com").await;
    app_ex.state.user_repo.set_admin(&admin_id, true).await.unwrap();
    let (_, admin_token) = app_ex.create_user("liveapp-repair-admin@test.com").await;

    let (status, json) = app_ex
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/documents/{doc_id}/repair-liveapp-attrs"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 200, "repair endpoint should 200, got {json}");
    assert!(
        json["nodesTouched"].as_u64().unwrap_or(0) >= 1,
        "expected at least one node touched, got {json}"
    );

    // SecurityAudit row was emitted keyed on the doc owner.
    let mut found = false;
    for _ in 0..10 {
        let rows = app_ex
            .state
            .security_audit_repo
            .list_for_user(&owner_id, 20)
            .await
            .unwrap();
        if rows.iter().any(|r| {
            matches!(
                r.action,
                ogrenotes_storage::models::security_audit::SecurityAuditAction::LiveAppAttrsRepaired { .. }
            )
        }) {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(found, "LiveAppAttrsRepaired audit row not found for owner");

    app_ex.cleanup().await;
}
