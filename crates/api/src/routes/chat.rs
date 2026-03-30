//! Chat endpoints: 1:1 DMs and group chat rooms.
//!
//! Chats reuse the THREAD#/MSG# infrastructure from comments,
//! with ThreadType::Chat and ThreadType::DirectMessage.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::thread::{Message, Thread, ThreadStatus, ThreadType};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_chats))
        .route("/", post(create_chat))
        .route("/{id}", get(get_chat))
        .route("/{id}/members", post(add_member))
        .route("/{id}/members/{user_id}", delete(remove_member))
        .route("/{id}/messages", get(list_messages))
        .route("/{id}/messages", post(send_message))
}

// ─── Types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateChatRequest {
    /// "chat" for group, "directMessage" for 1:1.
    chat_type: ThreadType,
    /// Room title (required for group chats, ignored for DMs).
    #[serde(default)]
    title: Option<String>,
    /// Initial member user IDs (the creator is added automatically).
    member_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatResponse {
    id: String,
    chat_type: ThreadType,
    title: Option<String>,
    member_ids: Vec<String>,
    created_by: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatListResponse {
    chats: Vec<ChatResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateChatResponse {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMemberRequest {
    user_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageRequest {
    content: String,
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
struct MessageListResponse {
    messages: Vec<MessageResponse>,
}

// ─── Helpers ────────────────────────────────────────────────────

fn thread_to_chat_response(t: Thread) -> ChatResponse {
    ChatResponse {
        id: t.thread_id,
        chat_type: t.thread_type,
        title: t.title,
        member_ids: t.member_ids,
        created_by: t.created_by,
        created_at: t.created_at,
        updated_at: t.updated_at,
    }
}

fn check_chat_member(thread: &Thread, user_id: &str) -> Result<(), ApiError> {
    if !thread.member_ids.contains(&user_id.to_string()) {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

// ─── Handlers ───────────────────────────────────────────────────

/// GET /chats — list the user's chats (DMs + group rooms).
async fn list_chats(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<axum::Json<ChatListResponse>, ApiError> {
    let threads = state.thread_repo.list_user_chats(&user_id).await?;
    let chats = threads
        .into_iter()
        .filter(|t| matches!(t.thread_type, ThreadType::Chat | ThreadType::DirectMessage))
        .map(thread_to_chat_response)
        .collect();

    Ok(axum::Json(ChatListResponse { chats }))
}

/// POST /chats — create a new chat room or DM.
async fn create_chat(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    axum::Json(body): axum::Json<CreateChatRequest>,
) -> Result<(StatusCode, axum::Json<CreateChatResponse>), ApiError> {
    // Validate chat type
    if !matches!(body.chat_type, ThreadType::Chat | ThreadType::DirectMessage) {
        return Err(ApiError::BadRequest(
            "chat_type must be 'chat' or 'directMessage'".to_string(),
        ));
    }

    // DMs require exactly one other member
    if body.chat_type == ThreadType::DirectMessage {
        if body.member_ids.len() != 1 {
            return Err(ApiError::BadRequest(
                "Direct messages require exactly one other member".to_string(),
            ));
        }
        if body.member_ids[0] == user_id {
            return Err(ApiError::BadRequest(
                "Cannot create a direct message with yourself".to_string(),
            ));
        }
    }

    // Group chats require a title
    if body.chat_type == ThreadType::Chat && body.title.as_ref().map_or(true, |t| t.trim().is_empty()) {
        return Err(ApiError::BadRequest(
            "Group chats require a title".to_string(),
        ));
    }

    let now = now_usec();
    let thread_id = nanoid::nanoid!(16);

    // Add creator to member list
    let mut member_ids = body.member_ids;
    if !member_ids.contains(&user_id) {
        member_ids.push(user_id.clone());
    }

    let thread = Thread {
        thread_id: thread_id.clone(),
        doc_id: String::new(), // Chats are not attached to a document
        thread_type: body.chat_type,
        status: ThreadStatus::Open,
        created_by: user_id,
        title: body.title,
        member_ids,
        anchor_start: None,
        anchor_end: None,
        created_at: now,
        updated_at: now,
    };

    state.thread_repo.create_thread(&thread).await?;

    Ok((
        StatusCode::CREATED,
        axum::Json(CreateChatResponse { id: thread_id }),
    ))
}

/// GET /chats/:id — get chat details.
async fn get_chat(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<axum::Json<ChatResponse>, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&id)
        .await?
        .ok_or(ApiError::NotFound("Chat not found".to_string()))?;

    check_chat_member(&thread, &user_id)?;

    Ok(axum::Json(thread_to_chat_response(thread)))
}

/// POST /chats/:id/members — add a member to a group chat.
async fn add_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<AddMemberRequest>,
) -> Result<StatusCode, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&id)
        .await?
        .ok_or(ApiError::NotFound("Chat not found".to_string()))?;

    // Only the creator can add members to group chats
    if thread.created_by != user_id {
        return Err(ApiError::Forbidden);
    }

    // Can't add members to DMs
    if thread.thread_type == ThreadType::DirectMessage {
        return Err(ApiError::BadRequest(
            "Cannot add members to a direct message".to_string(),
        ));
    }

    state.thread_repo.add_chat_member(&id, &body.user_id).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /chats/:id/members/:user_id — remove a member or leave a chat.
async fn remove_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, target_user_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&id)
        .await?
        .ok_or(ApiError::NotFound("Chat not found".to_string()))?;

    // You can remove yourself (leave) or the creator can remove others
    if target_user_id != user_id && thread.created_by != user_id {
        return Err(ApiError::Forbidden);
    }

    // Creator cannot be removed (including by themselves)
    if target_user_id == thread.created_by {
        return Err(ApiError::BadRequest(
            "Cannot remove the chat creator".to_string(),
        ));
    }

    state
        .thread_repo
        .remove_chat_member(&id, &target_user_id)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /chats/:id/messages — list messages in a chat.
async fn list_messages(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<axum::Json<MessageListResponse>, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&id)
        .await?
        .ok_or(ApiError::NotFound("Chat not found".to_string()))?;

    check_chat_member(&thread, &user_id)?;

    let messages = state.thread_repo.list_messages(&id).await?;
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

/// POST /chats/:id/messages — send a message to a chat.
async fn send_message(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<SendMessageRequest>,
) -> Result<StatusCode, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&id)
        .await?
        .ok_or(ApiError::NotFound("Chat not found".to_string()))?;

    check_chat_member(&thread, &user_id)?;

    if body.content.trim().is_empty() {
        return Err(ApiError::BadRequest("Message cannot be empty".to_string()));
    }

    let now = now_usec();
    let msg = Message {
        thread_id: id.clone(),
        message_id: nanoid::nanoid!(16),
        user_id,
        content: body.content,
        created_at: now,
        updated_at: None,
    };

    state.thread_repo.add_message(&msg).await?;
    state.thread_repo.bump_updated_at(&id, now).await?;

    Ok(StatusCode::CREATED)
}
