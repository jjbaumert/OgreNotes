use serde::{Deserialize, Serialize};
use super::client::{api_get, api_post, api_post_empty, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItem {
    pub thread_id: String,
    pub doc_id: String,
    pub thread_type: String,
    pub status: String,
    pub created_by: String,
    #[serde(default)]
    pub block_id: Option<String>,
    pub anchor_start: Option<u32>,
    pub anchor_end: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListResponse {
    pub threads: Vec<ThreadItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageItem {
    pub message_id: String,
    pub user_id: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageListResponse {
    pub messages: Vec<MessageItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateThreadResponse {
    pub thread_id: String,
}

pub async fn list_threads(doc_id: &str) -> Result<ThreadListResponse, ApiClientError> {
    api_get(&format!("/documents/{doc_id}/threads")).await
}

pub async fn create_thread(
    doc_id: &str,
    message: &str,
    block_id: Option<&str>,
) -> Result<CreateThreadResponse, ApiClientError> {
    let body = if let Some(bid) = block_id {
        serde_json::json!({
            "threadType": "inline",
            "blockId": bid,
            "message": message
        })
    } else {
        serde_json::json!({
            "threadType": "document",
            "message": message
        })
    };
    api_post(&format!("/documents/{doc_id}/threads"), &body).await
}

pub async fn list_messages(thread_id: &str) -> Result<MessageListResponse, ApiClientError> {
    api_get(&format!("/threads/{thread_id}/messages")).await
}

pub async fn add_message(
    thread_id: &str,
    content: &str,
) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/threads/{thread_id}/messages"),
        &serde_json::json!({ "content": content }),
    )
    .await
}
