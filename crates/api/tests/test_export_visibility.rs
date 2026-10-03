// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use axum::http::Method;
use serde_json::json;
use std::io::{Cursor, Read};
use yrs::{ReadTxn, StateVector, Transact};

const BODY: &str = "Visible body marker";
const SECRET: &str = "HiddenConversationMarker304";
const AUTHOR: &str = "Private Comment Author";

fn exported_text(format: &str, bytes: &[u8]) -> String {
    match format {
        "docx" => {
            let doc = ogrenotes_collab::import_docx::from_docx(bytes).unwrap();
            ogrenotes_collab::export::to_markdown(&doc)
        }
        #[cfg(feature = "pdf")]
        "pdf" => {
            let doc = ogrenotes_collab::import_pdf::from_pdf(bytes).unwrap();
            ogrenotes_collab::export::to_markdown(&doc)
        }
        _ => String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

async fn assert_exports(app: &common::TestApp, token: &str, doc_id: &str, comments: bool) {
    let mut formats = vec!["html", "markdown", "md", "docx"];
    if cfg!(feature = "pdf") {
        formats.push("pdf");
    }
    for format in formats {
        let (status, bytes) = app
            .bytes_request(
                Method::GET,
                &format!("/api/v1/documents/{doc_id}/export/{format}"),
                Some(token),
                Vec::new(),
                "application/octet-stream",
            )
            .await;
        assert_eq!(status, 200, "{format}: {}", String::from_utf8_lossy(&bytes));
        let text = exported_text(format, &bytes);
        assert!(
            text.contains(BODY),
            "{format}: missing document body: {text}"
        );
        assert_eq!(
            text.contains(SECRET),
            comments,
            "{format}: comment visibility: {text}"
        );
        assert_eq!(
            text.contains(AUTHOR),
            comments,
            "{format}: author visibility: {text}"
        );
        if format == "docx" && !comments {
            // Inspect every archive part as well as rendered text: a comment
            // omitted by the importer must not survive in hidden OOXML data.
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            for index in 0..archive.len() {
                let mut part = Vec::new();
                archive
                    .by_index(index)
                    .unwrap()
                    .read_to_end(&mut part)
                    .unwrap();
                let text = String::from_utf8_lossy(&part);
                assert!(!text.contains(SECRET));
                assert!(!text.contains(AUTHOR));
            }
        }
    }
    for format in ["html", "markdown"] {
        let (status, bytes) = app
            .bytes_request(
                Method::POST,
                "/api/v1/documents/bulk/export",
                Some(token),
                serde_json::to_vec(&json!({"docIds":[doc_id], "format":format})).unwrap(),
                "application/json",
            )
            .await;
        assert_eq!(status, 200);
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let name = archive
            .file_names()
            .find(|n| n.ends_with(if format == "html" { ".html" } else { ".md" }))
            .unwrap()
            .to_owned();
        let mut text = String::new();
        archive
            .by_name(&name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.contains(BODY));
        assert_eq!(text.contains(SECRET), comments, "bulk {format}: {text}");
        assert_eq!(text.contains(AUTHOR), comments, "bulk {format}: {text}");
    }
}

#[tokio::test]
async fn exports_follow_conversation_permissions_for_link_and_durable_viewers() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (owner_id, owner) = app
        .create_user_with_name("export-owner@test.com", AUTHOR)
        .await;
    let (reader_id, reader) = app.create_user("export-link@test.com").await;
    let (_, stranger) = app.create_user("export-stranger@test.com").await;
    let ws = app
        .state
        .user_repo
        .get_by_id(&owner_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .unwrap();
    app.state
        .workspace_repo
        .add_member(&ogrenotes_storage::models::workspace::WorkspaceMember {
            workspace_id: ws.clone(),
            user_id: reader_id.clone(),
            role: ogrenotes_storage::models::WorkspaceRole::Member,
            joined_at: 0,
        })
        .await
        .unwrap();
    let (status, doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&owner),
            Some(json!({"title":"Visibility", "docType":"document", "workspaceId":ws})),
        )
        .await;
    assert_eq!(status, 201);
    let id = doc["id"].as_str().unwrap();
    let content = ogrenotes_collab::import::from_markdown(BODY);
    let bytes = content
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{id}/content"),
            Some(&owner),
            bytes,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);
    let (status, thread) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{id}/threads"),
            Some(&owner),
            Some(json!({"threadType":"document", "message":SECRET})),
        )
        .await;
    assert_eq!(status, 201, "{thread}");

    for (mode, show, allow, visible) in [
        ("view", false, false, false),
        ("view", true, false, true),
        ("view", false, true, true),
        ("edit", false, false, true),
    ] {
        let (status, _) = app
            .json_request(
                Method::PATCH,
                &format!("/api/v1/documents/{id}/link-settings"),
                Some(&owner),
                Some(json!({"linkSharingMode":mode,
                "viewOptions":{"showConversation":show,"allowComments":allow}})),
            )
            .await;
        assert_eq!(status, 204);
        let (status, _) = app
            .json_request(
                Method::GET,
                &format!("/api/v1/documents/{id}/threads"),
                Some(&reader),
                None,
            )
            .await;
        assert_eq!(status, if visible { 200 } else { 403 });
        assert_exports(&app, &reader, id, visible).await;
    }
    let (status, _) = app.json_request(Method::PATCH, &format!("/api/v1/documents/{id}/link-settings"),
        Some(&owner), Some(json!({"linkSharingMode":"view", "viewOptions":{"showConversation":false,"allowComments":false}}))).await;
    assert_eq!(status, 204);
    assert_exports(&app, &owner, id, true).await;

    // A mixed archive must resolve conversation access separately for each
    // document, even when the first document grants the exporter full access.
    let (status, own_doc) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&reader),
            Some(json!({"title":"Reader owned", "docType":"document"})),
        )
        .await;
    assert_eq!(status, 201);
    let own_id = own_doc["id"].as_str().unwrap();
    let visible_comment = "AllowedConversationMarker304";
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{own_id}/threads"),
            Some(&reader),
            Some(json!({"threadType":"document", "message":visible_comment})),
        )
        .await;
    assert_eq!(status, 201);
    for format in ["html", "markdown", "md"] {
        let (status, bytes) = app
            .bytes_request(
                Method::POST,
                "/api/v1/documents/bulk/export",
                Some(&reader),
                serde_json::to_vec(&json!({"docIds":[own_id,id], "format":format})).unwrap(),
                "application/json",
            )
            .await;
        assert_eq!(status, 200);
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 3, "two documents and their manifest");
        let mut combined = String::new();
        for index in 0..archive.len() {
            archive
                .by_index(index)
                .unwrap()
                .read_to_string(&mut combined)
                .unwrap();
        }
        assert!(combined.contains(BODY));
        assert!(combined.contains(visible_comment));
        assert!(
            !combined.contains(SECRET),
            "mixed bulk {format}: {combined}"
        );
        assert!(
            !combined.contains(AUTHOR),
            "mixed bulk {format}: {combined}"
        );
    }
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{id}/members"),
            Some(&owner),
            Some(json!({"userId":reader_id,"accessLevel":"VIEW"})),
        )
        .await;
    assert_eq!(status, 204);
    assert_exports(&app, &reader, id, true).await;
    let (status, bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{id}/export/html"),
            Some(&stranger),
            Vec::new(),
            "application/octet-stream",
        )
        .await;
    assert!(status == 403 || status == 404);
    assert!(!String::from_utf8_lossy(&bytes).contains(SECRET));
    app.cleanup().await;
}
