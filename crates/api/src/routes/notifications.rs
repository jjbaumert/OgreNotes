// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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
        .route("/dismiss", post(dismiss))
        .route("/dismiss-all", post(dismiss_all))
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
    /// Resolved display name of the actor, so the client can render
    /// "Alice replied" instead of an opaque id (issue #50).
    actor_name: String,
    /// Resolved title of the related document (truncated), so the client
    /// can say which document a comment is on. None when the notification
    /// has no doc, or the doc title can't be resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    doc_title: Option<String>,
    message: String,
    /// Truncated comment/reply preview, when the notification is about a
    /// comment thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<String>,
    /// Anchor block/cell id for deep-linking to the exact comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DismissAllResponse {
    dismissed: usize,
}

/// Truncate a document title for notification display. Titles are
/// user-controlled and can be long; cap at 80 chars + ellipsis.
fn truncate_title(title: &str) -> String {
    const MAX: usize = 80;
    if title.chars().count() > MAX {
        let mut t: String = title.chars().take(MAX).collect();
        t.push('…');
        t
    } else {
        title.to_string()
    }
}

/// GET /notifications — list the user's notifications (newest first, max 50).
async fn list_notifications(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<axum::Json<NotificationListResponse>, ApiError> {
    let notifications = state.notification_repo.list(&user_id, 50).await?;

    // Actor names resolve through one BatchGetItem (#38) — previously a
    // sequential get_by_id per unique actor. Missing rows / a failed
    // batch fall back to the raw actor_id. Document titles keep the
    // per-page cache: they come from doc_repo, which has no batch path.
    let actor_ids: Vec<String> = notifications.iter().map(|n| n.actor_id.clone()).collect();
    let actor_users = state.user_repo.get_by_ids(&actor_ids).await.unwrap_or_default();
    let mut doc_titles: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    let mut responses = Vec::with_capacity(notifications.len());
    for n in notifications {
        let actor_name = actor_users
            .get(&n.actor_id)
            .map(|u| u.name.clone())
            .unwrap_or_else(|| n.actor_id.clone());

        // Best-effort document title (truncated). Cached per doc_id; None
        // when there's no doc or the lookup fails — the title is purely
        // contextual, so a miss must not drop the notification.
        let doc_title = match &n.doc_id {
            Some(doc_id) => {
                if let Some(cached) = doc_titles.get(doc_id) {
                    cached.clone()
                } else {
                    let title = match state.doc_repo.get(doc_id).await {
                        Ok(Some(meta)) => Some(truncate_title(&meta.title)),
                        _ => None,
                    };
                    doc_titles.insert(doc_id.clone(), title.clone());
                    title
                }
            }
            None => None,
        };

        let type_str = serde_json::to_string(&n.notif_type)
            .unwrap_or_default()
            .trim_matches('"')
            .to_string();
        responses.push(NotificationResponse {
            notif_id: n.notif_id,
            notif_type: type_str,
            doc_id: n.doc_id,
            thread_id: n.thread_id,
            actor_id: n.actor_id,
            actor_name,
            doc_title,
            message: n.message,
            preview: n.preview,
            block_id: n.block_id,
            read: n.read,
            created_at: n.created_at,
        });
    }

    Ok(axum::Json(NotificationListResponse { notifications: responses }))
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

/// POST /notifications/dismiss — dismiss (delete) specific notifications.
async fn dismiss(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    axum::Json(body): axum::Json<MarkReadRequest>,
) -> Result<StatusCode, ApiError> {
    if body.notification_sks.len() > 50 {
        return Err(ApiError::BadRequest("Too many notification IDs (max 50)".to_string()));
    }

    for sk in &body.notification_sks {
        // Best-effort: ignore a single row's not-found / delete error.
        let _ = state.notification_repo.delete_one(&user_id, sk).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// POST /notifications/dismiss-all — dismiss (delete) all notifications.
async fn dismiss_all(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<axum::Json<DismissAllResponse>, ApiError> {
    let count = state.notification_repo.delete_all(&user_id).await?;
    Ok(axum::Json(DismissAllResponse { dismissed: count }))
}

/// GET /notifications/unread-count — get count of unread notifications.
async fn unread_count(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<axum::Json<UnreadCountResponse>, ApiError> {
    let count = state.notification_repo.unread_count(&user_id).await?;
    Ok(axum::Json(UnreadCountResponse { count }))
}
