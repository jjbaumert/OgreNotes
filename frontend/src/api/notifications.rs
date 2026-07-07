// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use serde::{Deserialize, Serialize};
use super::client::{api_get, api_post, api_post_empty, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationItem {
    pub notif_id: String,
    pub notif_type: String,
    pub doc_id: Option<String>,
    pub thread_id: Option<String>,
    pub actor_id: String,
    /// Resolved actor display name (defaults to actor_id on older servers).
    #[serde(default)]
    pub actor_name: String,
    /// Resolved (truncated) document title for context. None when absent.
    #[serde(default)]
    pub doc_title: Option<String>,
    pub message: String,
    /// Truncated comment/reply preview for comment notifications.
    #[serde(default)]
    pub preview: Option<String>,
    /// Anchor block/cell id for deep-linking to the comment.
    #[serde(default)]
    pub block_id: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DismissAllResponse {
    pub dismissed: usize,
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

/// Dismiss (delete) all notifications from the bell.
pub async fn dismiss_all() -> Result<DismissAllResponse, ApiClientError> {
    api_post("/notifications/dismiss-all", &serde_json::json!({})).await
}

/// Dismiss (delete) specific notifications by their DynamoDB sort keys.
pub async fn dismiss(sks: Vec<String>) -> Result<(), ApiClientError> {
    api_post_empty(
        "/notifications/dismiss",
        &serde_json::json!({ "notificationSks": sks }),
    )
    .await
}

/// Mark specific notifications read by their DynamoDB sort keys.
pub async fn mark_read(sks: Vec<String>) -> Result<(), ApiClientError> {
    api_post_empty(
        "/notifications/read",
        &serde_json::json!({ "notificationSks": sks }),
    )
    .await
}

/// Reconstruct a notification's sort key from the fields the list
/// endpoint returns. Mirrors `Notification::sk` on the backend
/// (`NOTIF#<20-digit-zero-padded-created_at>#<notif_id>`), so the client
/// can mark a single notification read without a dedicated id field.
pub fn notification_sk(created_at: i64, notif_id: &str) -> String {
    format!("NOTIF#{created_at:020}#{notif_id}")
}
