// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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
use ogrenotes_storage::models::notification::{NotifType, Notification};
use ogrenotes_storage::models::thread::{Mention, Message, MessagePart, Thread, ThreadStatus, ThreadType};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::routes::slash_commands::{self, SlashCommand};
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
    /// Read receipts. Populated on list/get; empty on create responses.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    read_by: Vec<ReadReceiptResponse>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ReadReceiptResponse {
    user_id: String,
    last_read_at: i64,
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
    #[serde(default)]
    parts: Vec<MessagePart>,
    #[serde(default)]
    mentions: Vec<Mention>,
    #[serde(default)]
    attachments: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageResponse {
    message_id: String,
    user_id: String,
    content: String,
    created_at: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    parts: Vec<MessagePart>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mentions: Vec<Mention>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageListResponse {
    messages: Vec<MessageResponse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    read_by: Vec<ReadReceiptResponse>,
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
        read_by: Vec::new(),
    }
}

fn check_chat_member(thread: &Thread, user_id: &str) -> Result<(), ApiError> {
    if !thread.member_ids.contains(&user_id.to_string()) {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

/// Execute `/invite @handle` inside a chat thread. Only the chat creator
/// can add members (same rule as the explicit `POST /chats/:id/members`
/// endpoint), and DMs refuse the command outright.
async fn run_invite_in_chat(
    state: &AppState,
    thread: &Thread,
    caller_id: &str,
    handle: &str,
) -> Result<(String, Vec<MessagePart>), ApiError> {
    if thread.thread_type == ThreadType::DirectMessage {
        return Err(ApiError::BadRequest(
            "Cannot invite additional members into a direct message".to_string(),
        ));
    }
    if thread.created_by != caller_id {
        return Err(ApiError::Forbidden);
    }

    let invitee = slash_commands::resolve_handle(&state.user_repo, handle).await?;
    if invitee.user_id == caller_id {
        return Err(ApiError::BadRequest(
            "You're already in this chat".to_string(),
        ));
    }
    if thread.member_ids.iter().any(|m| m == &invitee.user_id) {
        return Err(ApiError::Conflict(
            "User is already a member of this chat".to_string(),
        ));
    }

    state
        .thread_repo
        .add_chat_member(&thread.thread_id, &invitee.user_id)
        .await?;

    // Use the invitee's display name, not their email — the announcement
    // is visible to every chat member, and surfacing the email would leak
    // PII to other participants.
    let caller_name = match state.user_repo.get_by_id(caller_id).await {
        Ok(Some(u)) => u.name,
        _ => caller_id.to_string(),
    };
    Ok(slash_commands::invite_announcement(&caller_name, &invitee.name))
}

// ─── Handlers ───────────────────────────────────────────────────

/// GET /chats — list the user's chats (DMs + group rooms).
async fn list_chats(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<axum::Json<ChatListResponse>, ApiError> {
    let threads = state.thread_repo.list_user_chats(&user_id).await?;

    // TODO: N+1. One `READ#`-prefix query per chat the user is a member of.
    // For users with many chats this is a visible latency cost; consider
    // batching with `join_all` or a GSI-backed fan-out when the count grows.
    let mut chats = Vec::new();
    for t in threads {
        if !matches!(t.thread_type, ThreadType::Chat | ThreadType::DirectMessage) {
            continue;
        }
        let thread_id = t.thread_id.clone();
        let mut chat = thread_to_chat_response(t);
        chat.read_by = state
            .thread_repo
            .list_read_receipts_for_thread(&thread_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| ReadReceiptResponse {
                user_id: r.user_id,
                last_read_at: r.last_read_at,
            })
            .collect();
        chats.push(chat);
    }

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
        block_id: None,
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

    let thread_id = thread.thread_id.clone();
    let mut response = thread_to_chat_response(thread);
    response.read_by = state
        .thread_repo
        .list_read_receipts_for_thread(&thread_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| ReadReceiptResponse {
            user_id: r.user_id,
            last_read_at: r.last_read_at,
        })
        .collect();

    Ok(axum::Json(response))
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

    // Verify the target user exists.
    if state.user_repo.get_by_id(&body.user_id).await?.is_none() {
        return Err(ApiError::NotFound("User not found".to_string()));
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

    // Record the caller's read of the chat. Best-effort; a failed upsert
    // should not block rendering messages.
    if let Err(e) = state
        .thread_repo
        .upsert_read_receipt(&id, &user_id, now_usec())
        .await
    {
        tracing::warn!(thread_id = %id, user_id = %user_id, "upsert_read_receipt failed: {e}");
    }

    let messages = state.thread_repo.list_messages(&id).await?;

    let read_by = state
        .thread_repo
        .list_read_receipts_for_thread(&id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| ReadReceiptResponse {
            user_id: r.user_id,
            last_read_at: r.last_read_at,
        })
        .collect();

    let response = MessageListResponse {
        messages: messages
            .into_iter()
            .map(|m| MessageResponse {
                message_id: m.message_id,
                user_id: m.user_id,
                content: m.content,
                created_at: m.created_at,
                parts: m.parts,
                mentions: m.mentions,
                attachments: m.attachments,
            })
            .collect(),
        read_by,
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

    // Slash-command dispatch for chats. `/invite` rewrites the message to a
    // system-styled announcement; `/shrug`, `/tableflip`, `/unflip`, and
    // `/me` are built-in decorations that need no external services.
    let (content, parts, mentions, attachments) = match slash_commands::try_parse(&body.content) {
        Some(SlashCommand::Invite { handle }) => {
            let (c, p) = run_invite_in_chat(&state, &thread, &user_id, handle).await?;
            (c, p, Vec::new(), Vec::new())
        }
        Some(SlashCommand::Kaomoji { kind, prefix }) => {
            let (c, p) = slash_commands::render_kaomoji(kind, prefix);
            (c, p, Vec::new(), Vec::new())
        }
        Some(SlashCommand::Me { action }) => {
            let actor_name = slash_commands::resolve_actor_name(&state.user_repo, &user_id).await;
            let (c, p) = slash_commands::me_announcement(&actor_name, action);
            (c, p, Vec::new(), Vec::new())
        }
        None => (body.content, body.parts, body.mentions, body.attachments),
    };

    let now = now_usec();
    let msg = Message {
        thread_id: id.clone(),
        message_id: nanoid::nanoid!(16),
        user_id: user_id.clone(),
        content,
        created_at: now,
        updated_at: None,
        parts,
        mentions,
        attachments,
    };

    state.thread_repo.add_message(&msg).await?;
    state.thread_repo.bump_updated_at(&id, now).await?;

    // Notify other chat members about the new message.
    let notif_repo = state.notification_repo.clone();
    let email_service = state.email_service.clone();
    let members = thread.member_ids.clone();
    let sender = user_id.clone();
    let chat_id = id.clone();
    tokio::spawn(async move {
        for member_id in members {
            if member_id == sender {
                continue;
            }
            // Gate by notification preferences (chat message → is_direct = false)
            if !notif_repo.should_notify(&member_id, &chat_id, false).await {
                continue;
            }
            let notif = Notification {
                notif_id: nanoid::nanoid!(16),
                user_id: member_id,
                notif_type: NotifType::ChatMessage,
                doc_id: None,
                thread_id: Some(chat_id.clone()),
                actor_id: sender.clone(),
                message: "sent a message in chat".to_string(),
                preview: None,
                block_id: None,
                read: false,
                created_at: now,
            };
            let _ = notif_repo.create(&notif).await;
            email_service.spawn_for_notification(notif, false);
        }
    });

    Ok(StatusCode::CREATED)
}
