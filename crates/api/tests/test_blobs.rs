// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

#[tokio::test]
async fn test_request_upload_url() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("blob@test.com").await;
    let doc_id = app.create_doc(&token, "Blob Doc", None).await;

    let body = serde_json::json!({
        "filename": "test.png",
        "contentType": "image/png",
    });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 200);
    assert!(json["uploadUrl"].is_string());
    assert!(json["blobId"].is_string());
    assert!(json["key"].is_string());

    app.cleanup().await;
}

#[tokio::test]
async fn test_upload_invalid_content_type() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("badct@test.com").await;
    let doc_id = app.create_doc(&token, "Bad CT Doc", None).await;

    let body = serde_json::json!({
        "filename": "evil.js",
        "contentType": "application/javascript",
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_upload_empty_filename() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("emptyname@test.com").await;
    let doc_id = app.create_doc(&token, "Empty Name Doc", None).await;

    let body = serde_json::json!({
        "filename": "",
        "contentType": "image/png",
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_request_download_url() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("download@test.com").await;
    let doc_id = app.create_doc(&token, "Download Doc", None).await;

    // First request an upload URL to get blobId and key
    let body = serde_json::json!({
        "filename": "photo.png",
        "contentType": "image/png",
    });
    let (status, upload_json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 200);

    let blob_id = upload_json["blobId"].as_str().unwrap();
    let key = upload_json["key"].as_str().unwrap();

    // Now request a download URL using the blobId and key
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!(
                "/api/v1/documents/{doc_id}/blobs/{blob_id}?key={key}"
            ),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert!(json["downloadUrl"].is_string());

    app.cleanup().await;
}

#[tokio::test]
async fn test_blob_unauthenticated() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Need a real doc_id, so create one with a temp user
    let token = app.create_user_token("tempblob@test.com").await;
    let doc_id = app.create_doc(&token, "Unauth Blob Doc", None).await;

    let body = serde_json::json!({
        "filename": "test.png",
        "contentType": "image/png",
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            None,
            Some(body),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

/// `request_download_url` rejects a `key` that doesn't belong to the
/// `{doc_id}/{blob_id}` path it's requested under. This prefix check is a
/// path-traversal / cross-doc S3-access guard — without it a caller with
/// access to one doc could presign a download URL for an arbitrary object
/// by passing a foreign `key`. Regression coverage for the guard.
#[tokio::test]
async fn test_download_url_rejects_foreign_key_prefix() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("blob-traversal@test.com").await;
    let doc_id = app.create_doc(&token, "Guarded Doc", None).await;

    // Obtain a legitimate blob id for this doc.
    let (status, upload_json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&token),
            Some(serde_json::json!({ "filename": "photo.png", "contentType": "image/png" })),
        )
        .await;
    assert_eq!(status, 200);
    let blob_id = upload_json["blobId"].as_str().unwrap();

    // Pass a key that points at a *different* document's blob namespace.
    // The caller owns this doc, but the key doesn't match the requested
    // {doc_id}/{blob_id} prefix → 400.
    let foreign_key = "blobs/some-other-doc/some-other-blob/secret.png";
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/blobs/{blob_id}?key={foreign_key}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 400, "foreign key prefix must be rejected: {json}");

    app.cleanup().await;
}
