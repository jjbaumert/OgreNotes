// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E5 piece D — `/scim/v2/workspaces/<id>/Users` end-to-end.
//!
//! Covers the routes that a real SCIM provisioner (Okta, Entra ID)
//! exercises during a sync cycle: bearer-token auth, JIT create,
//! list with filter, single get, PATCH replace active for
//! deprovision, DELETE. PUT and the richer PATCH paths land with
//! piece-D follow-ups if a real IdP needs them.

mod common;

use hyper::Method;

use ogrenotes_api::middleware::scim_auth::mint_token;
use ogrenotes_storage::models::workspace_scim_token::WorkspaceScimToken;

/// Set up a workspace + provision a SCIM token on it. Returns
/// (admin_token, workspace_id, scim_bearer).
async fn setup_workspace_with_scim_token(
    app: &common::TestApp,
    email: &str,
) -> (String, String, String) {
    let (_, admin_token) = app.create_user(email).await;
    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&admin_token),
            Some(serde_json::json!({ "name": "Acme" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();

    // Mint a SCIM token directly (the admin UI in piece F will do
    // this via HTTP; for now we go straight to the repo so the
    // test isn't blocked on piece F).
    let minted = mint_token().unwrap();
    let now = ogrenotes_common::time::now_usec();
    let row = WorkspaceScimToken {
        workspace_id: ws_id.clone(),
        token_id: minted.token_id.clone(),
        secret_hash: minted.secret_hash.clone(),
        name: "test connector".to_string(),
        created_at: now,
        last_used_at: 0,
        disabled_at: 0,
    };
    app.state
        .workspace_scim_token_repo
        .put(&row)
        .await
        .unwrap();

    (admin_token, ws_id, minted.plaintext)
}

async fn scim_request(
    app: &common::TestApp,
    method: Method,
    path: &str,
    bearer: Option<&str>,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use hyper::Request;
    use tower::ServiceExt;

    let mut req = Request::builder().method(method).uri(path);
    if let Some(b) = bearer {
        req = req.header("authorization", format!("Bearer {b}"));
    }
    if body.is_some() {
        req = req.header("content-type", "application/scim+json");
    }
    let body_bytes = body
        .map(|v| serde_json::to_vec(&v).unwrap())
        .unwrap_or_default();
    let req = req.body(Body::from(body_bytes)).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

#[tokio::test]
async fn scim_users_rejects_missing_bearer() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, _) =
        setup_workspace_with_scim_token(&app, "scim-noauth@test.com").await;

    let (status, body) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        None,
        None,
    )
    .await;
    assert_eq!(status, 401);
    // The error body must be a SCIM-shaped envelope (not the
    // OgreNotes default), so IdPs that strictly parse the SCIM
    // schema don't choke on the response.
    assert_eq!(body["schemas"][0].as_str().unwrap(),
        "urn:ietf:params:scim:api:messages:2.0:Error");
    assert_eq!(body["status"].as_str().unwrap(), "401");

    app.cleanup().await;
}

#[tokio::test]
async fn scim_users_rejects_wrong_bearer() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-badtok@test.com").await;
    // Flip one secret char.
    let bad = format!("{}x", &bearer[..bearer.len() - 1]);

    let (status, _) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bad),
        None,
    )
    .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn scim_create_user_jits_and_lists() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-create@test.com").await;

    // POST a fresh user — matches the shape Okta emits.
    let body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-test-001",
        "userName": "newuser@example.com",
        "name": { "givenName": "New", "familyName": "User" },
        "displayName": "New User",
        "active": true,
        "emails": [{ "value": "newuser@example.com", "type": "work", "primary": true }]
    });
    let (status, created) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(body),
    )
    .await;
    assert_eq!(status, 201);
    let user_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["externalId"].as_str().unwrap(), "okta-test-001");
    assert_eq!(created["userName"].as_str().unwrap(), "newuser@example.com");
    assert_eq!(created["active"].as_bool().unwrap(), true);

    // GET list — the new user must appear.
    let (status, list) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(list["schemas"][0].as_str().unwrap(),
        "urn:ietf:params:scim:api:messages:2.0:ListResponse");
    let resources = list["Resources"].as_array().unwrap();
    assert!(resources.iter().any(|r| r["id"].as_str() == Some(&user_id)));

    // Filter by externalId — Okta's pre-create existence check.
    let (status, filtered) = scim_request(
        &app,
        Method::GET,
        &format!(
            "/api/v1/scim/v2/workspaces/{ws_id}/Users?filter=externalId%20eq%20%22okta-test-001%22"
        ),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let filtered_resources = filtered["Resources"].as_array().unwrap();
    assert_eq!(filtered_resources.len(), 1);
    assert_eq!(filtered_resources[0]["id"].as_str().unwrap(), user_id);

    // GET single.
    let (status, single) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users/{user_id}"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(single["id"].as_str().unwrap(), user_id);

    app.cleanup().await;
}

#[tokio::test]
async fn scim_patch_replace_active_false_deprovisions() {
    // Okta deprovision pattern: PATCH with replace active=false.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-deprov@test.com").await;

    let body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-test-002",
        "userName": "deprov@example.com",
        "active": true,
    });
    let (_, created) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(body),
    )
    .await;
    let user_id = created["id"].as_str().unwrap().to_string();

    // Real Okta PATCH body — untargeted replace.
    let patch = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [
            { "op": "replace", "value": { "active": false } }
        ]
    });
    let (status, patched) = scim_request(
        &app,
        Method::PATCH,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users/{user_id}"),
        Some(&bearer),
        Some(patch),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(patched["active"].as_bool().unwrap(), false);

    app.cleanup().await;
}

#[tokio::test]
async fn scim_delete_soft_disables() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-del@test.com").await;

    let body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-test-003",
        "userName": "todel@example.com",
        "active": true,
    });
    let (_, created) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(body),
    )
    .await;
    let user_id = created["id"].as_str().unwrap().to_string();

    let (status, _) = scim_request(
        &app,
        Method::DELETE,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users/{user_id}"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 204);

    // Subsequent GET should reflect active=false.
    let (status, got) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users/{user_id}"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(got["active"].as_bool().unwrap(), false);

    app.cleanup().await;
}

#[tokio::test]
async fn scim_cross_workspace_user_mutation_is_rejected() {
    // F-1 regression: a SCIM token for workspace A must not be
    // able to mutate, disable, or replace a user who belongs only
    // to workspace B. Pre-fix, any of PUT/PATCH/DELETE would
    // succeed for a global user_id because workspace membership
    // was never checked on the write paths.
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Workspace A with its own SCIM token.
    let (_, ws_a, bearer_a) =
        setup_workspace_with_scim_token(&app, "scim-ws-a@test.com").await;

    // Workspace B owned by a separate admin, with a user provisioned
    // through B's SCIM token.
    let (_, ws_b, bearer_b) =
        setup_workspace_with_scim_token(&app, "scim-ws-b@test.com").await;
    let body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-only-in-b",
        "userName": "only-in-b@example.com",
        "active": true,
    });
    let (_, created) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_b}/Users"),
        Some(&bearer_b),
        Some(body),
    )
    .await;
    let user_id = created["id"].as_str().unwrap().to_string();

    // Workspace A's token tries to disable the workspace-B user
    // via three different paths. Each must return 404 (per the
    // "not visible in this scope" semantics) without disabling.
    let patch = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [{ "op": "replace", "value": { "active": false } }]
    });
    let (status, _) = scim_request(
        &app,
        Method::PATCH,
        &format!("/api/v1/scim/v2/workspaces/{ws_a}/Users/{user_id}"),
        Some(&bearer_a),
        Some(patch),
    )
    .await;
    assert_eq!(status, 404, "PATCH must reject cross-workspace mutation");

    let put_body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "userName": "only-in-b@example.com",
        "active": false,
    });
    let (status, _) = scim_request(
        &app,
        Method::PUT,
        &format!("/api/v1/scim/v2/workspaces/{ws_a}/Users/{user_id}"),
        Some(&bearer_a),
        Some(put_body),
    )
    .await;
    assert_eq!(status, 404, "PUT must reject cross-workspace mutation");

    let (status, _) = scim_request(
        &app,
        Method::DELETE,
        &format!("/api/v1/scim/v2/workspaces/{ws_a}/Users/{user_id}"),
        Some(&bearer_a),
        None,
    )
    .await;
    assert_eq!(status, 404, "DELETE must reject cross-workspace mutation");

    // Workspace B's own token, fetching its own user, must still
    // see active=true — the cross-workspace attempts above must
    // not have flipped the user.
    let (status, got) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_b}/Users/{user_id}"),
        Some(&bearer_b),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        got["active"].as_bool().unwrap(),
        true,
        "user must NOT have been disabled by cross-workspace caller"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn scim_list_with_invalid_filter_returns_scim_error() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-badfil@test.com").await;

    let (status, body) = scim_request(
        &app,
        Method::GET,
        &format!(
            "/api/v1/scim/v2/workspaces/{ws_id}/Users?filter=displayName%20eq%20%22x%22"
        ),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["scimType"].as_str().unwrap(), "invalidFilter");

    app.cleanup().await;
}

/// M-E8 gap-003 — a misconfigured IdP that pushes a multi-KB
/// displayName must be rejected with 400+invalidValue rather than
/// silently bloating the DDB row. 2 KB is well above the
/// SCIM_MAX_FIELD_BYTES (1 KB) hard-reject threshold.
#[tokio::test]
async fn scim_create_rejects_oversize_display_name() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-oversize@test.com").await;

    let oversize_name = "X".repeat(2048);
    let body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-oversize-001",
        "userName": "oversize@example.com",
        "displayName": oversize_name,
        "active": true,
    });
    let (status, err) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(body),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(err["scimType"].as_str().unwrap(), "invalidValue");
    assert!(
        err["detail"].as_str().unwrap_or("").contains("displayName"),
        "error must name the offending field, got: {}",
        err["detail"]
    );

    app.cleanup().await;
}

/// M-E8 gap-003 — a 400-char displayName (above the 256-byte
/// silent-truncate target but below the 1 KB hard-reject) must
/// still provision the user, with the stored name truncated at a
/// char boundary to 256 bytes.
#[tokio::test]
async fn scim_create_truncates_long_but_acceptable_display_name() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-trunc@test.com").await;

    let long_name = "A".repeat(400);
    let body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-trunc-001",
        "userName": "trunc@example.com",
        "displayName": long_name,
        "active": true,
    });
    let (status, created) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(body),
    )
    .await;
    assert_eq!(status, 201, "400-char name should provision, not 400");
    let stored = created["displayName"].as_str().unwrap();
    assert_eq!(stored.len(), 256, "stored name truncated to SCIM_TRUNCATE_BYTES");
    assert!(stored.chars().all(|c| c == 'A'), "truncation preserves prefix");

    app.cleanup().await;
}

/// M-E8 gap-005 — pre-auth rate limit on SCIM endpoints bounds
/// the bcrypt-CPU-DoS lever. The test AppConfig sets
/// `scim_request_rate_limit_per_minute = 5`; the 6th request
/// against the same workspace_id within a minute must 429.
///
/// Also confirms the limit is keyed PER workspace — a separate
/// workspace's SCIM endpoint is unaffected when the first is at
/// its budget cap, so a noisy connector on workspace A can't DoS
/// workspace B's SCIM channel.
#[tokio::test]
async fn scim_endpoint_rate_limit_caps_per_workspace() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_a, bearer_a) =
        setup_workspace_with_scim_token(&app, "scim-rl-a@test.com").await;
    let (_, ws_b, bearer_b) =
        setup_workspace_with_scim_token(&app, "scim-rl-b@test.com").await;

    // 5 valid GETs against ws_a — all 200.
    for i in 1..=5 {
        let (status, _) = scim_request(
            &app,
            Method::GET,
            &format!("/api/v1/scim/v2/workspaces/{ws_a}/Users"),
            Some(&bearer_a),
            None,
        )
        .await;
        assert_eq!(status, 200, "request {i} should still be within budget");
    }

    // 6th — must 429.
    let (status, _) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_a}/Users"),
        Some(&bearer_a),
        None,
    )
    .await;
    assert_eq!(status, 429, "6th request against ws_a must trip the cap");

    // ws_b — independent budget, must still serve.
    let (status, _) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_b}/Users"),
        Some(&bearer_b),
        None,
    )
    .await;
    assert_eq!(status, 200, "ws_b budget is independent of ws_a's");

    app.cleanup().await;
}

/// M-E8 gap-005 — failed-auth requests also count against the
/// budget. The whole point is to bound the bcrypt-CPU lever; an
/// attacker who could get bcrypt-for-free by intentionally failing
/// would invalidate the protection. 5 wrong-bearer requests against
/// the same workspace_id must exhaust the budget so the 6th
/// (regardless of bearer validity) is 429.
#[tokio::test]
async fn scim_rate_limit_counts_failed_auth_too() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, valid_bearer) =
        setup_workspace_with_scim_token(&app, "scim-rl-fail@test.com").await;

    // 5 wrong-bearer attempts — each 401 from auth, but each
    // counts against the rate-limit budget BEFORE bcrypt runs.
    for i in 1..=5 {
        let (status, _) = scim_request(
            &app,
            Method::GET,
            &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
            Some("not-a-real-bearer-token"),
            None,
        )
        .await;
        assert_eq!(status, 401, "wrong-bearer attempt {i} fails auth");
    }

    // 6th attempt — even with the VALID bearer, must 429 because
    // the rate-limit check fires before the auth check.
    let (status, _) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&valid_bearer),
        None,
    )
    .await;
    assert_eq!(
        status, 429,
        "6th request must 429 — failed-auth attempts must count against budget",
    );

    app.cleanup().await;
}

/// M-E8 gap-004 — SCIM-driven deprovision must write a
/// per-user SecurityAudit row keyed PK=USER#<affected> so
/// /admin/audit?target=<user> can surface the event. The
/// workspace_id sits in actor_id (the SCIM token's scope; no
/// individual user is the actor for a SCIM-driven mutation).
#[tokio::test]
async fn scim_delete_writes_per_user_session_revoked_audit() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-deprov-audit@test.com").await;

    // Create the SCIM user.
    let body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-deprov-audit-001",
        "userName": "deprov-audit@example.com",
        "displayName": "Deprov Audit",
        "active": true,
    });
    let (_, created) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(body),
    )
    .await;
    let user_id = created["id"].as_str().unwrap().to_string();

    // SCIM DELETE — soft-disable + audit.
    let (status, _) = scim_request(
        &app,
        Method::DELETE,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users/{user_id}"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 204);

    // Poll for the SessionRevoked row on the affected user's PK.
    use ogrenotes_storage::models::security_audit::SecurityAuditAction;
    let mut found = None;
    for _ in 0..20 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(&user_id, 20)
            .await
            .unwrap();
        if let Some(row) = rows.into_iter().find(|r| {
            matches!(&r.action, SecurityAuditAction::SessionRevoked { reason } if reason == "scim_deprovision")
        }) {
            found = Some(row);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let row = found.expect("expected scim_deprovision SecurityAudit row within 1s");
    assert_eq!(row.user_id, user_id, "subject = affected user");
    assert_eq!(
        row.actor_id, ws_id,
        "actor = workspace_id (the SCIM token's scope identifier)",
    );

    app.cleanup().await;
}

/// M-E8 gap-004 — re-DELETE on an already-disabled user is
/// idempotent and must NOT append a duplicate audit row.
/// Otherwise an hourly IdP reconcile would flood the audit log
/// with no-op deprovisions.
#[tokio::test]
async fn scim_redelete_does_not_append_duplicate_audit() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-redel-audit@test.com").await;

    let body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-redel-001",
        "userName": "redel@example.com",
        "active": true,
    });
    let (_, created) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(body),
    )
    .await;
    let user_id = created["id"].as_str().unwrap().to_string();

    // First DELETE — should land an audit row.
    let url = format!("/api/v1/scim/v2/workspaces/{ws_id}/Users/{user_id}");
    let (status, _) = scim_request(&app, Method::DELETE, &url, Some(&bearer), None).await;
    assert_eq!(status, 204);

    // Poll until the first row lands so the post-2nd-DELETE check
    // can compare counts deterministically.
    use ogrenotes_storage::models::security_audit::SecurityAuditAction;
    let mut count_after_first = 0;
    for _ in 0..20 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(&user_id, 20)
            .await
            .unwrap();
        count_after_first = rows
            .iter()
            .filter(|r| {
                matches!(&r.action, SecurityAuditAction::SessionRevoked { reason } if reason == "scim_deprovision")
            })
            .count();
        if count_after_first == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(count_after_first, 1, "first DELETE should land exactly one audit row");

    // Second DELETE — already disabled, must NOT emit.
    let (status, _) = scim_request(&app, Method::DELETE, &url, Some(&bearer), None).await;
    assert_eq!(status, 204);

    // Give any spurious spawn a chance to land then reassert.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let rows = app
        .state
        .security_audit_repo
        .list_for_user(&user_id, 20)
        .await
        .unwrap();
    let final_count = rows
        .iter()
        .filter(|r| {
            matches!(&r.action, SecurityAuditAction::SessionRevoked { reason } if reason == "scim_deprovision")
        })
        .count();
    assert_eq!(final_count, 1, "re-DELETE on already-disabled must not duplicate");

    app.cleanup().await;
}

/// M-E8 gap-003 — PATCH paths run through the same chokepoint
/// (`update_name`). An oversize displayName in a PATCH body must
/// produce 400+invalidValue, mirroring the create path.
#[tokio::test]
async fn scim_patch_rejects_oversize_display_name() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-patch-oversize@test.com").await;

    let body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-patch-oversize",
        "userName": "patch-oversize@example.com",
        "displayName": "Sane Initial Name",
    });
    let (_, created) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(body),
    )
    .await;
    let user_id = created["id"].as_str().unwrap().to_string();

    let oversize = "Y".repeat(2048);
    let patch = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [
            { "op": "replace", "path": "displayName", "value": oversize }
        ]
    });
    let (status, err) = scim_request(
        &app,
        Method::PATCH,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users/{user_id}"),
        Some(&bearer),
        Some(patch),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(err["scimType"].as_str().unwrap(), "invalidValue");

    app.cleanup().await;
}

/// `create_user` rejects an empty `userName` (400 invalidValue) — the
/// minimal SCIM well-formedness check, previously untested.
#[tokio::test]
async fn scim_create_user_rejects_empty_username() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-empty-un@test.com").await;

    let (status, _) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(serde_json::json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "externalId": "ext-empty-1",
            "userName": "",
            "active": true
        })),
    )
    .await;
    assert_eq!(status, 400, "empty userName must be rejected");

    app.cleanup().await;
}

/// JIT-provisioning a SCIM user whose `userName` (email) collides with an
/// existing non-SCIM account is a uniqueness violation — 409
/// `scimType=uniqueness`. This is the collision the handler explicitly maps
/// (`find_or_create_scim_user` failing on a different-provider email), and it
/// was untested.
#[tokio::test]
async fn scim_create_user_email_collision_conflicts() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-collide-admin@test.com").await;

    // Pre-existing non-SCIM (dev-login) account with this email.
    let collide_email = "collide-existing@example.com";
    let _ = app.create_user(collide_email).await;

    let (status, err) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(serde_json::json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "externalId": "ext-collide",
            "userName": collide_email,
            "active": true
        })),
    )
    .await;
    assert_eq!(status, 409, "email collision with an existing account must conflict");
    assert_eq!(err["scimType"].as_str().unwrap(), "uniqueness");

    app.cleanup().await;
}

/// `replace_user` (PUT) applies a full-resource replace (200) but treats
/// `userName` as immutable — changing it returns 400 `scimType=mutability`.
/// PUT was only exercised for the cross-workspace 404 before.
#[tokio::test]
async fn scim_replace_user_happy_and_username_immutable() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-replace@test.com").await;
    let base = format!("/api/v1/scim/v2/workspaces/{ws_id}/Users");

    // Create.
    let (status, created) = scim_request(
        &app,
        Method::POST,
        &base,
        Some(&bearer),
        Some(serde_json::json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "externalId": "ext-replace-1",
            "userName": "ruser@example.com",
            "displayName": "Original Name",
            "active": true
        })),
    )
    .await;
    assert_eq!(status, 201);
    let user_id = created["id"].as_str().unwrap().to_string();

    // PUT with the SAME userName but a changed displayName + active → 200.
    let (status, replaced) = scim_request(
        &app,
        Method::PUT,
        &format!("{base}/{user_id}"),
        Some(&bearer),
        Some(serde_json::json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "ruser@example.com",
            "displayName": "Renamed Display",
            "active": false
        })),
    )
    .await;
    assert_eq!(status, 200, "full replace with unchanged userName succeeds");
    assert_eq!(replaced["active"].as_bool().unwrap(), false);

    // PUT with a CHANGED userName → 400 mutability.
    let (status, err) = scim_request(
        &app,
        Method::PUT,
        &format!("{base}/{user_id}"),
        Some(&bearer),
        Some(serde_json::json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "changed@example.com",
            "active": false
        })),
    )
    .await;
    assert_eq!(status, 400, "userName is immutable");
    assert_eq!(err["scimType"].as_str().unwrap(), "mutability");

    app.cleanup().await;
}

/// `list_users` honors the `count` / `startIndex` pagination window. With
/// two provisioned users, `count=1` returns a single resource while
/// `totalResults` still reflects the full set. The window was untested.
#[tokio::test]
async fn scim_list_users_pagination_window() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-page@test.com").await;
    let base = format!("/api/v1/scim/v2/workspaces/{ws_id}/Users");

    for (ext, un) in [("ext-p1", "p1@example.com"), ("ext-p2", "p2@example.com")] {
        let (status, _) = scim_request(
            &app,
            Method::POST,
            &base,
            Some(&bearer),
            Some(serde_json::json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "externalId": ext,
                "userName": un,
                "active": true
            })),
        )
        .await;
        assert_eq!(status, 201);
    }

    // Request a one-item window.
    let (status, page) = scim_request(
        &app,
        Method::GET,
        &format!("{base}?count=1&startIndex=1"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        page["Resources"].as_array().unwrap().len(),
        1,
        "count=1 must return exactly one resource"
    );
    assert!(
        page["totalResults"].as_i64().unwrap() >= 2,
        "totalResults reflects the full set, not the page size: {page}"
    );

    app.cleanup().await;
}
