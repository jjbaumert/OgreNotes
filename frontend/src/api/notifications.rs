use serde::{Deserialize, Serialize};
use super::client::{api_get, api_post, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationItem {
    pub notif_id: String,
    pub notif_type: String,
    pub doc_id: Option<String>,
    pub thread_id: Option<String>,
    pub actor_id: String,
    pub message: String,
    pub read: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationListResponse {
    pub notifications: Vec<NotificationItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreadCountResponse {
    pub count: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkAllReadResponse {
    pub marked: usize,
}

pub async fn get_notifications() -> Result<NotificationListResponse, ApiClientError> {
    api_get("/notifications").await
}

pub async fn get_unread_count() -> Result<UnreadCountResponse, ApiClientError> {
    api_get("/notifications/unread-count").await
}

pub async fn mark_all_read() -> Result<MarkAllReadResponse, ApiClientError> {
    api_post("/notifications/read-all", &serde_json::json!({})).await
}
