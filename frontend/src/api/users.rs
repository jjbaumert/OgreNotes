use serde::Deserialize;
use super::client::{api_get, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub user_id: String,
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub users: Vec<SearchResult>,
}

pub async fn search_by_email(email: &str) -> Result<SearchResponse, ApiClientError> {
    let encoded = js_sys::encode_uri_component(email);
    api_get(&format!("/users/search?email={encoded}")).await
}

pub async fn search_users(query: &str) -> Result<SearchResponse, ApiClientError> {
    let encoded = js_sys::encode_uri_component(query);
    api_get(&format!("/users/search?q={encoded}")).await
}
