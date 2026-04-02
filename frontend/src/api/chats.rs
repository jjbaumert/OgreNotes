use serde::{Deserialize, Serialize};
use super::client::{api_get, api_post_empty, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatItem {
    pub id: String,
    pub chat_type: String,
    pub title: Option<String>,
    pub member_ids: Vec<String>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatListResponse {
    pub chats: Vec<ChatItem>,
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

pub async fn list_chats() -> Result<ChatListResponse, ApiClientError> {
    api_get("/chats").await
}

pub async fn get_chat(id: &str) -> Result<ChatItem, ApiClientError> {
    api_get(&format!("/chats/{id}")).await
}

pub async fn list_messages(chat_id: &str) -> Result<MessageListResponse, ApiClientError> {
    api_get(&format!("/chats/{chat_id}/messages")).await
}

pub async fn send_message(chat_id: &str, content: &str) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/chats/{chat_id}/messages"),
        &serde_json::json!({ "content": content }),
    )
    .await
}
