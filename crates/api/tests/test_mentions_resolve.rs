// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for POST /api/v1/mentions/resolve — batch mention
//! resolution for the block-links/mentions feature (Task 2).
//!
//! The critical security property under test in
//! `resolve_no_access_is_byte_identical_to_nonexistent`: a target the
//! caller can't access must serialize byte-identically to a nonexistent
//! one. This endpoint deliberately diverges from the document endpoints'
//! 403-vs-404 policy.

mod common;

use axum::http::Method;
use serde_json::json;

#[tokio::test]
async fn resolve_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/mentions/resolve",
            None,
            Some(json!({ "targets": [{ "docId": "whatever" }] })),
        )
        .await;
    assert_eq!(status, 401);
    app.cleanup().await;
}

#[tokio::test]
async fn resolve_doc_only_returns_title() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("alice-mr1@test.com").await;
    let doc_id = app.create_doc(&token, "Resolve Me", None).await;
    let (status, body) = app
        .json_request(
            Method::POST,
            "/api/v1/mentions/resolve",
            Some(&token),
            Some(json!({ "targets": [{ "docId": doc_id }] })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["results"][0]["status"], "ok");
    assert_eq!(body["results"][0]["title"], "Resolve Me");
    assert_eq!(body["results"][0]["blockFound"], false);
    assert!(body["results"][0].get("snippet").is_none());
    app.cleanup().await;
}

#[tokio::test]
async fn resolve_block_returns_snippet_and_dangling_is_flagged() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("alice-mr2@test.com").await;
    let doc_id = app.create_doc(&token, "Has Blocks", None).await;

    let doc = build_doc_with_one_block("blk-target", "Snippet source text");
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            doc.to_state_bytes(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);

    let (status, body) = app
        .json_request(
            Method::POST,
            "/api/v1/mentions/resolve",
            Some(&token),
            Some(json!({ "targets": [
                { "docId": doc_id, "blockId": "blk-target" },
                { "docId": doc_id, "blockId": "blk-does-not-exist" }
            ] })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["results"][0]["status"], "ok");
    assert_eq!(body["results"][0]["blockFound"], true);
    assert_eq!(body["results"][0]["snippet"], "Snippet source text");
    assert_eq!(body["results"][1]["status"], "ok"); // doc resolves…
    assert_eq!(body["results"][1]["blockFound"], false); // …block dangles
    assert!(body["results"][1].get("snippet").is_none());
    app.cleanup().await;
}

#[tokio::test]
async fn resolve_no_access_is_byte_identical_to_nonexistent() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let owner = app.create_user_token("owner-mr3@test.com").await;
    let stranger = app.create_user_token("stranger-mr3@test.com").await;
    let private_doc = app.create_doc(&owner, "Secret", None).await;

    let (s1, forbidden_body) = app
        .json_request(
            Method::POST,
            "/api/v1/mentions/resolve",
            Some(&stranger),
            Some(json!({ "targets": [{ "docId": private_doc }] })),
        )
        .await;
    let (s2, missing_body) = app
        .json_request(
            Method::POST,
            "/api/v1/mentions/resolve",
            Some(&stranger),
            Some(json!({ "targets": [{ "docId": "doc-does-not-exist" }] })),
        )
        .await;
    assert_eq!(s1, 200);
    assert_eq!(s2, 200);
    assert_eq!(forbidden_body["results"][0]["status"], "notFound");
    // The indistinguishability contract (spec §4): the two per-target
    // results must serialize identically — no title, no extra fields.
    assert_eq!(forbidden_body["results"][0], missing_body["results"][0]);
    app.cleanup().await;
}

#[tokio::test]
async fn resolve_batch_caps_targets() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("alice-mr4@test.com").await;
    let targets: Vec<_> = (0..101)
        .map(|i| json!({ "docId": format!("d{i}") }))
        .collect();
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/mentions/resolve",
            Some(&token),
            Some(json!({ "targets": targets })),
        )
        .await;
    assert_eq!(status, 400);
    app.cleanup().await;
}

/// Build a doc with a single paragraph block carrying `block_id` and
/// `text`, per the `doc_with_blocks` idiom in
/// `crates/collab/src/diff.rs` (module `block_plain_text_tests`).
fn build_doc_with_one_block(block_id: &str, text: &str) -> ogrenotes_collab::document::OgreDoc {
    use ogrenotes_collab::document::OgreDoc;
    use yrs::types::xml::{Xml, XmlElementPrelim, XmlFragment, XmlTextPrelim};
    use yrs::{Transact, WriteTxn};

    let doc = OgreDoc::new();
    {
        let mut txn = doc.inner().transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        let el = frag.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
        el.insert_attribute(&mut txn, "blockId", block_id);
        el.insert(&mut txn, 0, XmlTextPrelim::new(text));
    }
    doc
}
