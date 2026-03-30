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
        .route("/{thread_id}/messages", get(list_messages))
        .route("/{thread_id}/messages", post(add_message))
        .route("/{thread_id}/messages/{message_id}", delete(delete_message))
}

// ─── Types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateThreadRequest {
    thread_type: ThreadType,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor_start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor_end: Option<u32>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageResponse {
    message_id: String,
    user_id: String,
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
    let response = ThreadListResponse {
        threads: threads
            .into_iter()
            .map(|t| ThreadResponse {
                thread_id: t.thread_id,
                doc_id: t.doc_id,
                thread_type: t.thread_type,
                status: t.status,
                created_by: t.created_by,
                anchor_start: t.anchor_start,
                anchor_end: t.anchor_end,
                created_at: t.created_at,
                updated_at: t.updated_at,
            })
            .collect(),
    };

    Ok(axum::Json(response))
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

    // Inline comments must have anchor positions
    if body.thread_type == ThreadType::Inline
        && (body.anchor_start.is_none() || body.anchor_end.is_none())
    {
        return Err(ApiError::BadRequest(
            "Inline comments require anchor_start and anchor_end".to_string(),
        ));
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
                user_id,
                content,
                created_at: now,
                updated_at: None,
            };
            state.thread_repo.add_message(&msg).await?;
        }
    }

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
    let response = MessageListResponse {
        messages: messages
            .into_iter()
            .map(|m| MessageResponse {
                message_id: m.message_id,
                user_id: m.user_id,
                content: m.content,
                created_at: m.created_at,
            })
            .collect(),
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
        thread_id,
        message_id: nanoid::nanoid!(16),
        user_id,
        content: body.content,
        created_at: now_usec(),
        updated_at: None,
    };

    state.thread_repo.add_message(&msg).await?;

    // Bump thread's updated_at timestamp so it appears at the top of listings
    state.thread_repo.bump_updated_at(&msg.thread_id, now_usec()).await?;

    Ok(StatusCode::CREATED)
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
