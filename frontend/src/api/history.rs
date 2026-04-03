use serde::Deserialize;
use super::client::{api_get, api_get_bytes, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub version: u64,
    pub size_bytes: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionListResponse {
    pub versions: Vec<VersionEntry>,
}

pub async fn list_versions(doc_id: &str) -> Result<VersionListResponse, ApiClientError> {
    api_get(&format!("/documents/{doc_id}/versions")).await
}

pub async fn get_version_content(doc_id: &str, version: u64) -> Result<Vec<u8>, ApiClientError> {
    api_get_bytes(&format!("/documents/{doc_id}/versions/{version}")).await
}
