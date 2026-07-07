// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E5 piece F — SCIM token admin endpoints + audit wiring.
//!
//! Covers `/workspaces/:id/scim-tokens` POST/GET/DELETE (workspace
//! admin gated) and verifies that every authenticated SCIM request
//! lands a `ScimTokenUsed { token_id, op }` row in SecurityAudit.

mod common;

use hyper::Method;

#[tokio::test]
async fn admin_can_create_list_revoke_scim_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("admin-scim-tok@test.com").await;

    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "TokenCo" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();

    // Create — plaintext returned once.
    let (status, created) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/scim-tokens"),
            Some(&token),
            Some(serde_json::json!({ "name": "Okta connector" })),
        )
        .await;
    assert_eq!(status, 201);
    let plaintext = created["token"].as_str().unwrap().to_string();
    let token_id = created["tokenId"].as_str().unwrap().to_string();
    assert!(plaintext.starts_with(&format!("{token_id}.")),
        "wire format must be `<token_id>.<secret>`: got {plaintext}");
    assert_eq!(created["name"].as_str().unwrap(), "Okta connector");

    // List — the new token appears WITHOUT any plaintext / hash.
    let (status, list) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/scim-tokens"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["tokenId"].as_str().unwrap(), token_id);
    assert_eq!(arr[0]["isActive"].as_bool().unwrap(), true);
    assert_eq!(arr[0]["disabledAt"].as_i64().unwrap(), 0);
    // Secret hash / plaintext MUST NOT appear in the list response.
    assert!(arr[0].get("secretHash").is_none());
    assert!(arr[0].get("token").is_none());

    // The minted token actually works against the SCIM surface —
    // this is the round-trip proof that piece F's mint integrates
    // with piece B's verify.
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
            None,
            None,
        )
        .await;
    assert_eq!(status, 401, "without the new token, SCIM rejects");

    // Use the token directly.
    use axum::body::Body;
    use http_body_util::BodyExt;
    use hyper::Request;
    use tower::ServiceExt;
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"))
        .header("authorization", format!("Bearer {plaintext}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200, "minted token must authenticate against SCIM");

    // Revoke.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/workspaces/{ws_id}/scim-tokens/{token_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // After revoke, the SCIM bearer no longer authenticates.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"))
        .header("authorization", format!("Bearer {plaintext}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let _ = resp.into_body().collect().await.unwrap().to_bytes();
    // 401 — the row is now disabled. Caught at the verify path's
    // is_active() check.
    assert_eq!(status, 401, "revoked token must fail bearer verify");

    // List shows the revoked row with isActive=false.
    let (_, list) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/scim-tokens"),
            Some(&token),
            None,
        )
        .await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr[0]["isActive"].as_bool().unwrap(), false);
    assert!(arr[0]["disabledAt"].as_i64().unwrap() > 0);

    app.cleanup().await;
}

#[tokio::test]
async fn non_admin_cannot_create_scim_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Owner creates a workspace.
    let (_, owner_token) = app.create_user("scim-owner@test.com").await;
    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&owner_token),
            Some(serde_json::json!({ "name": "AuthZ" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();

    // A different user tries to create a SCIM token.
    let (_, other_token) = app.create_user("scim-other@test.com").await;
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/scim-tokens"),
            Some(&other_token),
            Some(serde_json::json!({ "name": "stealing" })),
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn scim_request_lands_audit_row() {
    // Every authenticated SCIM call must write a
    // SecurityAudit::ScimTokenUsed row, keyed on the workspace_id.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, admin) = app.create_user("scim-audit@test.com").await;
    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&admin),
            Some(serde_json::json!({ "name": "AuditCo" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();

    let (_, created) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/scim-tokens"),
            Some(&admin),
            Some(serde_json::json!({ "name": "audit-test" })),
        )
        .await;
    let bearer = created["token"].as_str().unwrap().to_string();
    let token_id = created["tokenId"].as_str().unwrap().to_string();

    // Drive a single SCIM call so an audit row should land.
    use axum::body::Body;
    use http_body_util::BodyExt;
    use hyper::Request;
    use tower::ServiceExt;
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"))
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let _ = resp.into_body().collect().await.unwrap().to_bytes();

    // The audit write fires in a spawned task — poll for the row
    // rather than sleeping a fixed window. Bounded to ~200ms total
    // (10 × 20ms) so a slow CI host doesn't flake on a cold DDB
    // container, but a fast machine exits the moment the row lands.
    let mut scim_row = None;
    for _ in 0..10 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(&ws_id, 10)
            .await
            .unwrap();
        scim_row = rows.into_iter().find(|r| {
            matches!(
                &r.action,
                ogrenotes_storage::models::security_audit::SecurityAuditAction::ScimTokenUsed { token_id: tid, op } if tid == &token_id && op == "users.list"
            )
        });
        if scim_row.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let scim_row = scim_row
        .expect("audit must include ScimTokenUsed { token_id, op: users.list } within 200ms");
    assert_eq!(scim_row.user_id, ws_id);

    app.cleanup().await;
}

/// `create_scim_token` validates the token name: an empty name and a name
/// over `MAX_TOKEN_NAME_LEN` are both rejected with 400. Only the happy path
/// and the non-admin gate were covered before.
#[tokio::test]
async fn create_scim_token_rejects_invalid_name() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("scim-tok-name@test.com").await;

    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "NameCo" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();

    // Empty name → 400.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/scim-tokens"),
            Some(&token),
            Some(serde_json::json!({ "name": "" })),
        )
        .await;
    assert_eq!(status, 400, "empty token name must be rejected");

    // Over-long name → 400.
    let long_name = "x".repeat(512);
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/scim-tokens"),
            Some(&token),
            Some(serde_json::json!({ "name": long_name })),
        )
        .await;
    assert_eq!(status, 400, "over-length token name must be rejected");

    app.cleanup().await;
}
