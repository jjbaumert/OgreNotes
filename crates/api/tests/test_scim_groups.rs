// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E5 piece E — `/scim/v2/workspaces/<id>/Groups` and
//! the three discovery endpoints.
//!
//! Groups in v1 are 1-to-1 with workspaces — the group_id MUST
//! equal the URL's ws_id. Listing returns a single Resource;
//! fetching a different group_id returns 404. PATCH supports
//! `add`/`replace`/`remove` operations on `members` only.

mod common;

use hyper::Method;

use ogrenotes_api::middleware::scim_auth::mint_token;
use ogrenotes_storage::models::workspace_scim_token::WorkspaceScimToken;

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
            Some(serde_json::json!({ "name": "GroupsCo" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();

    let minted = mint_token().unwrap();
    let now = ogrenotes_common::time::now_usec();
    app.state
        .workspace_scim_token_repo
        .put(&WorkspaceScimToken {
            workspace_id: ws_id.clone(),
            token_id: minted.token_id.clone(),
            secret_hash: minted.secret_hash.clone(),
            name: "test connector".to_string(),
            created_at: now,
            last_used_at: 0,
            disabled_at: 0,
        })
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
    let body_bytes = body.map(|v| serde_json::to_vec(&v).unwrap()).unwrap_or_default();
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
async fn scim_list_groups_returns_workspace_as_single_group() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-grp-list@test.com").await;

    let (status, list) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Groups"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(list["totalResults"].as_u64().unwrap(), 1);
    let groups = list["Resources"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["id"].as_str().unwrap(), ws_id);

    app.cleanup().await;
}

#[tokio::test]
async fn scim_get_group_with_wrong_id_returns_404() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-grp-404@test.com").await;

    let (status, body) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Groups/different-id"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(body["status"].as_str().unwrap(), "404");

    app.cleanup().await;
}

#[tokio::test]
async fn scim_patch_group_add_member_then_remove() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-grp-patch@test.com").await;

    // First provision a user via the SCIM Users endpoint so we have
    // a user_id to add to the group. (Doubles as proof that piece D
    // and piece E compose correctly.)
    let user_body = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "externalId": "okta-grpmember",
        "userName": "grpmember@example.com",
        "active": true,
    });
    let (_, user) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(user_body),
    )
    .await;
    let user_id = user["id"].as_str().unwrap().to_string();

    // The user is automatically a workspace member after POST
    // /Users — verify by listing the Group.
    let (_, group) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Groups/{ws_id}"),
        Some(&bearer),
        None,
    )
    .await;
    let members = group["members"].as_array().unwrap();
    assert!(members.iter().any(|m| m["value"].as_str() == Some(&user_id)));

    // PATCH remove that member — Okta's group-member sync pattern.
    let remove_patch = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [
            { "op": "remove", "path": "members", "value": [{ "value": user_id }] }
        ]
    });
    let (status, after_remove) = scim_request(
        &app,
        Method::PATCH,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Groups/{ws_id}"),
        Some(&bearer),
        Some(remove_patch),
    )
    .await;
    assert_eq!(status, 200);
    let members = after_remove["members"].as_array().unwrap();
    assert!(!members.iter().any(|m| m["value"].as_str() == Some(&user_id)),
        "member must be gone after remove patch");

    // PATCH add again — untargeted form Okta also uses.
    let add_patch = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [
            { "op": "add", "value": { "members": [{ "value": user_id }] } }
        ]
    });
    let (status, after_add) = scim_request(
        &app,
        Method::PATCH,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Groups/{ws_id}"),
        Some(&bearer),
        Some(add_patch),
    )
    .await;
    assert_eq!(status, 200);
    let members = after_add["members"].as_array().unwrap();
    assert!(members.iter().any(|m| m["value"].as_str() == Some(&user_id)),
        "member must be back after add patch");

    app.cleanup().await;
}

#[tokio::test]
async fn scim_patch_group_replace_reconciles_member_list() {
    // Okta's reconciliation-sync mode sends `replace` with the
    // complete target membership. v1 must remove members not in
    // the new list AND add the new ones — NOT just append.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-grp-recon@test.com").await;

    // Provision three users.
    let mut user_ids = Vec::new();
    for ext_id in ["recon-1", "recon-2", "recon-3"] {
        let (_, u) = scim_request(
            &app,
            Method::POST,
            &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
            Some(&bearer),
            Some(serde_json::json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "externalId": ext_id,
                "userName": format!("{ext_id}@example.com"),
                "active": true,
            })),
        )
        .await;
        user_ids.push(u["id"].as_str().unwrap().to_string());
    }

    // Now send a `replace` with only the FIRST user. After this
    // the membership list MUST be exactly {recon-1}, not
    // {recon-1, recon-2, recon-3}.
    let replace_patch = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [
            { "op": "replace", "value": { "members": [{ "value": user_ids[0] }] } }
        ]
    });
    let (status, _) = scim_request(
        &app,
        Method::PATCH,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Groups/{ws_id}"),
        Some(&bearer),
        Some(replace_patch),
    )
    .await;
    assert_eq!(status, 200);

    let (_, group) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Groups/{ws_id}"),
        Some(&bearer),
        None,
    )
    .await;
    let members = group["members"].as_array().unwrap();
    let member_values: Vec<&str> = members
        .iter()
        .map(|m| m["value"].as_str().unwrap())
        .collect();
    assert!(member_values.contains(&user_ids[0].as_str()));
    assert!(
        !member_values.contains(&user_ids[1].as_str()),
        "replace must remove members not in the new list"
    );
    assert!(
        !member_values.contains(&user_ids[2].as_str()),
        "replace must remove members not in the new list"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn scim_patch_group_rejects_malformed_members_entry() {
    // F-2 regression: a members array with a malformed entry must
    // reject the whole op, not silently partial-succeed. Pre-fix,
    // collect_member_values silently dropped entries without
    // `value` — an IdP typo would partial-apply with no signal.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-grp-malformed@test.com").await;

    let (_, u) = scim_request(
        &app,
        Method::POST,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Users"),
        Some(&bearer),
        Some(serde_json::json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "externalId": "malformed-test",
            "userName": "malformed@example.com",
            "active": true,
        })),
    )
    .await;
    let good_uid = u["id"].as_str().unwrap().to_string();

    // Send an `add` with one good entry and one entry missing
    // `value`. Pre-fix this would have added just the good one;
    // post-fix it rejects the whole op.
    let patch = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [
            { "op": "add", "value": { "members": [
                { "value": good_uid },
                { "display": "no value here" }
            ]}}
        ]
    });
    let (status, body) = scim_request(
        &app,
        Method::PATCH,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Groups/{ws_id}"),
        Some(&bearer),
        Some(patch),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["scimType"].as_str().unwrap(), "invalidValue");

    app.cleanup().await;
}

#[tokio::test]
async fn scim_service_provider_config_returns_capabilities() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-spc@test.com").await;

    let (status, body) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/ServiceProviderConfig"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert!(body["patch"]["supported"].as_bool().unwrap());
    assert!(!body["bulk"]["supported"].as_bool().unwrap());
    assert!(body["filter"]["supported"].as_bool().unwrap());
    assert!(body["authenticationSchemes"].is_array());

    app.cleanup().await;
}

#[tokio::test]
async fn scim_resource_types_lists_user_and_group() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-rt@test.com").await;

    let (status, body) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/ResourceTypes"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let resources = body["Resources"].as_array().unwrap();
    let ids: Vec<&str> = resources.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"User"));
    assert!(ids.contains(&"Group"));

    app.cleanup().await;
}

#[tokio::test]
async fn scim_schemas_endpoint_returns_user_and_group_schemas() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-schemas@test.com").await;

    let (status, body) = scim_request(
        &app,
        Method::GET,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Schemas"),
        Some(&bearer),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let resources = body["Resources"].as_array().unwrap();
    assert_eq!(resources.len(), 2);

    app.cleanup().await;
}

#[tokio::test]
async fn scim_discovery_requires_bearer_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, _) =
        setup_workspace_with_scim_token(&app, "scim-noauth-disc@test.com").await;

    for path in [
        "ServiceProviderConfig",
        "ResourceTypes",
        "Schemas",
        "Groups",
    ] {
        let (status, _) = scim_request(
            &app,
            Method::GET,
            &format!("/api/v1/scim/v2/workspaces/{ws_id}/{path}"),
            None,
            None,
        )
        .await;
        assert_eq!(status, 401, "unauthenticated {path} must be 401");
    }

    app.cleanup().await;
}

/// `patch_group` add rejects a member that doesn't exist (400 invalidValue) —
/// the ghost-member guard. Without it an IdP typo would create a dangling
/// membership row. Untested before (existing tests only add real members).
#[tokio::test]
async fn scim_patch_group_rejects_nonexistent_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id, bearer) =
        setup_workspace_with_scim_token(&app, "scim-grp-ghost@test.com").await;

    let add_ghost = serde_json::json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [
            { "op": "add", "value": { "members": [{ "value": "nonexistent-user-id" }] } }
        ]
    });
    let (status, err) = scim_request(
        &app,
        Method::PATCH,
        &format!("/api/v1/scim/v2/workspaces/{ws_id}/Groups/{ws_id}"),
        Some(&bearer),
        Some(add_ghost),
    )
    .await;
    assert_eq!(status, 400, "adding a nonexistent member must be rejected");
    assert_eq!(err["scimType"].as_str().unwrap(), "invalidValue");

    app.cleanup().await;
}
