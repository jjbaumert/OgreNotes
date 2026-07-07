// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::{ReadTxn, Text, Transact, WriteTxn};

/// Build Y.Doc state bytes with a single paragraph containing `text`.
/// Copied from `test_search.rs` style so this module stays self-contained.
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

#[tokio::test]
async fn test_list_versions_new_doc() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("history@test.com").await;
    let doc_id = app.create_doc(&token, "History Doc", None).await;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/versions"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let versions = json["versions"].as_array().unwrap();
    assert!(!versions.is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_version_content() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("versioncontent@test.com").await;
    let doc_id = app.create_doc(&token, "Version Content Doc", None).await;

    let (status, bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/versions/1"),
            Some(&token),
            vec![],
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200);
    assert!(!bytes.is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_version_not_found() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("notfound@test.com").await;
    let doc_id = app.create_doc(&token, "No Such Version", None).await;

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/versions/999999"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_history_unauthenticated() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("temphistory@test.com").await;
    let doc_id = app.create_doc(&token, "Unauth History Doc", None).await;

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/versions"),
            None,
            None,
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

/// Restoring a version on a trashed doc must 404 — writes must not cross
/// the trash boundary.
#[tokio::test]
async fn test_restore_version_on_trashed_doc_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    // Soft-delete the doc.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    // Attempt to restore version 1 — handler uses strict check, 404s.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/versions/1/restore"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

/// Regression: `GET /documents/:id/versions/:v1/diff/:v2` stamps every
/// DiffEntry with the newer version's author + timestamp so the frontend
/// can render "who edited this" alongside the content diff.
#[tokio::test]
async fn test_diff_versions_carries_attribution() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, token) = app
        .create_user_with_name("differ@test.com", "Diff User")
        .await;
    let doc_id = app.create_doc(&token, "Attribution Doc", None).await;

    // Persist a SNAPSHOT# row for v1 pointing at the initial snapshot S3
    // key. Without this, `resolve_snapshot_s3_key` has no record for v1
    // once v2 becomes live and the diff handler 404s.
    let v1_snapshot = ogrenotes_storage::models::snapshot::DocSnapshot {
        doc_id: doc_id.clone(),
        version: 1,
        s3_key: format!("docs/{doc_id}/snapshots/1.bin"),
        size_bytes: 0,
        user_id: user_id.clone(),
        created_at: 1_700_000_000_000_000,
    };
    app.state
        .snapshot_repo
        .create(&v1_snapshot)
        .await
        .expect("seed v1 snapshot row");

    // Push a new content version — this advances live to v2 with a
    // different paragraph, so the diff is non-empty.
    let bytes = make_doc_bytes("Second version body");
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            bytes,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204, "PUT content should succeed");

    // Read the post-PUT doc meta so we can pin the expected timestamp
    // exactly rather than asserting the weaker "is_i64" shape. Without
    // this, the handler could silently return `timestamp: None` on every
    // entry and the previous version of this test would still pass.
    let expected_meta_updated_at = app
        .state
        .doc_repo
        .get(&doc_id)
        .await
        .expect("repo get")
        .expect("doc exists")
        .updated_at;

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/versions/1/diff/2"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);

    let diffs = json["diffs"].as_array().expect("diffs array");
    assert!(!diffs.is_empty(), "expected at least one diff entry");
    for entry in diffs {
        // v2 is the live version, so `timestamp` must equal the doc's
        // `updated_at` exactly — not just "present and numeric".
        assert_eq!(
            entry["timestamp"].as_i64(),
            Some(expected_meta_updated_at),
            "timestamp should equal meta.updated_at, got {entry}"
        );
        // `userId` must be absent (serde skip) or explicitly null —
        // `DocumentMeta` doesn't track last editor, so the handler
        // deliberately reports unknown authorship for the live version.
        assert!(
            entry.get("userId").is_none() || entry["userId"].is_null(),
            "userId should be null/absent for the live-version path, got {entry}",
        );
    }

    app.cleanup().await;
}

/// Restoring an older version actually reverts the live content and writes a
/// new version. `restore_version` is a destructive content-overwrite path
/// that had only the trashed-doc rejection covered — the success path was
/// untested. We compare live encodings (GET /content before vs after) to
/// avoid snapshot-vs-live byte-encoding differences.
#[tokio::test]
async fn test_restore_version_reverts_live_content() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("restore@test.com").await;
    let doc_id = app.create_doc(&token, "Restore Doc", None).await;

    // Version one content.
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            make_doc_bytes("Version one body"),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);
    let v1 = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().snapshot_version;
    let (_, content_v1) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            vec![],
            "application/octet-stream",
        )
        .await;

    // Version two content — now live.
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            make_doc_bytes("Version two body"),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);
    let (_, content_v2) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            vec![],
            "application/octet-stream",
        )
        .await;
    assert_ne!(content_v1, content_v2, "the two versions must differ");

    // Restore v1.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/versions/{v1}/restore"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204, "restore should succeed");

    // Live content now matches the v1 encoding, and a new version was written.
    let (_, restored) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            vec![],
            "application/octet-stream",
        )
        .await;
    assert_eq!(restored, content_v1, "restored content must match version one");
    let new_version = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().snapshot_version;
    assert!(new_version > v1, "restore writes a new version, not an in-place rewind");

    app.cleanup().await;
}

/// `restore_version` requires Edit. A collaborator with only View access
/// cannot restore — the destructive overwrite is gated. (The trashed-doc
/// path is covered separately; this pins the permission gate.)
#[tokio::test]
async fn test_restore_version_view_collaborator_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_alice_id, alice_token) = app.create_user("restore-alice@test.com").await;
    let (bob_id, bob_token) = app.create_user("restore-bob@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Guarded Restore Doc", None).await;

    // Build a version to target.
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&alice_token),
            make_doc_bytes("Body"),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);
    let v1 = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().snapshot_version;

    // Share with Bob at VIEW only.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" })),
        )
        .await;
    assert_eq!(status, 204);

    // Bob (View) cannot restore — needs Edit.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/versions/{v1}/restore"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(status, 403, "a View-only collaborator cannot restore a version");

    app.cleanup().await;
}
