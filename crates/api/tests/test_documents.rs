// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;
use ogrenotes_collab::document::OgreDoc;

// ─── Helpers ───────────────────────────────────────────────────

async fn get_home_folder_id(app: &common::TestApp, token: &str) -> String {
    let (status, json) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(token), None)
        .await;
    assert_eq!(status, 200);
    json["homeFolderId"].as_str().unwrap().to_string()
}

// ─── Create ────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_document() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&token),
            Some(serde_json::json!({})),
        )
        .await;

    assert_eq!(status, 201);
    assert!(json["id"].is_string());
    assert_eq!(json["title"], "Untitled");

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_document_custom_title() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let body = serde_json::json!({ "title": "My Doc" });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/documents", Some(&token), Some(body))
        .await;

    assert_eq!(status, 201);
    assert_eq!(json["title"], "My Doc");

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_document_in_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Work", None).await;

    let body = serde_json::json!({ "folderId": folder_id });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/documents", Some(&token), Some(body))
        .await;

    assert_eq!(status, 201);
    assert_eq!(json["folderId"], folder_id);

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_document_unauthenticated() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            None,
            Some(serde_json::json!({})),
        )
        .await;

    assert_eq!(status, 401);

    app.cleanup().await;
}

// ─── Get ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_get_document() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Test Doc", None).await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 200);
    assert_eq!(json["id"], doc_id);
    assert_eq!(json["title"], "Test Doc");

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_document_not_found() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;

    let (status, _) = app
        .json_request(
            Method::GET,
            "/api/v1/documents/nonexistent_id_12345",
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_document_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;
    let doc_id = app.create_doc(&token_a, "Private", None).await;

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_document_via_folder_membership() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_a_id, token_a) = app.create_user("alice@test.com").await;
    let (user_b_id, token_b) = app.create_user("bob@test.com").await;

    // User A creates a folder and a doc inside it
    let folder_id = app.create_folder(&token_a, "Shared Folder", None).await;
    let doc_id = app.create_doc(&token_a, "Shared Doc", Some(&folder_id)).await;

    // User B cannot access the doc yet
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 403);

    // User A shares the folder with user B
    let share_body = serde_json::json!({
        "userId": user_b_id,
        "accessLevel": "EDIT"
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            Some(share_body),
        )
        .await;
    assert_eq!(status, 204);

    // Now user B can access the doc
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["id"], doc_id);

    // Suppress unused variable warnings
    let _ = user_a_id;

    app.cleanup().await;
}

// ─── Update ────────────────────────────────────────────────────

#[tokio::test]
async fn test_update_document_title() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Old Title", None).await;

    let body = serde_json::json!({ "title": "New" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

// ─── Delete ────────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_document() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "To Delete", None).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_document_non_owner() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;
    let doc_id = app.create_doc(&token_a, "Not Yours", None).await;

    // Non-owner should get 403 Forbidden (not a misleading 404)
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

/// Regression: non-owner with Edit access via shared folder should still get 403
/// (delete requires Own access).
#[tokio::test]
async fn test_delete_document_shared_editor_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_user_a_id, token_a) = app.create_user("alice@test.com").await;
    let (user_b_id, token_b) = app.create_user("bob@test.com").await;

    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Shared Doc", Some(&folder_id)).await;

    // Share folder with Bob as EDIT
    let share_body = serde_json::json!({
        "userId": user_b_id,
        "accessLevel": "EDIT"
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            Some(share_body),
        )
        .await;
    assert_eq!(status, 204);

    // Bob can read (via folder membership) but cannot delete (requires Own)
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

// ─── Content ───────────────────────────────────────────────────

#[tokio::test]
async fn test_put_content_roundtrip() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Content Test", None).await;

    // Build valid Y.Doc bytes
    let doc = ogrenotes_collab::document::OgreDoc::new();
    let state = doc.to_state_bytes();

    // PUT content
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            state.clone(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);

    // GET content back
    let (status, returned) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            vec![],
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(returned, state);

    app.cleanup().await;
}

#[tokio::test]
async fn test_put_content_invalid_ydoc() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Bad Content", None).await;

    let garbage = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03];
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            garbage,
            "application/octet-stream",
        )
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

/// Phase 2a LiveApp gate exercises `put_content`:
/// - default `log` mode never rejects, even for a doc whose
///   LiveApp attrs would fail validation, and
/// - `reject` mode returns 400 with a validation-diagnostic
///   body.
///
/// The 400-vs-204 contrast anchors the `emit_violations_and_should_reject`
/// wiring that would otherwise have no external observer.
#[tokio::test]
async fn test_put_content_liveapp_gate_log_mode_accepts_bad_attrs() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("liveapp-log@test.com").await;
    let doc_id = app.create_doc(&token, "Log Mode", None).await;

    // Build a doc with a Kanban card whose color is not on the
    // six-hue palette — hits the strict-Err path of the block
    // validator. Default `log` mode should still 204.
    let state = build_kanban_doc_with_card_color("javascript:").to_state_bytes();
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            state,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204, "log mode must accept violating LiveApp attrs");

    app.cleanup().await;
}

#[tokio::test]
async fn test_put_content_liveapp_gate_reject_mode_returns_400() {
    common::require_infra!();
    let mut app = common::TestApp::new().await;
    app.set_liveapp_validation_mode("reject");

    let token = app.create_user_token("liveapp-reject@test.com").await;
    let doc_id = app.create_doc(&token, "Reject Mode", None).await;

    let state = build_kanban_doc_with_card_color("javascript:").to_state_bytes();
    let (status, body) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            state,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 400, "reject mode must refuse invalid attrs");
    let body = String::from_utf8(body).unwrap_or_default();
    assert!(
        body.contains("liveapp validation rejected"),
        "expected diagnostic in body, got {body:?}"
    );

    app.cleanup().await;
}

/// gap-001 option 2 — an exempted doc-id bypasses the gate even
/// under reject mode. Locks in the operator escape hatch.
#[tokio::test]
async fn test_put_content_liveapp_gate_exemption_bypasses_reject() {
    common::require_infra!();
    let mut app = common::TestApp::new().await;
    app.set_liveapp_validation_mode("reject");

    let token = app.create_user_token("liveapp-exempt@test.com").await;
    let doc_id = app.create_doc(&token, "Exempt Doc", None).await;

    // Under reject mode without exemption: 400 (baseline from the
    // sibling test above already pins this — no need to re-assert).
    // Now add this doc to the exempt list and confirm the same
    // payload lands with 204.
    app.set_liveapp_gate_exempt_doc_ids(&[&doc_id]);

    let state = build_kanban_doc_with_card_color("javascript:").to_state_bytes();
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            state,
            "application/octet-stream",
        )
        .await;
    assert_eq!(
        status, 204,
        "an exempted doc-id must skip the gate even under reject mode"
    );

    app.cleanup().await;
}

/// Build a Kanban board with one column + one card carrying the
/// given color. Used by the put_content gate tests to construct
/// a doc whose LiveApp attrs violate `validate_card_attrs`.
fn build_kanban_doc_with_card_color(color: &str) -> ogrenotes_collab::document::OgreDoc {
    use ogrenotes_collab::document::OgreDoc;
    use ogrenotes_collab::schema::NodeType;
    use yrs::types::xml::{Xml, XmlElementPrelim, XmlFragment};
    use yrs::{Transact, WriteTxn};

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
        card.insert_attribute(&mut txn, "color", color);
    }
    doc
}

#[tokio::test]
async fn test_put_content_view_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_a_id, token_a) = app.create_user("alice@test.com").await;
    let (user_b_id, token_b) = app.create_user("bob@test.com").await;

    // User A creates a folder with a doc
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "View Only", Some(&folder_id)).await;

    // Share with user B as VIEW only
    let share_body = serde_json::json!({
        "userId": user_b_id,
        "accessLevel": "VIEW"
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            Some(share_body),
        )
        .await;
    assert_eq!(status, 204);

    // User B tries to PUT content -- should be forbidden
    let doc = ogrenotes_collab::document::OgreDoc::new();
    let state = doc.to_state_bytes();
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token_b),
            state,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 403);

    // Suppress unused variable warnings
    let _ = user_a_id;

    app.cleanup().await;
}

// ─── Edit lock (#140) ──────────────────────────────────────────

#[tokio::test]
async fn test_lock_blocks_content_writes_including_owner() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Lockable", None).await;

    let state = ogrenotes_collab::document::OgreDoc::new().to_state_bytes();
    let content_url = format!("/api/v1/documents/{doc_id}/content");

    // Owner can write while unlocked.
    let (status, _) = app
        .bytes_request(Method::PUT, &content_url, Some(&token), state.clone(), "application/octet-stream")
        .await;
    assert_eq!(status, 204);

    // Lock the doc.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/lock"),
            Some(&token),
            Some(serde_json::json!({ "locked": true })),
        )
        .await;
    assert_eq!(status, 204);

    // get_document reflects the lock to the owner, who may still manage it.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["locked"], true);
    assert_eq!(json["canManage"], true);

    // A locked doc is a doc-wide freeze: even the owner's write is rejected.
    let (status, _) = app
        .bytes_request(Method::PUT, &content_url, Some(&token), state.clone(), "application/octet-stream")
        .await;
    assert_eq!(status, 403);

    // The import path is also a content write — it must be frozen too
    // (a locked doc must not be writable by importing a file).
    let boundary = "lockimportboundary";
    let import_body = multipart_body(boundary, "data.csv", "text/csv", b"a,b\n1,2\n");
    let (status, _) = app
        .bytes_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/import"),
            Some(&token),
            import_body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 403);

    // Unlock restores write access.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/lock"),
            Some(&token),
            Some(serde_json::json!({ "locked": false })),
        )
        .await;
    assert_eq!(status, 204);
    let (status, _) = app
        .bytes_request(Method::PUT, &content_url, Some(&token), state, "application/octet-stream")
        .await;
    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_lock_toggle_is_owner_only_and_freezes_editor() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_user_a_id, token_a) = app.create_user("alice@test.com").await;
    let (user_b_id, token_b) = app.create_user("bob@test.com").await;

    // Alice owns a doc shared with Bob as EDIT.
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Shared Doc", Some(&folder_id)).await;
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            Some(serde_json::json!({ "userId": user_b_id, "accessLevel": "EDIT" })),
        )
        .await;
    assert_eq!(status, 204);

    // Bob (an editor, not owner) cannot toggle the lock.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/lock"),
            Some(&token_b),
            Some(serde_json::json!({ "locked": true })),
        )
        .await;
    assert_eq!(status, 403);

    // Alice locks it.
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/lock"),
            Some(&token_a),
            Some(serde_json::json!({ "locked": true })),
        )
        .await;
    assert_eq!(status, 204);

    // Now Bob's content write is frozen by the lock (not just permissions).
    let state = ogrenotes_collab::document::OgreDoc::new().to_state_bytes();
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token_b),
            state,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

// ─── Export ────────────────────────────────────────────────────

#[tokio::test]
async fn test_export_html() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Export Test", None).await;

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/html"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 200);

    app.cleanup().await;
}

#[tokio::test]
async fn test_export_docx() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Export DOCX Test", None).await;

    let (status, bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/docx"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;

    assert_eq!(status, 200);
    // A real .docx is a ZIP — the local-file-header magic "PK\x03\x04"
    // is enough to confirm we emitted a packaged OOXML container rather
    // than an empty body or an error page.
    assert!(bytes.len() > 4, "expected non-trivial docx body");
    assert_eq!(&bytes[..4], b"PK\x03\x04", "export should be a ZIP/OOXML container");

    app.cleanup().await;
}

#[tokio::test]
async fn test_export_pdf() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Export PDF Test", None).await;

    let (status, bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/pdf"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;

    assert_eq!(status, 200);
    // "%PDF" magic confirms a real PDF body rather than empty/error.
    assert!(bytes.len() > 4, "expected non-trivial pdf body");
    assert_eq!(&bytes[..4], b"%PDF", "export should be a PDF document");

    app.cleanup().await;
}

#[tokio::test]
async fn test_export_unsupported_format() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Export Fail", None).await;

    // rtf is not a supported export format (html/markdown/csv/xlsx/docx/pdf are).
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/rtf"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

/// Regression: GET /export/{format} without a bearer token returns 401,
/// and an authenticated stranger gets 404 (existence-leak protection,
/// matching the rest of the doc read paths). The 2026-05-28 frontend
/// bug opened the export URL in a fresh browser tab via
/// `window.open(...)`, which carries no `Authorization` header — and
/// the route silently said 401. This pins the server side of that
/// contract: a missing bearer is rejected, *and* a present-but-wrong
/// principal can't even confirm the doc exists.
#[tokio::test]
async fn test_export_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "AuthCheck", None).await;

    // 1. No bearer → 401.
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/markdown"),
            None,
            None,
        )
        .await;
    assert_eq!(status, 401, "export without bearer must be 401");

    // 2. Bearer for a different user with no access to a *live* doc → 403.
    //    Established contract across all live-doc read/write endpoints
    //    (see test_get_document_forbidden, test_delete_document_non_owner,
    //    test_put_content_view_forbidden). 404 is reserved for trashed
    //    docs and not-found ids — see the `AccessDecision::NotFound` doc
    //    comment on `check_doc_access_allow_deleted`.
    let token_b = app.create_user_token("bob@test.com").await;
    let (cross_status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/markdown"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(
        cross_status, 403,
        "cross-user export on a live doc must 403, matching the rest of the doc read paths",
    );

    app.cleanup().await;
}

/// Markdown export round-trip — seed via POST /documents/import
/// (which had no HTTP-layer test coverage either), then GET the same
/// content back via /export/markdown and assert text round-trips. Two
/// gaps closed in one shape, and a meaningful body assertion that
/// catches "200 but empty / wrong format" regressions.
#[tokio::test]
async fn test_export_markdown_round_trips_text() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("alice@test.com").await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/import",
            Some(&token),
            Some(serde_json::json!({
                "format": "markdown",
                "title": "Roundtrip",
                "content": "# Heading One\n\nBody paragraph text.\n",
            })),
        )
        .await;
    assert_eq!(status, 201, "import should succeed: {json}");
    let doc_id = json["id"].as_str().expect("import returns doc id").to_string();

    let (status, bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/markdown"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200);
    let text = String::from_utf8(bytes).expect("markdown export is utf-8");
    assert!(
        text.contains("Heading One"),
        "heading text missing from markdown export: {text:?}",
    );
    assert!(
        text.contains("Body paragraph text"),
        "body text missing from markdown export: {text:?}",
    );

    app.cleanup().await;
}

/// CSV export round-trip — import CSV via the spreadsheet-import
/// route (already covered separately for the import direction), then
/// export it back and assert every imported cell value reappears in
/// the response.
#[tokio::test]
async fn test_export_csv_round_trips_imported_rows() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Sheet", None).await;

    let boundary = "exportcsvboundary";
    let csv = b"name,score\nalice,10\nbob,20\n";
    let body = multipart_body(boundary, "data.csv", "text/csv", csv);
    let (status, _) = app
        .bytes_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/import"),
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 204);

    let (status, bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/csv"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200);
    let text = String::from_utf8(bytes).expect("csv export is utf-8");
    for expected in ["name", "score", "alice", "10", "bob", "20"] {
        assert!(
            text.contains(expected),
            "csv export missing {expected:?}: {text:?}",
        );
    }

    app.cleanup().await;
}

/// XLSX export emits a real ZIP container. Mirrors the docx/pdf magic
/// checks above; closes the binary-export coverage gap on xlsx.
#[tokio::test]
async fn test_export_xlsx_returns_zip_container() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Sheet", None).await;

    let boundary = "exportxlsxboundary";
    let csv = b"col1,col2\nv1,v2\n";
    let body = multipart_body(boundary, "data.csv", "text/csv", csv);
    let (status, _) = app
        .bytes_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/import"),
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 204);

    let (status, bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/xlsx"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200);
    assert!(bytes.len() > 4, "xlsx body should be non-trivial");
    assert_eq!(&bytes[..4], b"PK\x03\x04", "xlsx should be a ZIP/OOXML container");

    app.cleanup().await;
}

// ─── Regression tests for reviewed defects ─────────────────────

/// Regression: text/html uploads must be blocked (stored XSS via presigned S3 URLs).
#[tokio::test]
async fn test_upload_blocks_text_html() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Blob Test", None).await;

    let body = serde_json::json!({
        "filename": "evil.html",
        "contentType": "text/html"
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 400, "text/html uploads should be blocked");

    app.cleanup().await;
}

/// Regression: text/javascript uploads must be blocked.
#[tokio::test]
async fn test_upload_blocks_text_javascript() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Blob Test", None).await;

    let body = serde_json::json!({
        "filename": "payload.js",
        "contentType": "text/javascript"
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 400, "text/javascript uploads should be blocked");

    app.cleanup().await;
}

/// Regression: text/plain uploads should still be allowed.
#[tokio::test]
async fn test_upload_allows_text_plain() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Blob Test", None).await;

    let body = serde_json::json!({
        "filename": "notes.txt",
        "contentType": "text/plain"
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 200, "text/plain uploads should be allowed");

    app.cleanup().await;
}

/// Regression: after creation, the document must exist AND appear in the folder.
/// Verifies fix for the ordering bug where folder-child was added before the doc existed.
#[tokio::test]
async fn test_create_document_appears_in_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Project", None).await;
    let doc_id = app.create_doc(&token, "My Doc", Some(&folder_id)).await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/folders/{folder_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let children = json["children"].as_array().unwrap();
    assert!(
        children.iter().any(|c| c["childId"].as_str() == Some(&doc_id)),
        "Document should be listed as a folder child"
    );

    app.cleanup().await;
}

/// Regression: export must include pending CRDT updates, not just the S3 snapshot.
/// Previously, export_document only loaded the snapshot and skipped UPDATE# rows.
#[tokio::test]
async fn test_export_html_includes_pending_updates() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Export Pending", None).await;

    // PUT initial content (creates snapshot in S3)
    let doc = OgreDoc::new();
    let state_bytes = doc.to_state_bytes();
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            state_bytes,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);

    // Simulate a WebSocket edit by appending an UPDATE# row directly.
    // Create a new doc, insert text, and compute the diff as the update.
    let edited_doc = OgreDoc::new();
    {
        use yrs::{Transact, WriteTxn, types::xml::{XmlFragment, XmlOut, XmlTextPrelim}};
        let mut txn = edited_doc.inner().transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        if let Some(XmlOut::Element(para)) = frag.get(&txn, 0) {
            para.insert(&mut txn, 0, XmlTextPrelim::new("pending edit text"));
        }
    }
    let sv = doc.state_vector();
    let diff = edited_doc.encode_diff(&sv).unwrap();

    let update = ogrenotes_storage::models::document::DocUpdate {
        doc_id: doc_id.clone(),
        clock: format!("{}_test", ogrenotes_common::time::now_usec()),
        update_bytes: diff,
        user_id: "test-user".to_string(),
        created_at: ogrenotes_common::time::now_usec(),
        client_version: None,
    };
    app.state.doc_repo.append_update(&update).await.unwrap();

    // Export should include the pending update
    let (status, body) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/html"),
            Some(&token),
            vec![],
            "text/html",
        )
        .await;
    assert_eq!(status, 200);
    let html = String::from_utf8(body).unwrap();
    assert!(
        html.contains("pending edit text"),
        "Export should include pending updates, got: {html}"
    );

    app.cleanup().await;
}

/// Regression for #38: a single `DocUpdate` whose `update_bytes`
/// exceed DDB's 400 KB item cap must persist via the S3-backed path
/// in `DocRepo::append_update` and round-trip through
/// `get_pending_updates` byte-identical. Before the fix, the DDB
/// PutItem returned `ValidationException: Item size has exceeded the
/// maximum allowed size` and the WS handler silently logged-and-dropped
/// the edit, so a large paste appeared in the live UI but vanished on
/// reload.
#[tokio::test]
async fn test_append_update_oversize_routes_to_s3_and_round_trips() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Big Paste", None).await;

    // 300 KiB payload — strictly larger than `UPDATE_INLINE_MAX_BYTES`
    // (256 KiB) and also strictly larger than DDB's 400 KB item cap
    // when combined with row overhead, so the inline-Blob path would
    // have failed against real DynamoDB.
    let big_bytes: Vec<u8> = (0..(300 * 1024)).map(|i| (i % 251) as u8).collect();
    let clock = format!("{}_oversize", ogrenotes_common::time::now_usec());
    let update = ogrenotes_storage::models::document::DocUpdate {
        doc_id: doc_id.clone(),
        clock: clock.clone(),
        update_bytes: big_bytes.clone(),
        user_id: "alice".to_string(),
        created_at: ogrenotes_common::time::now_usec(),
        client_version: None,
    };

    app.state
        .doc_repo
        .append_update(&update)
        .await
        .expect("append_update must succeed for oversize payload");

    let pending = app
        .state
        .doc_repo
        .get_pending_updates(&doc_id, usize::MAX)
        .await
        .expect("get_pending_updates");

    let row = pending
        .iter()
        .find(|u| u.clock == clock)
        .expect("oversize update should be visible to get_pending_updates");
    assert_eq!(
        row.update_bytes, big_bytes,
        "S3-backed update bytes must round-trip byte-identical",
    );

    // Prune sweep should remove both the DDB row and the S3 blob —
    // a future `get_pending_updates` returns nothing for this clock.
    let cutoff = ogrenotes_common::time::now_usec() + 1;
    app.state
        .doc_repo
        .delete_updates_before(&doc_id, cutoff)
        .await
        .expect("delete_updates_before");
    let pending_after = app
        .state
        .doc_repo
        .get_pending_updates(&doc_id, usize::MAX)
        .await
        .expect("get_pending_updates after prune");
    assert!(
        pending_after.iter().all(|u| u.clock != clock),
        "oversize update row should be gone after delete_updates_before",
    );

    app.cleanup().await;
}

/// Gap #3 of the test-coverage plan. GET /content must correctly
/// reassemble a document whose UPDATE# tail exceeds DynamoDB's
/// per-page Query response cap (1 MB). Pre-`108d4fc` the query
/// silently truncated at 1 MB and the response was a doc built
/// from only the first ~16 rows; this test would have failed by
/// returning a state that's missing later edits.
///
/// Seeds 100 real yrs updates totaling >2 MB by inserting ~22 KiB
/// of text per iteration into a transient ground-truth `OgreDoc`,
/// then appending each diff as an UPDATE# row via `DocRepo`. The
/// sentinel is placed at iteration 60 so a pagination regression
/// that returns only the first page would lose it. The test
/// asserts the GET /content response decodes cleanly and the
/// sentinel survives.
///
/// Also serves as the harness for the future #91 defensive-cap
/// acceptance test — once that cap exists, this test's 2 MB
/// total will need to either drop below the cap or move to a
/// dedicated "must be rejected with 503" test.
#[tokio::test]
async fn test_get_content_survives_many_inline_updates_2mb() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Big Tail", None).await;

    // Ground-truth doc mirrors what the server initialized on
    // `create_doc` (one empty paragraph) so the diffs we generate
    // apply cleanly on top of the snapshot.
    let mut truth = OgreDoc::new();

    // Append `N_ROWS` UPDATE# rows. Each row's update_bytes is the
    // real yrs diff from the previous state vector — so the chain
    // is replayable in order and merges to a single coherent doc.
    // Each iteration inserts a ~22 KiB chunk so the cumulative
    // update_bytes total exceeds 2 MiB (well past DDB's per-page
    // 1 MB Query response cap that 108d4fc paginates around).
    const N_ROWS: usize = 100;
    const CHUNK_BYTES: usize = 22 * 1024;
    const SENTINEL: &str = "SENTINEL_TOKEN_AT_ITER_60";
    const SENTINEL_AT: usize = 60;

    let mut total_update_bytes: usize = 0;

    for i in 0..N_ROWS {
        let baseline_sv = truth.state_vector();

        // Insert text into the paragraph created by OgreDoc::new().
        // Position 0 inside the paragraph element is fine — the
        // operation is what matters, not the visual order.
        {
            use yrs::{Transact, WriteTxn, types::xml::{XmlFragment, XmlOut}};
            let doc = truth.inner();
            let mut txn = doc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");
            let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) else {
                panic!("expected initial paragraph element");
            };
            let chunk = if i == SENTINEL_AT {
                // Pad to CHUNK_BYTES so this row's update is the
                // same size class as its neighbors; the sentinel
                // itself only needs to be present somewhere.
                let mut s = SENTINEL.to_string();
                s.push_str(&"x".repeat(CHUNK_BYTES.saturating_sub(SENTINEL.len())));
                s
            } else {
                format!("row{i:03}_") + &"x".repeat(CHUNK_BYTES.saturating_sub(7))
            };
            para.insert(
                &mut txn,
                0,
                yrs::types::xml::XmlTextPrelim::new(chunk.as_str()),
            );
        }

        let diff = truth.encode_diff(&baseline_sv)
            .expect("encode_diff should succeed for a forward step");

        total_update_bytes += diff.len();

        let clock = format!("{}_{:08}", ogrenotes_common::time::now_usec(), i);
        let update = ogrenotes_storage::models::document::DocUpdate {
            doc_id: doc_id.clone(),
            clock,
            update_bytes: diff,
            user_id: "alice".to_string(),
            created_at: ogrenotes_common::time::now_usec(),
            client_version: None,
        };

        app.state
            .doc_repo
            .append_update(&update)
            .await
            .expect("append_update");
    }

    assert!(
        total_update_bytes > 2 * 1024 * 1024,
        "test seed produced only {total_update_bytes} bytes — expected >2 MiB to exercise the pagination path",
    );

    let pending = app
        .state
        .doc_repo
        .get_pending_updates(&doc_id, usize::MAX)
        .await
        .expect("get_pending_updates");
    assert_eq!(
        pending.len(),
        N_ROWS,
        "all {N_ROWS} seeded updates must be visible to get_pending_updates",
    );

    // Now exercise the production GET /content route end-to-end. If
    // pagination regresses, the response will be built from only the
    // first ~16 rows and the sentinel (at iter 60) will be missing.
    let (status, body) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            vec![],
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200, "GET /content must succeed for the seeded doc");

    let restored = OgreDoc::from_state_bytes(&body)
        .expect("response body must decode as yrs state bytes");

    // The encoded state contains every Insert op's text inline. If
    // the iteration-60 row was retrieved via the paginated Query,
    // the sentinel substring appears in the response bytes.
    let restored_state = restored.to_state_bytes();
    assert!(
        contains_sentinel(&restored_state, SENTINEL.as_bytes()),
        "decoded doc must contain the iter-{SENTINEL_AT} sentinel — pagination likely regressed",
    );

    app.cleanup().await;
}

/// #91: Closes the regression window the 49-MiB outage opened.
/// `get_pending_updates` now caps the accumulated `update_bytes`;
/// crossing it surfaces as `RepoError::TooLarge` →
/// `503 ServiceUnavailable` instead of OOM-ing the task.
///
/// This test directly drives the repo at a tiny cap so we don't
/// have to seed 32+ MiB of rows. The semantic verified is the same:
/// "summed payload above the cap → bail with TooLarge".
#[tokio::test]
async fn test_get_pending_updates_caps_total_bytes_with_too_large() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice-cap@test.com").await;
    let doc_id = app.create_doc(&token, "Cap Test", None).await;

    // Seed two updates of ~16 KiB each. Total ~32 KiB; cap at
    // 20 KiB. The first row pushes the running total to ~16 KiB
    // (under cap, accepted); the second crosses ~32 KiB and bails.
    let mut truth = ogrenotes_collab::document::OgreDoc::new();
    for i in 0..2u32 {
        let baseline_sv = truth.state_vector();
        {
            use yrs::{Transact, WriteTxn, types::xml::{XmlFragment, XmlOut}};
            let doc = truth.inner();
            let mut txn = doc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");
            let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) else {
                panic!("expected initial paragraph element");
            };
            let chunk = format!("r{i:02}_") + &"x".repeat(16 * 1024 - 4);
            para.insert(
                &mut txn,
                0,
                yrs::types::xml::XmlTextPrelim::new(chunk.as_str()),
            );
        }
        let diff = truth.encode_diff(&baseline_sv)
            .expect("encode_diff should succeed");
        let now = ogrenotes_common::time::now_usec();
        let update = ogrenotes_storage::models::document::DocUpdate {
            doc_id: doc_id.clone(),
            clock: format!("{now:020}-r{i:02}"),
            update_bytes: diff,
            user_id: "alice-cap".to_string(),
            created_at: now,
            client_version: Some("test".to_string()),
        };
        app.state
            .doc_repo
            .append_update(&update)
            .await
            .expect("append_update");
    }

    // 20 KiB cap is smaller than the cumulative payload (~32 KiB)
    // but larger than the first row alone (~16 KiB). Repo must bail
    // on the second row with TooLarge.
    const TIGHT_CAP: usize = 20 * 1024;
    let result = app
        .state
        .doc_repo
        .get_pending_updates(&doc_id, TIGHT_CAP)
        .await;

    match result {
        Err(ogrenotes_storage::repo::RepoError::TooLarge { what, actual, cap }) => {
            assert!(
                what.contains(&doc_id),
                "TooLarge.what should mention doc id; got {what:?}"
            );
            assert!(
                actual > cap,
                "TooLarge.actual ({actual}) must exceed cap ({cap})"
            );
            assert_eq!(cap, TIGHT_CAP);
        }
        Err(other) => panic!("expected TooLarge, got {other:?}"),
        Ok(rows) => panic!(
            "expected TooLarge, got Ok with {} rows (cap was {TIGHT_CAP})",
            rows.len()
        ),
    }

    // The same call with the generous (production-default) cap
    // succeeds — confirms the bail is purely cap-driven, not a
    // bug in pagination at the seeded size.
    let pending = app
        .state
        .doc_repo
        .get_pending_updates(&doc_id, usize::MAX)
        .await
        .expect("with usize::MAX cap, get_pending_updates must succeed");
    assert_eq!(pending.len(), 2, "all 2 seeded rows must be visible");

    app.cleanup().await;
}

/// Substring search over a byte slice. Avoids pulling in a regex
/// dep for a one-off test assertion.
fn contains_sentinel(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

// ─── Activity Feed ────────────────────────────────────────────

/// Activity feed should record events from document operations.
#[tokio::test]
async fn test_activity_feed_records_open() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Activity Test", Some(&folder_id)).await;

    // Share with Bob
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(Method::POST, &format!("/api/v1/folders/{folder_id}/members"), Some(&token_a), Some(body)).await;

    // Bob opens the document (triggers open activity)
    app.json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None).await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Check activity feed
    let (status, json) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}/activity"), Some(&token_a), None)
        .await;
    assert_eq!(status, 200);
    let activities = json["activities"].as_array().unwrap();
    assert!(!activities.is_empty(), "Activity feed should have at least one event");
    assert!(
        activities.iter().any(|a| a["eventType"] == "open"),
        "Should have an 'open' event, got: {activities:?}"
    );

    app.cleanup().await;
}

/// Activity feed should record share events.
#[tokio::test]
async fn test_activity_feed_records_share() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let doc_id = app.create_doc(&token_a, "Share Activity", None).await;

    // Share document with Bob
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(Method::POST, &format!("/api/v1/documents/{doc_id}/members"), Some(&token_a), Some(body)).await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let (status, json) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}/activity"), Some(&token_a), None)
        .await;
    assert_eq!(status, 200);
    let activities = json["activities"].as_array().unwrap();
    assert!(
        activities.iter().any(|a| a["eventType"] == "share"),
        "Should have a 'share' event, got: {activities:?}"
    );

    app.cleanup().await;
}

/// Activity feed requires document access.
#[tokio::test]
async fn test_activity_feed_requires_access() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;

    let doc_id = app.create_doc(&token_a, "Private", None).await;

    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}/activity"), Some(&token_b), None)
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

// ─── Trash / Restore / Purge ───────────────────────────────────

async fn get_trash_folder_id(app: &common::TestApp, token: &str) -> String {
    let (status, json) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(token), None)
        .await;
    assert_eq!(status, 200);
    json["trashFolderId"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_delete_places_doc_in_trash_and_hides_from_home() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doomed", None).await;
    let home_id = get_home_folder_id(&app, &token).await;
    let trash_id = get_trash_folder_id(&app, &token).await;

    let (status, _) = app
        .json_request(Method::DELETE, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 204);

    // Home folder no longer lists the doc.
    let (_, home) = app
        .json_request(Method::GET, &format!("/api/v1/folders/{home_id}"), Some(&token), None)
        .await;
    let home_child_ids: Vec<&str> = home["children"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["childId"].as_str())
        .collect();
    assert!(!home_child_ids.contains(&doc_id.as_str()), "trashed doc should not appear in Home");

    // Trash folder lists the doc with is_deleted=true and is_trash=true.
    let (_, trash) = app
        .json_request(Method::GET, &format!("/api/v1/folders/{trash_id}"), Some(&token), None)
        .await;
    assert_eq!(trash["isTrash"], true);
    let trash_children = trash["children"].as_array().unwrap();
    let row = trash_children.iter().find(|c| c["childId"] == doc_id).expect("trashed doc not in trash listing");
    assert_eq!(row["isDeleted"], true);
    assert_eq!(row["title"], "Doomed");

    app.cleanup().await;
}

#[tokio::test]
async fn test_trashed_doc_read_only_for_owner() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Soon Trashed", None).await;

    // Trash it.
    let (status, _) = app
        .json_request(Method::DELETE, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 204);

    // Owner can still GET metadata — is_deleted=true flag surfaces.
    let (status, json) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["isDeleted"], true);

    // Owner can read content.
    let (status, _) = app
        .bytes_request(Method::GET, &format!("/api/v1/documents/{doc_id}/content"), Some(&token), vec![], "")
        .await;
    assert_eq!(status, 200);

    // Owner cannot PATCH / PUT content / delete again cleanly — write paths 404.
    let patch_body = serde_json::json!({ "title": "Renamed" });
    let (status, _) = app
        .json_request(Method::PATCH, &format!("/api/v1/documents/{doc_id}"), Some(&token), Some(patch_body))
        .await;
    assert_eq!(status, 404);

    let doc = OgreDoc::new();
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            doc.to_state_bytes(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_trashed_doc_invisible_to_non_owner() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_user_a, token_a) = app.create_user("alice@test.com").await;
    let (user_b_id, token_b) = app.create_user("bob@test.com").await;

    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Shared Doc", Some(&folder_id)).await;
    let share_body = serde_json::json!({ "userId": user_b_id, "accessLevel": "EDIT" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&token_a),
            Some(share_body),
        )
        .await;
    assert_eq!(status, 204);

    // Alice trashes.
    let (status, _) = app
        .json_request(Method::DELETE, &format!("/api/v1/documents/{doc_id}"), Some(&token_a), None)
        .await;
    assert_eq!(status, 204);

    // Bob — editor via folder — now sees 404; trashed docs never leak.
    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_restore_document_happy_path() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let home_id = get_home_folder_id(&app, &token).await;
    let doc_id = app.create_doc(&token, "Roundtrip", None).await;

    // Trash.
    let (status, _) = app
        .json_request(Method::DELETE, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 204);

    // Restore into Home.
    let body = serde_json::json!({ "targetFolderId": home_id });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/restore"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 204);

    // Doc is live again and in Home.
    let (status, json) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["isDeleted"], false);
    assert_eq!(json["folderId"], home_id);

    app.cleanup().await;
}

#[tokio::test]
async fn test_restore_document_not_in_trash_rejects() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let home_id = get_home_folder_id(&app, &token).await;
    let doc_id = app.create_doc(&token, "Live", None).await;

    let body = serde_json::json!({ "targetFolderId": home_id });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/restore"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_restore_target_must_be_accessible() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;
    let bob_folder = app.create_folder(&token_b, "Bob's folder", None).await;

    let doc_id = app.create_doc(&token_a, "Alice Doc", None).await;
    let (status, _) = app
        .json_request(Method::DELETE, &format!("/api/v1/documents/{doc_id}"), Some(&token_a), None)
        .await;
    assert_eq!(status, 204);

    // Alice tries to restore into Bob's folder — not allowed.
    let body = serde_json::json!({ "targetFolderId": bob_folder });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/restore"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_restore_into_trash_folder_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let trash_id = get_trash_folder_id(&app, &token).await;
    let doc_id = app.create_doc(&token, "Round-robin", None).await;

    let (status, _) = app
        .json_request(Method::DELETE, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 204);

    // Restoring into Trash itself is nonsensical and the server must refuse.
    let body = serde_json::json!({ "targetFolderId": trash_id });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/restore"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_purge_document_happy_path() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Dust", None).await;

    // Trash first.
    let (status, _) = app
        .json_request(Method::DELETE, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 204);

    // Purge.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/purge"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Doc should now be gone entirely.
    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 404);

    // And removed from the trash listing.
    let trash_id = get_trash_folder_id(&app, &token).await;
    let (_, trash) = app
        .json_request(Method::GET, &format!("/api/v1/folders/{trash_id}"), Some(&token), None)
        .await;
    let trash_children = trash["children"].as_array().unwrap();
    assert!(
        !trash_children.iter().any(|c| c["childId"] == doc_id),
        "purged doc should not linger in trash listing"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_purge_requires_doc_be_in_trash() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Live", None).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/purge"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_purge_cross_user_denied() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("alice@test.com").await;
    let token_b = app.create_user_token("bob@test.com").await;
    let doc_id = app.create_doc(&token_a, "Alice's", None).await;
    let (status, _) = app
        .json_request(Method::DELETE, &format!("/api/v1/documents/{doc_id}"), Some(&token_a), None)
        .await;
    assert_eq!(status, 204);

    // Bob cannot purge Alice's trashed doc — he doesn't see it at all.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/purge"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

// ─── Trashed doc: remaining write endpoints reject ─────────────
//
// The general contract: every endpoint that mutates document state must
// use the strict `check_doc_access` (not the allow-deleted variant) so a
// trashed doc cannot be touched. These pin that for the endpoints not
// already covered above.

/// Build a minimal multipart/form-data body carrying a single file field.
fn multipart_body(boundary: &str, filename: &str, content_type: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

#[tokio::test]
async fn test_import_on_trashed_doc_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let boundary = "doctortestboundary";
    let body = multipart_body(boundary, "data.csv", "text/csv", b"a,b\n1,2\n");
    let (status, _) = app
        .bytes_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/import"),
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_link_settings_patch_on_trashed_doc_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let body = serde_json::json!({ "linkSharingMode": "view" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

// ─── Import endpoint ───────────────────────────────────────────

#[tokio::test]
async fn test_import_csv_happy_path() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Sheet", None).await;

    let boundary = "importtestboundary";
    let csv = b"name,score\nalice,10\nbob,20\n";
    let body = multipart_body(boundary, "data.csv", "text/csv", csv);
    let (status, _) = app
        .bytes_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/import"),
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 204);

    // Content read-back should include the imported rows — at least the
    // cell text should be embedded in the Y.Doc bytes somewhere.
    let (status, bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            vec![],
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200);
    // Cell values appear verbatim in the CRDT state blob (they're plain
    // YText entries). This is a weak signal but catches total import no-op.
    let blob = String::from_utf8_lossy(&bytes);
    assert!(blob.contains("alice"), "imported cell 'alice' missing");
    assert!(blob.contains("bob"), "imported cell 'bob' missing");

    app.cleanup().await;
}

#[tokio::test]
async fn test_import_unsupported_extension_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let boundary = "importtestboundary";
    let body = multipart_body(boundary, "data.txt", "text/plain", b"not a spreadsheet");
    let (status, _) = app
        .bytes_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/import"),
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_import_non_editor_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;

    // Share folder with Bob as VIEW only.
    let body = serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/folders/{folder_id}/members"),
        Some(&token_a),
        Some(body),
    )
    .await;

    let boundary = "importtestboundary";
    let body = multipart_body(boundary, "data.csv", "text/csv", b"a\n1\n");
    let (status, _) = app
        .bytes_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/import"),
            Some(&token_b),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_import_invalid_csv_utf8_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    // Bytes that are not valid UTF-8 — the CSV branch rejects them with
    // a BadRequest before attempting to parse.
    let boundary = "importtestboundary";
    let bad = vec![0xFF, 0xFE, 0xFD, 0xFC];
    let body = multipart_body(boundary, "data.csv", "text/csv", &bad);
    let (status, _) = app
        .bytes_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/import"),
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

/// Oversize body exceeds the DefaultBodyLimit layer and is cut off at the
/// transport layer before the handler sees it.
#[tokio::test]
async fn test_import_oversize_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    // 11 MiB of CSV body — DefaultBodyLimit is 10 MiB.
    let huge: Vec<u8> = std::iter::repeat(b"abcdefgh,")
        .take(11 * 1024 * 1024 / 9 + 1)
        .flatten()
        .copied()
        .collect();
    let boundary = "importtestboundary";
    let body = multipart_body(boundary, "data.csv", "text/csv", &huge);
    let (status, _) = app
        .bytes_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/import"),
            Some(&token),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .await;
    assert!(
        status == 413 || status == 400,
        "oversize should be 413 or 400, got {status}"
    );

    app.cleanup().await;
}

/// The WebSocket upgrade path requires Edit via strict check_doc_access.
/// Trashed docs must not hand out a WS token — otherwise a live session
/// could keep pushing CRDT updates into a doc the user just trashed.
#[tokio::test]
async fn test_ws_token_on_trashed_doc_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/ws-token"),
            Some(&token),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

/// `PUT /content` uses an optimistic-locked snapshot write: it reads the
/// current `snapshot_version`, then conditionally bumps it. When two writers
/// race off the same version, exactly one wins (204) and the other's
/// conditional bump fails — surfacing as HTTP 409. This exercises the
/// `SnapshotWrite::VersionConflict → ApiError::Conflict` branch, the
/// autosave concurrency guard, which had no coverage.
///
/// Multi-threaded runtime so the writers genuinely run in parallel and read
/// the same starting version before either's conditional write commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_put_content_concurrent_writers_conflict_409() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("conflict@test.com").await;
    let doc_id = app.create_doc(&token, "Conflict Doc", None).await;
    let state = OgreDoc::new().to_state_bytes();

    // Fire several concurrent PUTs at the same (version 0) snapshot.
    let n = 4;
    let mut handles = Vec::new();
    for _ in 0..n {
        let router = app.router.clone();
        let path = format!("/api/v1/documents/{doc_id}/content");
        let token = token.clone();
        let body = state.clone();
        handles.push(tokio::spawn(async move {
            use tower::ServiceExt;
            let req = hyper::Request::builder()
                .method(Method::PUT)
                .uri(&path)
                .header("Content-Type", "application/octet-stream")
                .header("Authorization", format!("Bearer {token}"))
                .header("X-Forwarded-For", "conflict-test-ip")
                .body(axum::body::Body::from(body))
                .unwrap();
            router.oneshot(req).await.unwrap().status().as_u16()
        }));
    }
    let mut statuses = Vec::new();
    for h in handles {
        statuses.push(h.await.unwrap());
    }

    assert!(
        statuses.iter().all(|s| *s == 204 || *s == 409),
        "writers should only 204 or 409, got: {statuses:?}"
    );
    assert!(
        statuses.contains(&204),
        "at least one concurrent writer must win (204): {statuses:?}"
    );
    assert!(
        statuses.contains(&409),
        "at least one concurrent writer must lose the optimistic lock (409): {statuses:?}"
    );

    app.cleanup().await;
}
