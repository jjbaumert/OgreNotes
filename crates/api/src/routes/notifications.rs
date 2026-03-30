//! Notification endpoints.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_notifications))
        .route("/read", post(mark_read))
        .route("/read-all", post(mark_all_read))
        .route("/unread-count", get(unread_count))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationResponse {
    notif_id: String,
    notif_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    doc_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
    actor_id: String,
    message: String,
    read: bool,
    created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationListResponse {
    notifications: Vec<NotificationResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarkReadRequest {
    /// Notification IDs to mark as read. Each is `NOTIF#<timestamp>#<id>` (the SK).
    notification_sks: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnreadCountResponse {
    count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MarkAllReadResponse {
    marked: usize,
}

/// GET /notifications — list the user's notifications (newest first, max 50).
async fn list_notifications(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<axum::Json<NotificationListResponse>, ApiError> {
    let notifications = state.notification_repo.list(&user_id, 50).await?;

    let response = NotificationListResponse {
        notifications: notifications
            .into_iter()
            .map(|n| {
                let type_str = serde_json::to_string(&n.notif_type)
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string();
                NotificationResponse {
                    notif_id: n.notif_id,
                    notif_type: type_str,
                    doc_id: n.doc_id,
                    thread_id: n.thread_id,
                    actor_id: n.actor_id,
                    message: n.message,
                    read: n.read,
                    created_at: n.created_at,
                }
            })
            .collect(),
    };

    Ok(axum::Json(response))
}

/// POST /notifications/read — mark specific notifications as read.
async fn mark_read(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    axum::Json(body): axum::Json<MarkReadRequest>,
) -> Result<StatusCode, ApiError> {
    if body.notification_sks.len() > 50 {
        return Err(ApiError::BadRequest("Too many notification IDs (max 50)".to_string()));
    }

    for sk in &body.notification_sks {
        // Ignore not-found errors for individual notifications
        let _ = state.notification_repo.mark_read(&user_id, sk).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// POST /notifications/read-all — mark all notifications as read.
async fn mark_all_read(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<axum::Json<MarkAllReadResponse>, ApiError> {
    let count = state.notification_repo.mark_all_read(&user_id).await?;
    Ok(axum::Json(MarkAllReadResponse { marked: count }))
}

/// GET /notifications/unread-count — get count of unread notifications.
async fn unread_count(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<axum::Json<UnreadCountResponse>, ApiError> {
    let count = state.notification_repo.unread_count(&user_id).await?;
    Ok(axum::Json(UnreadCountResponse { count }))
}
