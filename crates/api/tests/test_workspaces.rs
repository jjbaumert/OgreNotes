// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use aws_sdk_dynamodb::types::AttributeValue;
use hyper::Method;

// ─── Helpers ────────────────────────────────────────────────────

async fn create_workspace(app: &common::TestApp, token: &str, name: &str) -> String {
    let body = serde_json::json!({ "name": name });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/workspaces", Some(token), Some(body))
        .await;
    assert_eq!(status, 201, "create_workspace failed: {json}");
    json["id"].as_str().unwrap().to_string()
}

// ─── CRUD ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_workspace() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let body = serde_json::json!({ "name": "Acme Corp" });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/workspaces", Some(&token), Some(body))
        .await;

    assert_eq!(status, 201);
    assert!(json["id"].is_string());
    assert_eq!(json["name"], "Acme Corp");

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_workspace() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let ws_id = create_workspace(&app, &token, "My Workspace").await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 200);
    assert_eq!(json["name"], "My Workspace");

    app.cleanup().await;
}

#[tokio::test]
async fn test_update_workspace() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let ws_id = create_workspace(&app, &token, "Old Name").await;

    let body = serde_json::json!({ "name": "New Name" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/workspaces/{ws_id}"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 204);

    let (_, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(json["name"], "New Name");

    app.cleanup().await;
}

// ─── Members ────────────────────────────────────────────────────

#[tokio::test]
async fn test_list_members_includes_owner() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token) = app.create_user("alice@test.com").await;
    let ws_id = create_workspace(&app, &token, "Team").await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 200);
    let members = json["members"].as_array().unwrap();
    assert!(members.iter().any(|m| m["userId"] == alice_id));

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_and_remove_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let ws_id = create_workspace(&app, &token_a, "Team").await;

    // Add Bob
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

    // Remove Bob
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/workspaces/{ws_id}/members/{bob_id}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 204);

    app.cleanup().await;
}

/// Regression: adding an already-existing member should return 409 Conflict.
#[tokio::test]
async fn test_add_duplicate_member_returns_conflict() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let ws_id = create_workspace(&app, &token_a, "Team").await;

    // Add Bob first time — 204
    let body = serde_json::json!({ "userId": bob_id, "role": "member" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&token_a),
            Some(body.clone()),
        )
        .await;
    assert_eq!(status, 204);

    // Add Bob again — should be 409
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status, 409, "Duplicate member add should return 409 Conflict");

    app.cleanup().await;
}

/// Regression: Admin role should be able to manage members (add/remove).
#[tokio::test]
async fn test_admin_can_manage_members() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let (carol_id, _) = app.create_user("carol@test.com").await;
    let ws_id = create_workspace(&app, &token_a, "Team").await;

    // Add Bob as Admin
    let body = serde_json::json!({ "userId": bob_id, "role": "admin" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status, 204);

    // Bob (Admin) adds Carol — should succeed
    let body = serde_json::json!({ "userId": carol_id, "role": "member" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&token_b),
            Some(body),
        )
        .await;
    assert_eq!(status, 204, "Admin should be able to add members");

    // Bob (Admin) can update workspace name
    let body = serde_json::json!({ "name": "Admin Updated" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/workspaces/{ws_id}"),
            Some(&token_b),
            Some(body),
        )
        .await;
    assert_eq!(status, 204, "Admin should be able to update workspace");

    app.cleanup().await;
}

/// Regular Member should NOT be able to manage members.
#[tokio::test]
async fn test_member_cannot_manage_members() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let (carol_id, _) = app.create_user("carol@test.com").await;
    let ws_id = create_workspace(&app, &token_a, "Team").await;

    // Add Bob as Member
    let body = serde_json::json!({ "userId": bob_id, "role": "member" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/workspaces/{ws_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    // Bob (Member) tries to add Carol — should fail
    let body = serde_json::json!({ "userId": carol_id, "role": "member" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&token_b),
            Some(body),
        )
        .await;
    assert_eq!(status, 403, "Regular member should not be able to add members");

    app.cleanup().await;
}

// ─── Repo-level backfill regressions ────────────────────────────
//
// These tests exercise the DocRepo surface used by the M1 backfill binary
// (`crates/api/src/bin/backfill_workspaces.rs`). They reach into
// `TestApp.state.doc_repo` directly because the behaviours — conditional
// writes, scan pagination — are not exposed through the HTTP API.

#[tokio::test]
async fn test_set_workspace_id_rejects_missing_doc() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // No doc exists with this id. Without attribute_exists(PK), update_item
    // would silently upsert a partial METADATA row with only workspace_id /
    // workspace_id_gsi set, corrupting GSI3 downstream. Assert the guard
    // fires.
    let result = app
        .state
        .doc_repo
        .set_workspace_id("definitely-not-a-real-doc", "some-workspace")
        .await;
    assert!(
        result.is_err(),
        "set_workspace_id should reject a non-existent doc, got {result:?}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_set_workspace_id_updates_gsi_attr() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("docowner@test.com").await;
    let doc_id = app.create_doc(&token, "WS move test", None).await;

    // Move the doc into a workspace.
    let ws_id = "ws-test-move";
    app.state
        .doc_repo
        .set_workspace_id(&doc_id, ws_id)
        .await
        .expect("set_workspace_id should succeed on a real doc");

    // Model-side check.
    let meta = app
        .state
        .doc_repo
        .get(&doc_id)
        .await
        .expect("get doc")
        .expect("doc exists");
    assert_eq!(meta.workspace_id.as_deref(), Some(ws_id));

    // GSI3 check: the row must be discoverable by workspace_id_gsi.
    let items = app
        .state
        .doc_repo
        .query_docs_by_workspace(ws_id)
        .await
        .expect("GSI3 query");
    assert!(
        items.iter().any(|m| m.doc_id == doc_id),
        "doc {doc_id} should be returned by GSI3 workspace query, got {items:?}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_list_all_meta_paginates_across_docs() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("scanner@test.com").await;
    let d1 = app.create_doc(&token, "doc-1", None).await;
    let d2 = app.create_doc(&token, "doc-2", None).await;
    let d3 = app.create_doc(&token, "doc-3", None).await;
    let expected = [d1, d2, d3];

    // Page size 1 forces at least three DynamoDB scan calls plus any
    // evaluated-but-filtered pages. Every DOC# METADATA must appear exactly
    // once across the cursor sequence.
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<(String, String)> = None;
    let mut iterations = 0;
    loop {
        iterations += 1;
        assert!(iterations < 200, "pagination loop ran away (>200 pages)");
        let (batch, next) = app
            .state
            .doc_repo
            .list_all_meta(1, cursor.clone())
            .await
            .expect("list_all_meta");
        for m in batch {
            seen.push(m.doc_id);
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    for id in &expected {
        let count = seen.iter().filter(|s| *s == id).count();
        assert_eq!(count, 1, "doc {id} should appear exactly once, saw {count} in {seen:?}");
    }

    app.cleanup().await;
}

/// Regression: `list_all_meta` must return all documents even when rows
/// that match the `SK = METADATA` filter but are not `DOC#` (folder
/// metadata) outnumber the page limit. DynamoDB applies a Scan's `Limit`
/// before the filter, so the pre-fix single-page scan returned zero
/// documents in that case; `list_all_meta` now loops to fill the page.
#[tokio::test]
async fn test_list_all_meta_returns_all_docs_past_scan_limit() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("meta-scan@test.com").await;

    let d1 = app.create_doc(&token, "m1", None).await;
    let d2 = app.create_doc(&token, "m2", None).await;
    let d3 = app.create_doc(&token, "m3", None).await;

    // Many folder METADATA rows: they pass the SK=METADATA filter but are
    // dropped by the in-code DOC# check, so a naive single page surfaces
    // no documents at all.
    for i in 0..60 {
        app.dynamo_client()
            .put_item()
            .table_name(&app.table_name)
            .item("PK", AttributeValue::S(format!("FOLDER#scanfill-{i}")))
            .item("SK", AttributeValue::S("METADATA".to_string()))
            .send()
            .await
            .expect("seed folder metadata row");
    }

    // A single call with limit >= doc count must return all three docs.
    let (metas, _cursor) = app
        .state
        .doc_repo
        .list_all_meta(3, None)
        .await
        .expect("list_all_meta");

    let ids: std::collections::HashSet<String> = metas.into_iter().map(|m| m.doc_id).collect();
    for id in [&d1, &d2, &d3] {
        assert!(ids.contains(id), "missing {id}; got {ids:?}");
    }

    app.cleanup().await;
}

/// `add_member` rejects the three invalid inputs: adding yourself (400),
/// granting the Owner role via sharing (400), and a nonexistent target user
/// (404). Only the happy and role-escalation paths were covered before.
#[tokio::test]
async fn test_add_member_validation_branches() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (owner_id, owner_token) = app.create_user("ws-val-owner@test.com").await;
    let (bob_id, _) = app.create_user("ws-val-bob@test.com").await;
    let ws_id = create_workspace(&app, &owner_token, "Team").await;

    // Self-add → 400.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&owner_token),
            Some(serde_json::json!({ "userId": owner_id, "role": "member" })),
        )
        .await;
    assert_eq!(status, 400, "cannot add yourself");

    // Granting Owner role → 400.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&owner_token),
            Some(serde_json::json!({ "userId": bob_id, "role": "owner" })),
        )
        .await;
    assert_eq!(status, 400, "cannot grant Owner role via add_member");

    // Unknown target user → 404.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&owner_token),
            Some(serde_json::json!({ "userId": "nonexistent-user-id", "role": "member" })),
        )
        .await;
    assert_eq!(status, 404, "unknown target user");

    app.cleanup().await;
}

/// `remove_member` refuses to remove the workspace owner (400). The owner
/// row is structural — removing it would orphan the workspace.
#[tokio::test]
async fn test_remove_workspace_owner_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (owner_id, owner_token) = app.create_user("ws-rm-owner@test.com").await;
    let ws_id = create_workspace(&app, &owner_token, "Team").await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/workspaces/{ws_id}/members/{owner_id}"),
            Some(&owner_token),
            None,
        )
        .await;
    assert_eq!(status, 400, "cannot remove the workspace owner");

    app.cleanup().await;
}

/// A non-member cannot view a workspace (404 — existence not leaked) nor list
/// its members (403). Only owner happy paths were covered before.
#[tokio::test]
async fn test_get_workspace_and_members_deny_non_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_owner_id, owner_token) = app.create_user("ws-deny-owner@test.com").await;
    let carol_token = app.create_user_token("ws-deny-carol@test.com").await;
    let ws_id = create_workspace(&app, &owner_token, "Private Team").await;

    // Non-member GET workspace → 404.
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}"),
            Some(&carol_token),
            None,
        )
        .await;
    assert_eq!(status, 404, "non-member cannot view the workspace");

    // Non-member GET members → 403.
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&carol_token),
            None,
        )
        .await;
    assert_eq!(status, 403, "non-member cannot list members");

    app.cleanup().await;
}
