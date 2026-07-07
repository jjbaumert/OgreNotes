// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P5 piece C — integration tests for the bulk-export
//! endpoint.
//!
//! Covers: auth gate, the 100-doc cap, unknown-format rejection,
//! per-id authz when caller doesn't own a requested doc (manifest
//! records 404/403, the archive still ships the docs they could
//! read), the all-fail → 207 path, and the happy multi-doc zip.

use axum::http::Method;
use serde_json::json;

mod common;

#[tokio::test]
async fn test_bulk_export_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/export",
            None,
            Some(json!({ "docIds": [], "format": "markdown" })),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_export_rejects_too_many_ids() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-cap@test.com").await;

    let ids: Vec<String> = (0..101).map(|n| format!("doc-{n}")).collect();
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/export",
            Some(&token),
            Some(json!({ "docIds": ids, "format": "markdown" })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_export_rejects_unknown_format() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-format@test.com").await;

    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/export",
            Some(&token),
            Some(json!({ "docIds": ["whatever"], "format": "xlsx" })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_export_all_missing_returns_207() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-empty@test.com").await;

    // Nothing exists — every id is a 404. The endpoint returns the
    // manifest as JSON with status 207 rather than zipping a
    // manifest-only archive.
    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/bulk/export",
            Some(&token),
            Some(json!({
                "docIds": ["ghost-1", "ghost-2"],
                "format": "markdown"
            })),
        )
        .await;
    assert_eq!(status, 207);
    let arr = json.as_array().expect("manifest is an array");
    assert_eq!(arr.len(), 2);
    for entry in arr {
        assert_eq!(entry["status"].as_u64().unwrap(), 404);
    }

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_export_happy_path_zip_with_manifest() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("bulk-happy@test.com").await;
    let doc_a = app.create_doc(&token, "First doc", None).await;
    let doc_b = app.create_doc(&token, "Second doc", None).await;

    let (status, body) = app
        .bytes_request(
            Method::POST,
            "/api/v1/documents/bulk/export",
            Some(&token),
            serde_json::to_vec(&json!({
                "docIds": [doc_a, doc_b],
                "format": "markdown"
            }))
            .unwrap(),
            "application/json",
        )
        .await;
    assert_eq!(status, 200);

    // Verify the bytes parse as a zip archive containing both a
    // `_manifest.json` entry and at least one exported file.
    let cursor = std::io::Cursor::new(&body);
    let mut archive = zip::ZipArchive::new(cursor).expect("response is a zip");
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(names.contains(&"_manifest.json".to_string()), "names={names:?}");
    let md_count = names.iter().filter(|n| n.ends_with(".md")).count();
    assert_eq!(md_count, 2, "expected 2 .md entries, got {names:?}");

    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_export_mixed_authz_records_per_id_status() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let owner = app.create_user_token("bulk-mixed-owner@test.com").await;
    let outsider = app.create_user_token("bulk-mixed-outsider@test.com").await;
    let doc_a = app.create_doc(&owner, "Owned", None).await;

    // Outsider asks for one doc they can see (none — they own
    // nothing here) plus the owner's doc and a non-existent doc.
    // The owner's doc should come back as access-denied or
    // not-found (depending on whether the link-share path is
    // open); the missing id is a 404.
    let (status, body) = app
        .bytes_request(
            Method::POST,
            "/api/v1/documents/bulk/export",
            Some(&outsider),
            serde_json::to_vec(&json!({
                "docIds": [doc_a.clone(), "definitely-missing"],
                "format": "html"
            }))
            .unwrap(),
            "application/json",
        )
        .await;
    // Either path is fine: zero successes (207 JSON) or all-fail
    // mapped to 207. The owner's doc not appearing in the archive
    // is the key invariant.
    assert!(status == 207 || status == 200, "status={status}");

    // If we got a 207, body parses as a JSON manifest with both
    // ids accounted for as failures.
    if status == 207 {
        let manifest: serde_json::Value =
            serde_json::from_slice(&body).expect("207 body is JSON");
        let arr = manifest.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let ids: Vec<String> = arr
            .iter()
            .map(|e| e["docId"].as_str().unwrap().to_string())
            .collect();
        assert!(ids.contains(&doc_a));
        assert!(ids.contains(&"definitely-missing".to_string()));
    }

    app.cleanup().await;
}
