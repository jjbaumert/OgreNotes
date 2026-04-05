//! Comment thread and message endpoints.
//!
//! Threads are attached to documents. Each thread can be inline (anchored to text)
//! or document-level (conversation pane).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::notification::{NotifType, Notification};
use ogrenotes_storage::models::thread::{Message, Thread, ThreadStatus, ThreadType};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Build the comments router (nested under /documents).
/// Comments router: document-scoped thread listing + creation.
pub fn doc_router() -> Router<AppState> {
    Router::new()
        .route("/{doc_id}/threads", get(list_threads))
        .route("/{doc_id}/threads", post(create_thread))
}

/// Thread router: operations on specific threads (mounted at /api/v1/threads).
pub fn thread_router() -> Router<AppState> {
    Router::new()
        .route("/{thread_id}", patch(update_thread))
        .route("/{thread_id}", delete(delete_thread_handler))
        .route("/{thread_id}/messages", get(list_messages))
        .route("/{thread_id}/messages", post(add_message))
        .route("/{thread_id}/messages/{message_id}", delete(delete_message))
}

// ─── Types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateThreadRequest {
    thread_type: ThreadType,
    /// Block ID this comment is anchored to (for inline comments).
    #[serde(default)]
    block_id: Option<String>,
    #[serde(default)]
    anchor_start: Option<u32>,
    #[serde(default)]
    anchor_end: Option<u32>,
    /// Initial message content (optional).
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateThreadRequest {
    status: ThreadStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMessageRequest {
    content: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadResponse {
    thread_id: String,
    doc_id: String,
    thread_type: ThreadType,
    status: ThreadStatus,
    created_by: String,
    created_by_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor_start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor_end: Option<u32>,
    /// Preview of the first message in the thread (truncated to 120 chars).
    #[serde(skip_serializing_if = "Option::is_none")]
    first_message: Option<String>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageResponse {
    message_id: String,
    user_id: String,
    user_name: String,
    content: String,
    created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadListResponse {
    threads: Vec<ThreadResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageListResponse {
    messages: Vec<MessageResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateThreadResponse {
    thread_id: String,
}

// ─── Handlers ───────────────────────────────────────────────────

/// GET /documents/:doc_id/threads — list all comment threads for a document.
async fn list_threads(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(doc_id): Path<String>,
) -> Result<axum::Json<ThreadListResponse>, ApiError> {
    // Verify access to the document
    let _meta = super::documents::check_doc_access(
        &state,
        &doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;

    let threads = state.thread_repo.list_threads_for_doc(&doc_id).await?;

    // Look up user names for thread creators and first message previews.
    let mut user_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut responses = Vec::with_capacity(threads.len());
    for t in threads {
        let name = if let Some(cached) = user_names.get(&t.created_by) {
            cached.clone()
        } else {
            let name = match state.user_repo.get_by_id(&t.created_by).await {
                Ok(Some(user)) => user.name,
                _ => t.created_by.clone(), // fallback to user ID
            };
            user_names.insert(t.created_by.clone(), name.clone());
            name
        };

        // Fetch first message preview (best-effort; None on error).
        let first_message = state
            .thread_repo
            .get_first_message(&t.thread_id)
            .await
            .ok()
            .flatten()
            .map(|m| {
                if m.content.len() > 120 {
                    let mut preview = m.content[..120].to_string();
                    preview.push_str("...");
                    preview
                } else {
                    m.content
                }
            });

        responses.push(ThreadResponse {
            thread_id: t.thread_id,
            doc_id: t.doc_id,
            thread_type: t.thread_type,
            status: t.status,
            created_by: t.created_by,
            created_by_name: name,
            block_id: t.block_id,
            anchor_start: t.anchor_start,
            anchor_end: t.anchor_end,
            first_message,
            created_at: t.created_at,
            updated_at: t.updated_at,
        });
    }

    Ok(axum::Json(ThreadListResponse { threads: responses }))
}

/// POST /documents/:doc_id/threads — create a new comment thread.
async fn create_thread(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(doc_id): Path<String>,
    axum::Json(body): axum::Json<CreateThreadRequest>,
) -> Result<(StatusCode, axum::Json<CreateThreadResponse>), ApiError> {
    // Require at least Comment access to create threads
    let _meta = super::documents::check_doc_access(
        &state,
        &doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::Comment,
    )
    .await?;

    // Inline comments must have a valid block_id.
    if body.thread_type == ThreadType::Inline {
        match &body.block_id {
            None => {
                return Err(ApiError::BadRequest(
                    "Inline comments require a blockId".to_string(),
                ));
            }
            Some(bid) => {
                // Validate block_id format: alphanumeric only, 4-32 chars.
                if bid.len() < 4
                    || bid.len() > 32
                    || !bid.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    return Err(ApiError::BadRequest(
                        "blockId must be 4-32 alphanumeric characters".to_string(),
                    ));
                }
            }
        }
    }

    // Enforce thread uniqueness per block:
    // - Block comments (no anchor range): one per block
    // - Inline comments (with anchor range): multiple allowed per block,
    //   but not on the exact same text range
    // - Block and inline can coexist on the same block
    if let Some(ref bid) = body.block_id {
        // Best-effort duplicate check — if the GSI query fails, allow creation
        // rather than blocking all comments.
        let existing = match state.thread_repo.list_threads_for_doc(&doc_id).await {
            Ok(threads) => threads,
            Err(e) => {
                tracing::warn!("Failed to check existing threads (GSI may not exist): {e}");
                Vec::new()
            }
        };
        let is_inline = body.anchor_start.is_some() && body.anchor_end.is_some();

        let has_conflict = existing.iter().any(|t| {
            if t.block_id.as_deref() != Some(bid.as_str()) { return false; }
            if t.status != ThreadStatus::Open { return false; }

            if is_inline {
                // Inline: conflict only if exact same anchor range
                t.anchor_start == body.anchor_start && t.anchor_end == body.anchor_end
            } else {
                // Block: conflict if any block comment (no anchors) exists
                t.anchor_start.is_none() && t.anchor_end.is_none()
            }
        });

        if has_conflict {
            let msg = if is_inline {
                "This text range already has an open comment thread"
            } else {
                "This block already has an open block comment"
            };
            return Err(ApiError::Conflict(msg.to_string()));
        }
    }

    let now = now_usec();
    let thread_id = nanoid::nanoid!(16);

    let thread = Thread {
        thread_id: thread_id.clone(),
        doc_id: doc_id.clone(),
        thread_type: body.thread_type,
        status: ThreadStatus::Open,
        created_by: user_id.clone(),
        title: None,
        member_ids: Vec::new(),
        block_id: body.block_id,
        anchor_start: body.anchor_start,
        anchor_end: body.anchor_end,
        created_at: now,
        updated_at: now,
    };

    state.thread_repo.create_thread(&thread).await?;

    // Add initial message if provided
    if let Some(content) = body.message {
        if !content.trim().is_empty() {
            let msg = Message {
                thread_id: thread_id.clone(),
                message_id: nanoid::nanoid!(16),
                user_id: user_id.clone(),
                content,
                created_at: now,
                updated_at: None,
            };
            state.thread_repo.add_message(&msg).await?;
        }
    }

    // Notify document owner about the new comment thread.
    let notif_repo = state.notification_repo.clone();
    let notif_doc_id = doc_id.clone();
    let notif_actor = user_id.clone();
    let notif_thread_id = thread_id.clone();
    let doc_repo = state.doc_repo.clone();
    tokio::spawn(async move {
        if let Ok(Some(meta)) = doc_repo.get(&notif_doc_id).await {
            if meta.owner_id != notif_actor {
                let notif = Notification {
                    notif_id: nanoid::nanoid!(16),
                    user_id: meta.owner_id,
                    notif_type: NotifType::Commented,
                    doc_id: Some(notif_doc_id),
                    thread_id: Some(notif_thread_id),
                    actor_id: notif_actor,
                    message: "commented on your document".to_string(),
                    read: false,
                    created_at: now_usec(),
                };
                let _ = notif_repo.create(&notif).await;
            }
        }
    });

    Ok((
        StatusCode::CREATED,
        axum::Json(CreateThreadResponse { thread_id }),
    ))
}

/// PATCH /threads/:thread_id — update thread status (resolve/reopen).
async fn update_thread(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(thread_id): Path<String>,
    axum::Json(body): axum::Json<UpdateThreadRequest>,
) -> Result<StatusCode, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;

    // Verify access to the parent document
    let _meta = super::documents::check_doc_access(
        &state,
        &thread.doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::Comment,
    )
    .await?;

    state
        .thread_repo
        .update_status(&thread_id, body.status, now_usec())
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /threads/:thread_id/messages — list messages in a thread.
async fn list_messages(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(thread_id): Path<String>,
) -> Result<axum::Json<MessageListResponse>, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;

    // Verify access to the parent document
    let _meta = super::documents::check_doc_access(
        &state,
        &thread.doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;

    let messages = state.thread_repo.list_messages(&thread_id).await?;
    let mut user_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut msg_responses = Vec::with_capacity(messages.len());
    for m in messages {
        let name = if let Some(cached) = user_names.get(&m.user_id) {
            cached.clone()
        } else {
            let name = match state.user_repo.get_by_id(&m.user_id).await {
                Ok(Some(user)) => user.name,
                _ => m.user_id.clone(),
            };
            user_names.insert(m.user_id.clone(), name.clone());
            name
        };
        msg_responses.push(MessageResponse {
            message_id: m.message_id,
            user_id: m.user_id,
            user_name: name,
            content: m.content,
            created_at: m.created_at,
        });
    }
    let response = MessageListResponse {
        messages: msg_responses,
    };

    Ok(axum::Json(response))
}

/// POST /threads/:thread_id/messages — add a message to a thread.
async fn add_message(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(thread_id): Path<String>,
    axum::Json(body): axum::Json<AddMessageRequest>,
) -> Result<StatusCode, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;

    // Require Comment access to add messages
    let _meta = super::documents::check_doc_access(
        &state,
        &thread.doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::Comment,
    )
    .await?;

    if body.content.trim().is_empty() {
        return Err(ApiError::BadRequest("Message cannot be empty".to_string()));
    }

    let msg = Message {
        thread_id: thread_id.clone(),
        message_id: nanoid::nanoid!(16),
        user_id: user_id.clone(),
        content: body.content,
        created_at: now_usec(),
        updated_at: None,
    };

    let now = msg.created_at;
    state.thread_repo.add_message(&msg).await?;

    // Bump thread's updated_at timestamp so it appears at the top of listings
    state.thread_repo.bump_updated_at(&thread_id, now_usec()).await?;

    // Notify thread creator about the new reply.
    if thread.created_by != user_id {
        let notif_repo = state.notification_repo.clone();
        let notif_user = thread.created_by.clone();
        let notif_doc_id = thread.doc_id.clone();
        tokio::spawn(async move {
            let notif = Notification {
                notif_id: nanoid::nanoid!(16),
                user_id: notif_user,
                notif_type: NotifType::Commented,
                doc_id: Some(notif_doc_id),
                thread_id: Some(thread_id),
                actor_id: user_id,
                message: "replied to your comment".to_string(),
                read: false,
                created_at: now,
            };
            let _ = notif_repo.create(&notif).await;
        });
    }

    Ok(StatusCode::CREATED)
}

/// DELETE /threads/:thread_id — delete a thread and all its messages.
async fn delete_thread_handler(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(thread_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;

    // Verify access: must be thread creator or have Edit access on the document
    let _meta = super::documents::check_doc_access(
        &state,
        &thread.doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::Edit,
    )
    .await?;

    state.thread_repo.delete_thread(&thread_id).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /threads/:thread_id/messages/:message_id — delete a message.
async fn delete_message(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((thread_id, message_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;

    // Verify access to the parent document
    let _meta = super::documents::check_doc_access(
        &state,
        &thread.doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::Comment,
    )
    .await?;

    // Find the message to verify the caller is the author
    let messages = state.thread_repo.list_messages(&thread_id).await?;
    let msg = messages
        .iter()
        .find(|m| m.message_id == message_id)
        .ok_or(ApiError::NotFound("Message not found".to_string()))?;

    if msg.user_id != user_id {
        return Err(ApiError::Forbidden);
    }

    state.thread_repo.delete_message(&thread_id, &msg.sk()).await?;

    Ok(StatusCode::NO_CONTENT)
}
