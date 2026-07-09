// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// #142 Phase 4 — admin-curated Company template galleries. Covers
// the CRUD surface plus the fold-into-`list_templates` behavior.

mod common;

use hyper::Method;

/// Make `alice` a member of her own default workspace's Admin role so the
/// require_workspace_admin gate lets her CRUD galleries there. Returns
/// (alice_id, alice_token, workspace_id).
async fn make_admin(app: &common::TestApp, email: &str) -> (String, String, String) {
    let (alice_id, alice_token) = app.create_user(email).await;
    let ws_id = app
        .state
        .user_repo
        .get_by_id(&alice_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .expect("default workspace");
    app.state
        .workspace_repo
        .add_member(&ogrenotes_storage::models::workspace::WorkspaceMember {
            workspace_id: ws_id.clone(),
            user_id: alice_id.clone(),
            role: ogrenotes_storage::models::WorkspaceRole::Admin,
            joined_at: 0,
        })
        .await
        .unwrap();
    (alice_id, alice_token, ws_id)
}

#[tokio::test]
async fn test_gallery_crud_roundtrip() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token, ws_id) = make_admin(&app, "gallery-crud@test.com").await;

    // Real docs — the create/update path now verifies each doc_id
    // resolves to a live document, so fake ids would 400.
    let doc_a = app.create_doc(&token, "Doc A", None).await;
    let doc_b = app.create_doc(&token, "Doc B", None).await;
    let doc_c = app.create_doc(&token, "Doc C", None).await;

    // Create.
    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&token),
            Some(serde_json::json!({ "name": "Engineering", "docIds": [doc_a, doc_b] })),
        )
        .await;
    assert_eq!(s, 201, "body: {body}");
    let gallery_id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["name"].as_str(), Some("Engineering"));
    assert_eq!(
        body["docIds"].as_array().map(|a| a.len()),
        Some(2),
    );

    // List.
    let (_, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(body["galleries"].as_array().map(|a| a.len()), Some(1));

    // Get by id.
    let (s, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(body["name"].as_str(), Some("Engineering"));

    // Patch — rename + add a doc.
    let (s, body) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&token),
            Some(
                serde_json::json!({ "name": "Eng Templates", "docIds": [doc_a, doc_b, doc_c] }),
            ),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(body["name"].as_str(), Some("Eng Templates"));
    assert_eq!(body["docIds"].as_array().map(|a| a.len()), Some(3));

    // Delete.
    let (s, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 204);

    // Get after delete → 404.
    let (s, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_non_admin_cannot_crud_galleries() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_admin_id, admin_token, ws_id) = make_admin(&app, "gallery-perm-admin@test.com").await;
    let (_bob_id, bob_token) = app.create_user("gallery-perm-bob@test.com").await;

    // Bob isn't a member of Alice's workspace at all — POST is 403.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&bob_token),
            Some(serde_json::json!({ "name": "Attempt" })),
        )
        .await;
    assert_eq!(s, 403);

    // Admin can create it.
    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&admin_token),
            Some(serde_json::json!({ "name": "Real" })),
        )
        .await;
    assert_eq!(s, 201);
    let gallery_id = body["id"].as_str().unwrap().to_string();

    // Bob still can't modify or delete it.
    let (s, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&bob_token),
            Some(serde_json::json!({ "name": "Hacked" })),
        )
        .await;
    assert_eq!(s, 403);
    let (s, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(s, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_gallery_validation() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token, ws_id) = make_admin(&app, "gallery-validation@test.com").await;

    // Empty / whitespace-only name → 400.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&token),
            Some(serde_json::json!({ "name": "   " })),
        )
        .await;
    assert_eq!(s, 400, "whitespace-only name must be rejected");

    // Overlong name → 400. The cap is 80; give it a comfortable overshoot.
    let long = "x".repeat(200);
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&token),
            Some(serde_json::json!({ "name": long })),
        )
        .await;
    assert_eq!(s, 400, "overlong name must be rejected");

    app.cleanup().await;
}

/// A template referenced by a company gallery shows up in
/// `list_templates` tagged `{type: "company", galleryId, galleryName}` —
/// even when it doesn't otherwise fit the caller's owned / workspace /
/// samples query. Also tests that the same doc surfaces separately
/// under "Mine" AND under the gallery.
#[tokio::test]
async fn test_company_gallery_folds_into_list_templates() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token, ws_id) = make_admin(&app, "gallery-fold@test.com").await;

    // Alice creates + marks her own template.
    let doc_id = app.create_doc(&token, "My Template", None).await;
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/template"),
            Some(&token),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 204);

    // Admin adds it to a company gallery.
    let (_, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&token),
            Some(serde_json::json!({
                "name": "Onboarding",
                "docIds": [doc_id],
            })),
        )
        .await;
    let gallery_id = body["id"].as_str().unwrap().to_string();

    // The picker now shows the doc twice: once as Mine, once as Company.
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&token), None)
        .await;
    let list = body.as_array().unwrap();
    let rows: Vec<_> = list
        .iter()
        .filter(|r| r["id"].as_str() == Some(doc_id.as_str()))
        .collect();
    assert_eq!(rows.len(), 2, "doc should appear under Mine AND Company");

    let has_mine = rows
        .iter()
        .any(|r| r["gallery"]["type"].as_str() == Some("mine"));
    let has_company = rows.iter().any(|r| {
        r["gallery"]["type"].as_str() == Some("company")
            && r["gallery"]["galleryId"].as_str() == Some(gallery_id.as_str())
            && r["gallery"]["galleryName"].as_str() == Some("Onboarding")
    });
    assert!(has_mine, "one row should be gallery=mine");
    assert!(has_company, "one row should be gallery=company with the id/name; got {rows:?}");

    app.cleanup().await;
}

/// POST/PATCH must reject doc ids that don't resolve to a live document.
/// A curl typo or a paste from a mock list otherwise persists silently
/// and the row simply disappears at picker time.
#[tokio::test]
async fn test_gallery_rejects_unknown_doc_ids() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token, ws_id) = make_admin(&app, "gallery-unknown@test.com").await;

    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&token),
            Some(serde_json::json!({
                "name": "Bogus",
                "docIds": ["doc-does-not-exist"],
            })),
        )
        .await;
    assert_eq!(s, 400, "unknown doc_id must be rejected; body: {body}");

    app.cleanup().await;
}

/// PATCH `{}` must be a no-op — no DDB rewrite, no audit row. Assert on
/// the returned `updatedAt` staying equal to the value from the initial
/// create response.
#[tokio::test]
async fn test_empty_patch_is_noop() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token, ws_id) = make_admin(&app, "gallery-noop-patch@test.com").await;

    let (_, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&token),
            Some(serde_json::json!({ "name": "Stable" })),
        )
        .await;
    let gallery_id = body["id"].as_str().unwrap().to_string();
    let created_at = body["updatedAt"].as_i64().unwrap();

    let (s, body) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&token),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        body["updatedAt"].as_i64(),
        Some(created_at),
        "empty PATCH must not bump updatedAt"
    );

    app.cleanup().await;
}

/// A PATCH that sends `docIds: []` without `clearMembership: true` must
/// be rejected — the empty array is more often a client-defaulting bug
/// than a deliberate wipe, and losing the whole membership silently is
/// a hard user pothole.
#[tokio::test]
async fn test_empty_docids_patch_requires_explicit_flag() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token, ws_id) = make_admin(&app, "gallery-wipe-guard@test.com").await;
    let doc = app.create_doc(&token, "Doc", None).await;

    let (_, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&token),
            Some(serde_json::json!({ "name": "Grp", "docIds": [doc] })),
        )
        .await;
    let gallery_id = body["id"].as_str().unwrap().to_string();

    // Bare `docIds: []` must fail.
    let (s, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&token),
            Some(serde_json::json!({ "docIds": [] })),
        )
        .await;
    assert_eq!(s, 400, "empty docIds without clearMembership must be 400");

    // Same PATCH with the explicit flag succeeds and clears the row.
    let (s, body) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&token),
            Some(serde_json::json!({ "docIds": [], "clearMembership": true })),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(body["docIds"].as_array().map(|a| a.len()), Some(0));

    app.cleanup().await;
}

/// Gallery mutations are admin curation of workspace-visible shared state
/// and each write path emits a SecurityAudit row (create → Created,
/// PATCH → Updated, DELETE → Deleted), all self-events keyed on the acting
/// admin. None of the three writers had coverage — the CRUD roundtrip
/// never looks at the audit table.
#[tokio::test]
async fn gallery_mutations_write_security_audit_rows() {
    use ogrenotes_storage::models::security_audit::SecurityAuditAction;

    common::require_infra!();
    let app = common::TestApp::new().await;
    let (alice_id, token, ws_id) = make_admin(&app, "gallery-audit@test.com").await;
    let doc_a = app.create_doc(&token, "Audited Doc", None).await;

    // Poll for a matching row — the writer fires via tokio::spawn, so the
    // HTTP response can race the DDB write (same 10×20ms bound as the
    // audit-writer suite).
    async fn wait_for_audit(
        app: &common::TestApp,
        user_id: &str,
        matcher: impl Fn(&SecurityAuditAction) -> bool,
    ) -> ogrenotes_storage::models::security_audit::SecurityAudit {
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
        panic!("expected gallery SecurityAudit row for user {user_id} within 200ms");
    }

    // Create → TemplateGalleryCreated.
    let (status, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries"),
            Some(&token),
            Some(serde_json::json!({ "name": "Audited", "docIds": [doc_a] })),
        )
        .await;
    assert_eq!(status, 201, "body: {body}");
    let gallery_id = body["id"].as_str().unwrap().to_string();

    let row = wait_for_audit(&app, &alice_id, |a| {
        matches!(
            a,
            SecurityAuditAction::TemplateGalleryCreated { workspace_id: w, gallery_id: g }
                if w == &ws_id && g == &gallery_id
        )
    })
    .await;
    assert_eq!(row.actor_id, alice_id, "self-event: actor is the curating admin");

    // PATCH → TemplateGalleryUpdated.
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&token),
            Some(serde_json::json!({ "name": "Audited v2" })),
        )
        .await;
    assert_eq!(status, 200);

    wait_for_audit(&app, &alice_id, |a| {
        matches!(
            a,
            SecurityAuditAction::TemplateGalleryUpdated { workspace_id: w, gallery_id: g }
                if w == &ws_id && g == &gallery_id
        )
    })
    .await;

    // DELETE → TemplateGalleryDeleted.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/admin/workspaces/{ws_id}/template-galleries/{gallery_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    wait_for_audit(&app, &alice_id, |a| {
        matches!(
            a,
            SecurityAuditAction::TemplateGalleryDeleted { workspace_id: w, gallery_id: g }
                if w == &ws_id && g == &gallery_id
        )
    })
    .await;

    app.cleanup().await;
}
