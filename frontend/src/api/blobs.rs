// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

use super::client::{api_post, http_error, ApiClientError};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadRequest {
    filename: String,
    content_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResponse {
    pub upload_url: String,
    pub blob_id: String,
    pub key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadResponse {
    download_url: String,
}

/// Request a presigned upload URL for a blob.
pub async fn request_upload_url(
    doc_id: &str,
    filename: &str,
    content_type: &str,
) -> Result<UploadResponse, ApiClientError> {
    let body = UploadRequest {
        filename: filename.to_string(),
        content_type: content_type.to_string(),
    };
    api_post(&format!("/documents/{doc_id}/blobs"), &body).await
}

/// Upload raw bytes to a presigned S3 URL (no auth header).
pub async fn upload_to_s3(
    presigned_url: &str,
    data: &[u8],
    content_type: &str,
) -> Result<(), ApiClientError> {
    // #5: the URL is whatever the backend returned; refuse to PUT the
    // raw bytes anywhere but an https endpoint, so a compromised or
    // misconfigured backend can't redirect the upload to http:// or an
    // attacker-controlled host.
    //
    // Dev-stack carve-out: when the app itself is served over plain
    // http (local compose stack / CI playwright stack, where MinIO
    // presigns http://127.0.0.1:9000 URLs), an https-only requirement
    // on the upload leg protects nothing — the page, its script, and
    // every API call already ride unencrypted http. Production serves
    // over https, so the strict branch is the one real deployments
    // take. This is what makes image upload exercisable at all in the
    // local/CI doctor scenarios (deck-blocks).
    let app_is_http = web_sys::window()
        .map(|w| w.location().protocol().unwrap_or_default() == "http:")
        .unwrap_or(false);
    if !presigned_url.starts_with("https://") && !(app_is_http && presigned_url.starts_with("http://")) {
        return Err(ApiClientError::Network(
            "refusing non-https upload URL".to_string(),
        ));
    }
    let resp = Request::put(presigned_url)
        .header("Content-Type", content_type)
        .body(data.to_vec())
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    if !resp.ok() {
        return Err(http_error(&resp));
    }
    Ok(())
}

/// Request a presigned download URL for a blob.
pub async fn request_download_url(
    doc_id: &str,
    blob_id: &str,
    key: &str,
) -> Result<String, ApiClientError> {
    let encoded_key = js_sys::encode_uri_component(key);
    let resp: DownloadResponse = super::client::api_get(
        &format!("/documents/{doc_id}/blobs/{blob_id}?key={encoded_key}"),
    )
    .await?;
    Ok(resp.download_url)
}
