use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

use super::client::{api_post, ApiClientError};

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
    let resp = Request::put(presigned_url)
        .header("Content-Type", content_type)
        .body(data.to_vec())
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    if !resp.ok() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiClientError::Http(status, text));
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
