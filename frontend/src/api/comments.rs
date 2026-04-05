use serde::{Deserialize, Serialize};
use super::client::{api_delete, api_get, api_patch, api_post, api_post_empty, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItem {
    pub thread_id: String,
    pub doc_id: String,
    pub thread_type: String,
    pub status: String,
    pub created_by: String,
    #[serde(default)]
    pub created_by_name: String,
    #[serde(default)]
    pub block_id: Option<String>,
    pub anchor_start: Option<u32>,
    pub anchor_end: Option<u32>,
    #[serde(default)]
    pub first_message: Option<String>,
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
    #[serde(default)]
    pub user_name: String,
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
    anchor_start: Option<u32>,
    anchor_end: Option<u32>,
) -> Result<CreateThreadResponse, ApiClientError> {
    let mut body = if let Some(bid) = block_id {
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
    if let (Some(start), Some(end)) = (anchor_start, anchor_end) {
        body["anchorStart"] = serde_json::json!(start);
        body["anchorEnd"] = serde_json::json!(end);
    }
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

pub async fn update_thread_status(
    thread_id: &str,
    status: &str,
) -> Result<(), ApiClientError> {
    api_patch(
        &format!("/threads/{thread_id}"),
        &serde_json::json!({ "status": status }),
    )
    .await
}

pub async fn delete_thread(thread_id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/threads/{thread_id}")).await
}
