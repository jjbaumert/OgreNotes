// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use serde::Deserialize;

use super::client::{api_get, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultItem {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub doc_type: String,
    /// Unix timestamp in microseconds (consistent with all other API types).
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub results: Vec<SearchResultItem>,
    pub total_estimate: usize,
}

/// Search documents by keyword.
pub async fn search(query: &str, count: Option<usize>) -> Result<SearchResponse, ApiClientError> {
    let encoded = urlencoding::encode(query);
    let count_param = count.map(|c| format!("&count={c}")).unwrap_or_default();
    api_get(&format!("/search?q={encoded}{count_param}")).await
}
