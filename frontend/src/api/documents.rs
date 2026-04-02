use serde::{Deserialize, Serialize};

use super::client::{api_delete, api_get, api_get_bytes, api_patch, api_post, api_put_bytes, ApiClientError};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentResponse {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub folder_id: Option<String>,
    pub doc_type: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateDocumentRequest {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder_id: Option<String>,
}

pub async fn create_document(
    title: &str,
    folder_id: Option<&str>,
) -> Result<DocumentResponse, ApiClientError> {
    let body = CreateDocumentRequest {
        title: title.to_string(),
        folder_id: folder_id.map(|s| s.to_string()),
    };
    api_post("/documents", &body).await
}

pub async fn get_document(id: &str) -> Result<DocumentResponse, ApiClientError> {
    api_get(&format!("/documents/{id}")).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDocumentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

pub async fn update_document_title(id: &str, title: &str) -> Result<(), ApiClientError> {
    let body = UpdateDocumentRequest {
        title: Some(title.to_string()),
    };
    api_patch(&format!("/documents/{id}"), &body).await
}

pub async fn delete_document(id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/documents/{id}")).await
}

pub async fn get_content(id: &str) -> Result<Vec<u8>, ApiClientError> {
    api_get_bytes(&format!("/documents/{id}/content")).await
}

pub async fn put_content(id: &str, data: &[u8]) -> Result<(), ApiClientError> {
    api_put_bytes(&format!("/documents/{id}/content"), data).await
}

pub async fn export_document(id: &str, format: &str) -> Result<String, ApiClientError> {
    api_get(&format!("/documents/{id}/export/{format}")).await
}

#[derive(Deserialize)]
pub struct WsTokenResponse {
    pub token: String,
}

/// Request a single-use WebSocket authentication token for a document.
pub async fn request_ws_token(id: &str) -> Result<WsTokenResponse, ApiClientError> {
    api_post(&format!("/documents/{id}/ws-token"), &serde_json::json!({})).await
}
