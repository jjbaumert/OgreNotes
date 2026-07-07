// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::{ReadTxn, Text, Transact, WriteTxn};

/// Helper: build Y.Doc state bytes with a single paragraph containing `text`.
fn make_doc_bytes(text: &str) -> Vec<u8> {
    let doc = yrs::Doc::new();
    {
        let mut txn = doc.transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        let p = frag.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
        let t = p.insert(&mut txn, 0, XmlTextPrelim::new(""));
        t.push(&mut txn, text);
    }
    let txn = doc.transact();
    txn.encode_state_as_update_v1(&yrs::StateVector::default())
}

/// Helper: upload content to a document and wait for indexing.
async fn upload_content(app: &common::TestApp, token: &str, doc_id: &str, text: &str) {
    let bytes = make_doc_bytes(text);
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(token),
            bytes,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);
    // Allow fire-and-forget indexing to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}

#[tokio::test]
async fn test_search_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(Method::GET, "/api/v1/search?q=hello", None, None)
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_requires_query() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("searchq@test.com").await;

    let (status, _) = app
        .json_request(Method::GET, "/api/v1/search?q=", Some(&token), None)
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_finds_own_document() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("finder@test.com").await;
    let doc_id = app.create_doc(&token, "Searchable Report", None).await;

    upload_content(&app, &token, &doc_id, "unique searchable keyword xylophone").await;

    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/search?q=xylophone",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"].as_str().unwrap(), doc_id);

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_respects_permissions() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token_a = app.create_user_token("owner@test.com").await;
    let doc_id = app.create_doc(&token_a, "Private Notes", None).await;

    upload_content(&app, &token_a, &doc_id, "secret confidential zygote").await;

    // User B should NOT see user A's private document
    let token_b = app.create_user_token("outsider@test.com").await;
    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/search?q=zygote",
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let results = json["results"].as_array().unwrap();
    assert!(results.is_empty(), "User B should not see User A's document");

    // User A should see their own document
    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/search?q=zygote",
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_deleted_document_not_found() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("deleter@test.com").await;
    let doc_id = app.create_doc(&token, "To Be Deleted", None).await;

    upload_content(&app, &token, &doc_id, "ephemeral quokka content").await;

    // Verify it's searchable
    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/search?q=quokka",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["results"].as_array().unwrap().len(), 1);

    // Delete the document
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Should no longer appear in search
    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/search?q=quokka",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert!(json["results"].as_array().unwrap().is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_type_filter() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("typefilter@test.com").await;

    // Create a document (default type)
    let _doc_id = app.create_doc(&token, "Narwhal Document", None).await;

    // Create a spreadsheet
    let body = serde_json::json!({
        "title": "Narwhal Spreadsheet",
        "docType": "spreadsheet"
    });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/documents", Some(&token), Some(body))
        .await;
    assert_eq!(status, 201);
    let _sheet_id = json["id"].as_str().unwrap().to_string();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Search with type filter
    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/search?q=Narwhal&type=spreadsheet",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["docType"].as_str().unwrap(), "spreadsheet");

    app.cleanup().await;
}

// ─── Review fix #1: totalEstimate doesn't leak across permissions ──

#[tokio::test]
async fn test_total_estimate_does_not_leak_inaccessible_count() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // User A creates 3 documents with searchable content
    let token_a = app.create_user_token("leaker_a@test.com").await;
    for i in 0..3 {
        let doc_id = app
            .create_doc(&token_a, &format!("Platypus Doc {i}"), None)
            .await;
        upload_content(&app, &token_a, &doc_id, "platypus habitat research").await;
    }

    // User B searches — should see 0 results and totalEstimate 0
    let token_b = app.create_user_token("leaker_b@test.com").await;
    let (status, json) = app
        .json_request(
            Method::GET,
            "/api/v1/search?q=platypus",
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let results = json["results"].as_array().unwrap();
    assert!(results.is_empty());
    assert_eq!(
        json["totalEstimate"].as_u64().unwrap(),
        0,
        "totalEstimate should not reveal inaccessible document count"
    );

    app.cleanup().await;
}

// ─── Review fix #6: malformed query returns 400, not 500 ──────────

#[tokio::test]
async fn test_malformed_query_returns_400() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("badquery@test.com").await;

    // Unbalanced parenthesis — invalid Tantivy query syntax
    let (status, _) = app
        .json_request(
            Method::GET,
            "/api/v1/search?q=%29",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 400, "Malformed query should return 400, not 500");

    app.cleanup().await;
}

#[tokio::test]
async fn test_query_too_long_returns_400() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("longquery@test.com").await;

    let long_query = "a".repeat(201);
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/search?q={long_query}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 400, "Query over 200 chars should return 400");

    app.cleanup().await;
}
