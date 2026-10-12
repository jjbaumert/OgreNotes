// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P5 piece C — integration tests for the bulk-export
//! endpoint.
//!
//! Covers: auth gate, the 100-doc cap, unknown-format rejection,
//! per-id authz when caller doesn't own a requested doc (manifest
//! records 404/403, the archive still ships the docs they could
//! read), the all-fail → 207 path, and the happy multi-doc zip.

use axum::http::Method;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read};
use yrs::{ReadTxn, StateVector, Transact};

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
    assert!(
        names.contains(&"_manifest.json".to_string()),
        "names={names:?}"
    );
    let md_count = names.iter().filter(|n| n.ends_with(".md")).count();
    assert_eq!(md_count, 2, "expected 2 .md entries, got {names:?}");

    app.cleanup().await;
}

async fn write_marker(app: &common::TestApp, token: &str, id: &str, marker: &str) {
    let doc = ogrenotes_collab::import::from_markdown(marker);
    let bytes = doc
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{id}/content"),
            Some(token),
            bytes,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);
}

async fn bulk(app: &common::TestApp, token: &str, ids: &[String], format: &str) -> (u16, Vec<u8>) {
    app.bytes_request(
        Method::POST,
        "/api/v1/documents/bulk/export",
        Some(token),
        serde_json::to_vec(&json!({"docIds":ids,"format":format})).unwrap(),
        "application/json",
    )
    .await
}

fn archive_contents(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("response is a ZIP");
    let mut files = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let name = file.name().to_owned();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert!(
            files.insert(name.clone(), text).is_none(),
            "duplicate ZIP entry: {name}"
        );
    }
    files
}

#[tokio::test]
async fn test_bulk_export_mixed_authz_records_per_id_status() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let owner = app.create_user_token("bulk-mixed-owner@test.com").await;
    let reader = app.create_user_token("bulk-mixed-reader@test.com").await;
    let readable = app.create_doc(&reader, "Readable", None).await;
    let denied = app.create_doc(&owner, "Private", None).await;
    write_marker(&app, &reader, &readable, "ReadableBodyMarker316").await;
    write_marker(&app, &owner, &denied, "UnauthorizedBodyMarker316").await;
    let ids = vec![
        readable.clone(),
        denied.clone(),
        "definitely-missing".into(),
    ];
    for format in ["html", "markdown"] {
        let (status, bytes) = bulk(&app, &reader, &ids, format).await;
        assert_eq!(status, 200);
        let files = archive_contents(&bytes);
        assert_eq!(
            files.len(),
            2,
            "only the readable document and manifest may be exported"
        );
        assert!(
            files
                .values()
                .all(|text| !text.contains("UnauthorizedBodyMarker316"))
        );
        let manifest: Vec<Value> = serde_json::from_str(&files["_manifest.json"]).unwrap();
        assert_eq!(manifest.len(), 3);
        for (entry, id) in manifest.iter().zip(&ids) {
            assert_eq!(entry["docId"], *id);
        }
        assert_eq!(manifest[0]["status"], 200);
        let filename = manifest[0]["filename"].as_str().unwrap();
        assert!(files[filename].contains("ReadableBodyMarker316"));
        assert!(manifest[0].get("error").is_none());
        assert_eq!(
            manifest[1],
            json!({"docId":denied,"status":403,"error":"access denied"})
        );
        assert_eq!(
            manifest[2],
            json!({"docId":"definitely-missing","status":404,"error":"not found"})
        );
    }
    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_export_zero_readable_has_exact_failure_only_manifest() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let owner = app.create_user_token("bulk-private-owner@test.com").await;
    let outsider = app
        .create_user_token("bulk-private-outsider@test.com")
        .await;
    let denied = app.create_doc(&owner, "Private", None).await;
    write_marker(&app, &owner, &denied, "UnauthorizedZeroBodyMarker316").await;
    for format in ["html", "markdown"] {
        let (status, bytes) =
            bulk(&app, &outsider, &[denied.clone(), "missing".into()], format).await;
        assert_eq!(
            status, 207,
            "no readable documents must return JSON, never a ZIP"
        );
        let manifest: Value = serde_json::from_slice(&bytes).expect("failure-only body is JSON");
        assert_eq!(
            manifest,
            json!([
                {"docId":denied,"status":403,"error":"access denied"},
                {"docId":"missing","status":404,"error":"not found"}
            ])
        );
        assert!(!String::from_utf8_lossy(&bytes).contains("UnauthorizedZeroBodyMarker316"));
    }
    app.cleanup().await;
}

#[tokio::test]
async fn test_bulk_export_allocates_unique_names_for_generated_and_sanitized_collisions() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    for (case, titles) in [
        vec!["Notes", "Notes", "Notes-1"],
        vec!["Notes-1", "Notes", "Notes"],
        vec!["A B", "A_B", "A_B-1"],
        vec!["Notes", "notes", "NOTES-1", "Notes"],
        vec!["!!!", "untitled", "untitled-1"],
    ]
    .into_iter()
    .enumerate()
    {
        let token = app
            .create_user_token(&format!("bulk-collisions-{case}@test.com"))
            .await;
        let mut ids = Vec::new();
        for (index, title) in titles.iter().enumerate() {
            let id = app.create_doc(&token, title, None).await;
            write_marker(&app, &token, &id, &format!("DistinctBodyMarker305_{index}")).await;
            ids.push(id);
        }
        for format in ["markdown", "md", "html"] {
            let (status, bytes) = bulk(&app, &token, &ids, format).await;
            assert_eq!(
                status,
                200,
                "bulk export failed for titles {titles:?}: {}",
                String::from_utf8_lossy(&bytes)
            );
            let files = archive_contents(&bytes);
            assert_eq!(files.len(), ids.len() + 1);
            let manifest: Vec<Value> = serde_json::from_str(&files["_manifest.json"]).unwrap();
            assert_eq!(manifest.len(), ids.len());
            let mut allocated = HashSet::new();
            for (index, (entry, id)) in manifest.iter().zip(&ids).enumerate() {
                assert_eq!(entry["docId"], *id);
                assert_eq!(entry["status"], 200);
                let name = entry["filename"].as_str().unwrap();
                assert!(
                    allocated.insert(name.to_ascii_lowercase()),
                    "case-insensitive filename collision: {name}"
                );
                assert!(files[name].contains(&format!("DistinctBodyMarker305_{index}")));
            }
        }
    }
    app.cleanup().await;
}
