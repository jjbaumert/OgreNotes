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

/// Regression: uploading a blob is a WRITE and must require Edit access.
/// A viewer (folder shared at VIEW) must be refused — previously the
/// upload path went through the View-level `get_verified_doc`, letting a
/// view-only sharer stage content into the doc's blob namespace.
#[tokio::test]
async fn test_upload_url_denied_for_view_only_sharer() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, owner) = app.create_user("blob-owner@test.com").await;
    let (viewer_id, viewer) = app.create_user("blob-viewer@test.com").await;

    let folder_id = app.create_folder(&owner, "Shared", None).await;
    let doc_id = app.create_doc(&owner, "Doc", Some(&folder_id)).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&owner),
            Some(serde_json::json!({ "userId": viewer_id, "accessLevel": "VIEW" })),
        )
        .await;
    assert_eq!(status, 204, "grant VIEW should succeed");

    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&viewer),
            Some(serde_json::json!({ "filename": "x.png", "contentType": "image/png" })),
        )
        .await;
    assert_eq!(status, 403, "view-only sharer must not get an upload URL: {json}");

    app.cleanup().await;
}

/// Companion to the test above: an EDIT sharer CAN upload — proves the
/// access check discriminates by level rather than blanket-denying
/// non-owners.
#[tokio::test]
async fn test_upload_url_allowed_for_edit_sharer() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, owner) = app.create_user("blob-owner2@test.com").await;
    let (editor_id, editor) = app.create_user("blob-editor@test.com").await;

    let folder_id = app.create_folder(&owner, "Shared", None).await;
    let doc_id = app.create_doc(&owner, "Doc", Some(&folder_id)).await;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(&owner),
            Some(serde_json::json!({ "userId": editor_id, "accessLevel": "EDIT" })),
        )
        .await;
    assert_eq!(status, 204);

    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/blobs"),
            Some(&editor),
            Some(serde_json::json!({ "filename": "x.png", "contentType": "image/png" })),
        )
        .await;
    assert_eq!(status, 200, "edit sharer must get an upload URL: {json}");

    app.cleanup().await;
}
