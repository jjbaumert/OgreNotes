// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Comment thread and message endpoints.
//!
//! Threads are attached to documents. Each thread can be inline (anchored to text)
//! or document-level (conversation pane).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use serde::{Deserialize, Serialize};

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::document::DocMember;
use ogrenotes_storage::models::notification::{NotifType, Notification};
use ogrenotes_storage::models::thread::{Mention, Message, MessagePart, Thread, ThreadStatus, ThreadType};
use ogrenotes_storage::models::AccessLevel;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Look up a user's display name, falling back to the raw `user_id` when
/// the row is missing or the repo call fails. Centralizes the rendering
/// policy that `list_threads`, `list_messages`, and the `/invite`
/// announcement each previously inlined (the fallback string had begun to
/// drift between copies). Callers that resolve many names in a loop keep
/// their own per-request cache around this single-name lookup.
async fn resolve_display_name(state: &AppState, user_id: &str) -> String {
    match state.user_repo.get_by_id(user_id).await {
        Ok(Some(u)) => u.name,
        _ => user_id.to_string(),
    }
}

/// Per-user comment-write rate limit (M-E7 item 10). Shared
/// between `create_thread` and `add_message` so the budget caps
/// total comment activity, not per-endpoint slices that could be
/// gamed by cycling between them.
async fn enforce_comments_rate_limit(state: &AppState, user_id: &str) -> Result<(), ApiError> {
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "comments",
        user_id,
        state.config.rate_limit_comments_per_min,
        60,
    )
    .await
}
use crate::routes::slash_commands::{self, SlashCommand};

use ogrenotes_collab::protocol::{encode_message, MessageType};

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
        .route(
            "/{thread_id}/messages/{message_id}/reactions",
            post(add_reaction),
        )
        .route(
            "/{thread_id}/messages/{message_id}/reactions/{emoji}",
            delete(remove_reaction),
        )
        .route("/{thread_id}/notification-level", get(get_notification_level))
        .route("/{thread_id}/notification-level", put(set_notification_level))
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
    #[serde(default)]
    status: Option<ThreadStatus>,
    #[serde(default)]
    anchor_start: Option<u32>,
    #[serde(default)]
    anchor_end: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMessageRequest {
    content: String,
    /// Optional rich-text segments. Clients that don't need styling can
    /// omit this and rely on the plain `content` field.
    #[serde(default)]
    parts: Vec<MessagePart>,
    /// Resolved @-mentions in the message. The backend does not validate
    /// the referenced IDs; it persists them for rendering.
    #[serde(default)]
    mentions: Vec<Mention>,
    /// Blob IDs attached to the message.
    #[serde(default)]
    attachments: Vec<String>,
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
    /// Total number of messages in the thread (including the opening
    /// one). Lets the client render a "N replies" affordance — e.g. the
    /// spreadsheet cell-comment hover preview — without a per-thread
    /// message fetch. Best-effort: 0 if the count query failed.
    message_count: u32,
    /// Read receipts for this thread, one per user who has opened it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    read_by: Vec<ReadReceiptResponse>,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reactions: Vec<ReactionResponse>,
    /// Rich-text segments. Empty for legacy plain-text messages.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    parts: Vec<MessagePart>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mentions: Vec<Mention>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<String>,
}

/// One emoji group on a message: the emoji plus every user_id that reacted
/// with it. Collapsed client-side to "👍 3" pills; the user_ids list lets the
/// UI mark the caller's own reactions as toggled.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReactionResponse {
    emoji: String,
    user_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddReactionRequest {
    emoji: String,
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
    /// Every user who has read this thread, with their last-read timestamp.
    /// Populated on the list_messages call that also records the caller's
    /// own receipt, so the client sees its own read state immediately.
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
struct CreateThreadResponse {
    thread_id: String,
}

/// Truncate comment/reply text to a notification-sized preview so the
/// recipient can tell threads apart (issue #50). Mirrors the 120-char
/// cap used for thread first-message previews.
fn notif_preview_snippet(s: &str) -> String {
    const MAX: usize = 120;
    if s.chars().count() > MAX {
        let mut t: String = s.chars().take(MAX).collect();
        t.push('…');
        t
    } else {
        s.to_string()
    }
}

// ─── Handlers ───────────────────────────────────────────────────

/// GET /documents/:doc_id/threads — list all comment threads for a document.
async fn list_threads(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(doc_id): Path<String>,
) -> Result<axum::Json<ThreadListResponse>, ApiError> {
    // Verify access to the document
    let meta = super::documents::check_doc_access(
        &state,
        &doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;
    // A View-mode link-only viewer sees the conversation only if the link
    // lets them read it (show_conversation) or participate (allow_comments).
    // Durable members and edit-link viewers are unaffected.
    super::documents::enforce_view_link_option(
        &state,
        &meta,
        &user_id,
        meta.link_view_options.show_conversation || meta.link_view_options.allow_comments,
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
            let name = resolve_display_name(&state, &t.created_by).await;
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
                if m.content.chars().count() > 120 {
                    let mut preview: String = m.content.chars().take(120).collect();
                    preview.push_str("...");
                    preview
                } else {
                    m.content
                }
            });

        // Total message count for the thread. Best-effort (0 on error);
        // a `Select::Count` query so it transfers a tally, not bodies.
        // Same per-thread-fanout caveat as the receipt query below.
        let message_count = state
            .thread_repo
            .count_messages(&t.thread_id)
            .await
            .unwrap_or(0);

        // Per-thread read receipts. Best-effort: an error here shouldn't
        // prevent the thread list from rendering.
        //
        // TODO: N+1. Each thread here triggers its own `READ#`-prefix query,
        // so a doc with M threads does M sequential DynamoDB round-trips.
        // Acceptable for MVP thread counts; when threads-per-doc grows into
        // the dozens, batch with `join_all` or fold receipts into one
        // per-doc fan-out.
        let read_by = state
            .thread_repo
            .list_read_receipts_for_thread(&t.thread_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| ReadReceiptResponse {
                user_id: r.user_id,
                last_read_at: r.last_read_at,
            })
            .collect();

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
            message_count,
            read_by,
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
    enforce_comments_rate_limit(&state, &user_id).await?;
    // Require Comment access — satisfied by durable Comment+/Edit, an
    // Edit-mode link, or a View-mode link with `allow_comments` on for a
    // workspace member (link-sharing Phase 2, §5.3).
    let _meta = super::documents::check_comment_access(&state, &doc_id, &user_id).await?;

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

    // Captured before `body.message` is consumed below so the owner's
    // notification can carry a comment preview + anchor for deep-linking
    // back to this exact block/cell (issue #50).
    let notif_preview = body.message.as_deref().map(notif_preview_snippet);
    let notif_block_id = thread.block_id.clone();

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
                parts: Vec::new(),
                mentions: Vec::new(),
                attachments: Vec::new(),
            };
            state.thread_repo.add_message(&msg).await?;
        }
    }

    // Tell other connected editors in this doc that a new thread exists
    // so they can refresh their highlights / conversation pane.
    fanout_comment_event(
        &state,
        &doc_id,
        CommentEventPayload::ThreadCreated {
            thread: CommentEventThread::from(&thread),
        },
    );

    // Record activity event
    {
        let activity_repo = state.activity_repo.clone();
        let act_doc_id = doc_id.clone();
        let act_user_id = user_id.clone();
        let act_thread_id = thread_id.clone();
        tokio::spawn(async move {
            let activity = ogrenotes_storage::models::activity::Activity {
                activity_id: nanoid::nanoid!(16),
                doc_id: act_doc_id,
                event_type: ogrenotes_storage::models::activity::ActivityEventType::Comment,
                actor_id: act_user_id,
                detail: serde_json::json!({ "threadId": act_thread_id }).to_string(),
                created_at: now_usec(),
            };
            let _ = activity_repo.create(&activity).await;
        });
    }

    // Notify document owner about the new comment thread.
    let notif_repo = state.notification_repo.clone();
    let email_service = state.email_service.clone();
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
                    preview: notif_preview,
                    block_id: notif_block_id,
                    read: false,
                    created_at: now_usec(),
                };
                let _ = notif_repo.create(&notif).await;
                // Not a direct reply to the owner; falls under `All` prefs only.
                email_service.spawn_for_notification(notif, false);
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

    if let Some(ref status) = body.status {
        state
            .thread_repo
            .update_status(&thread_id, status.clone(), now_usec())
            .await?;

        fanout_comment_event(
            &state,
            &thread.doc_id,
            CommentEventPayload::ThreadStatusChanged {
                thread_id: thread_id.clone(),
                status: status.clone(),
            },
        );

        // Emit ResolveComment activity when thread is resolved
        if *status == ThreadStatus::Resolved {
            let activity_repo = state.activity_repo.clone();
            let act_doc_id = thread.doc_id.clone();
            let act_user_id = user_id.clone();
            let act_thread_id = thread_id.clone();
            tokio::spawn(async move {
                let activity = ogrenotes_storage::models::activity::Activity {
                    activity_id: nanoid::nanoid!(16),
                    doc_id: act_doc_id,
                    event_type: ogrenotes_storage::models::activity::ActivityEventType::ResolveComment,
                    actor_id: act_user_id,
                    detail: serde_json::json!({ "threadId": act_thread_id }).to_string(),
                    created_at: ogrenotes_common::time::now_usec(),
                };
                let _ = activity_repo.create(&activity).await;
            });
        }
    }

    if let (Some(start), Some(end)) = (body.anchor_start, body.anchor_end) {
        state
            .thread_repo
            .update_anchors(&thread_id, start, end)
            .await?;
    }

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
    let meta = super::documents::check_doc_access(
        &state,
        &thread.doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;
    // Same conversation gate as list_threads (gap-001): a View-mode
    // link-only viewer reads messages only when show_conversation or
    // allow_comments is on.
    super::documents::enforce_view_link_option(
        &state,
        &meta,
        &user_id,
        meta.link_view_options.show_conversation || meta.link_view_options.allow_comments,
    )
    .await?;

    let messages = state.thread_repo.list_messages(&thread_id).await?;

    // Record this caller's read of the thread. Best-effort: a failed
    // upsert shouldn't block the user from actually seeing the messages.
    // The repo uses a conditional write that rejects stale timestamps, so
    // concurrent GETs can't roll the receipt backwards.
    if let Err(e) = state
        .thread_repo
        .upsert_read_receipt(&thread_id, &user_id, now_usec())
        .await
    {
        tracing::warn!(thread_id, user_id, "upsert_read_receipt failed: {e}");
    }

    // One range query pulls every reaction on the thread; we group by
    // message_id so the per-message loop is O(r) lookups, not O(m × r).
    let reactions = state
        .thread_repo
        .list_reactions_for_thread(&thread_id)
        .await?;
    let mut reactions_by_msg: std::collections::HashMap<String, Vec<ReactionResponse>> =
        std::collections::HashMap::new();
    for r in reactions {
        if r.user_ids.is_empty() {
            continue; // empty-set row from a last-user removal race
        }
        reactions_by_msg
            .entry(r.message_id.clone())
            .or_default()
            .push(ReactionResponse {
                emoji: r.emoji,
                user_ids: r.user_ids,
            });
    }

    let mut user_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut msg_responses = Vec::with_capacity(messages.len());
    for m in messages {
        let name = if let Some(cached) = user_names.get(&m.user_id) {
            cached.clone()
        } else {
            let name = resolve_display_name(&state, &m.user_id).await;
            user_names.insert(m.user_id.clone(), name.clone());
            name
        };
        let msg_reactions = reactions_by_msg.remove(&m.message_id).unwrap_or_default();
        msg_responses.push(MessageResponse {
            message_id: m.message_id,
            user_id: m.user_id,
            user_name: name,
            content: m.content,
            created_at: m.created_at,
            reactions: msg_reactions,
            parts: m.parts,
            mentions: m.mentions,
            attachments: m.attachments,
        });
    }
    // Pull every read receipt so the client can render per-message read
    // state without a second round-trip.
    let read_by = state
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

    let response = MessageListResponse {
        messages: msg_responses,
        read_by,
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
    enforce_comments_rate_limit(&state, &user_id).await?;
    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;

    // Require Comment access — see create_thread; honors the
    // `allow_comments` View-mode link sub-option (Phase 2, §5.3).
    let _meta = super::documents::check_comment_access(&state, &thread.doc_id, &user_id).await?;

    if body.content.trim().is_empty() {
        return Err(ApiError::BadRequest("Message cannot be empty".to_string()));
    }

    // If the message is a recognized slash command, execute it first. The
    // stored message is replaced with a system announcement so the thread
    // history reads "Alice invited Bob" rather than "/invite @bob". User-
    // supplied `parts` / `mentions` / `attachments` are ignored on a
    // slash-command path — the rewritten content is authoritative.
    let (content, parts, mentions, attachments) = match slash_commands::try_parse(&body.content) {
        Some(SlashCommand::Invite { handle }) => {
            let (c, p) = run_invite_in_doc_thread(&state, &thread, &user_id, handle).await?;
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
        thread_id: thread_id.clone(),
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

    // Bump thread's updated_at to match the message timestamp exactly
    state.thread_repo.bump_updated_at(&thread_id, now).await?;

    // Tell other connected editors that a reply landed in this thread.
    // Skip chat threads — they aren't tied to a doc/room.
    if !thread.doc_id.is_empty() {
        fanout_comment_event(
            &state,
            &thread.doc_id,
            CommentEventPayload::MessageAdded {
                message: CommentEventMessage::from(&msg),
            },
        );
    }

    // Record a Comment activity row on the document so the activity feed
    // picks up replies, not just new threads. Skip chat threads — their
    // `doc_id` is empty and activity is scoped per-document.
    if !thread.doc_id.is_empty() {
        let activity_repo = state.activity_repo.clone();
        let act_doc_id = thread.doc_id.clone();
        let act_user_id = user_id.clone();
        let act_thread_id = thread_id.clone();
        tokio::spawn(async move {
            let activity = ogrenotes_storage::models::activity::Activity {
                activity_id: nanoid::nanoid!(16),
                doc_id: act_doc_id,
                event_type: ogrenotes_storage::models::activity::ActivityEventType::Comment,
                actor_id: act_user_id,
                detail: serde_json::json!({ "threadId": act_thread_id }).to_string(),
                created_at: now,
            };
            let _ = activity_repo.create(&activity).await;
        });
    }

    // Notify thread creator about the new reply (gated by notification prefs).
    if thread.created_by != user_id {
        let notif_repo = state.notification_repo.clone();
        let email_service = state.email_service.clone();
        let notif_user = thread.created_by.clone();
        let notif_doc_id = thread.doc_id.clone();
        let notif_thread_id = thread_id.clone();
        // Reply preview + anchor for the recipient's deep-link (#50).
        let notif_preview = Some(notif_preview_snippet(&msg.content));
        let notif_block_id = thread.block_id.clone();
        tokio::spawn(async move {
            // Direct reply → is_direct = true
            if notif_repo.should_notify(&notif_user, &notif_thread_id, true).await {
                let notif = Notification {
                    notif_id: nanoid::nanoid!(16),
                    user_id: notif_user,
                    notif_type: NotifType::Commented,
                    doc_id: Some(notif_doc_id),
                    thread_id: Some(notif_thread_id),
                    actor_id: user_id,
                    message: "replied to your comment".to_string(),
                    preview: notif_preview,
                    block_id: notif_block_id,
                    read: false,
                    created_at: now,
                };
                let _ = notif_repo.create(&notif).await;
                email_service.spawn_for_notification(notif, true);
            }
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

    // Thread creator needs only Comment access; non-creators need Edit access
    let is_creator = thread.created_by == user_id;
    let required = if is_creator {
        ogrenotes_storage::models::AccessLevel::Comment
    } else {
        ogrenotes_storage::models::AccessLevel::Edit
    };
    let _meta = super::documents::check_doc_access(
        &state,
        &thread.doc_id,
        &user_id,
        required,
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

// ─── Slash commands ─────────────────────────────────────────────

/// Execute `/invite @handle` against a document-attached thread. The caller
/// needs `Own` access (share permission) on the doc. The invitee is granted
/// `Comment` access — enough to reply in the thread without also handing
/// them edit rights on the underlying document.
async fn run_invite_in_doc_thread(
    state: &AppState,
    thread: &Thread,
    caller_id: &str,
    handle: &str,
) -> Result<(String, Vec<MessagePart>), ApiError> {
    // Share permission required to grant others access.
    let _meta = super::documents::check_doc_access(
        state,
        &thread.doc_id,
        caller_id,
        AccessLevel::Own,
    )
    .await?;

    let invitee = slash_commands::resolve_handle(&state.user_repo, handle).await?;
    if invitee.user_id == caller_id {
        return Err(ApiError::BadRequest(
            "You're already on this document".to_string(),
        ));
    }

    // Reject if the invitee is already a direct doc member; silent re-invites
    // would otherwise post a stream of "Alice invited Bob" lines with no
    // actual permission change.
    if let Ok(Some(_)) = state
        .doc_repo
        .get_doc_member(&thread.doc_id, &invitee.user_id)
        .await
    {
        return Err(ApiError::Conflict(
            "User already has access to this document".to_string(),
        ));
    }

    // Exclusive put: if two concurrent /invite calls both passed the
    // get_doc_member check above, the second write rejects instead of
    // silently upserting. The RepoError maps to ApiError::Conflict via the
    // existing "ConditionalCheckFailed" branch in error.rs.
    state
        .doc_repo
        .add_doc_member_exclusive(&DocMember {
            doc_id: thread.doc_id.clone(),
            user_id: invitee.user_id.clone(),
            access_level: AccessLevel::Comment,
            added_at: now_usec(),
        })
        .await?;

    // Resolve the caller's display name for the announcement. Fall back to
    // the raw user_id if the lookup fails so the message still reads sanely.
    // Use the invitee's display name (not email) — the announcement is
    // visible to every reader of the thread, and broadcasting an email
    // address would leak PII.
    let caller_name = resolve_display_name(state, caller_id).await;
    Ok(slash_commands::invite_announcement(&caller_name, &invitee.name))
}

// ─── Reactions ──────────────────────────────────────────────────

/// Gate reaction access. Document-attached threads (inline / document) check
/// doc-level permissions at Comment level; standalone chat threads (Chat /
/// DirectMessage) have an empty `doc_id` and instead gate on
/// `thread.member_ids`. This mirrors the split between `routes/comments.rs`
/// (doc threads) and `routes/chat.rs` (chat threads).
async fn require_reaction_access(
    state: &AppState,
    thread: &Thread,
    user_id: &str,
) -> Result<(), ApiError> {
    if thread.doc_id.is_empty() {
        if !thread.member_ids.iter().any(|m| m == user_id) {
            return Err(ApiError::Forbidden);
        }
        return Ok(());
    }
    let _meta = super::documents::check_doc_access(
        state,
        &thread.doc_id,
        user_id,
        ogrenotes_storage::models::AccessLevel::Comment,
    )
    .await?;
    Ok(())
}

/// POST /threads/:thread_id/messages/:message_id/reactions
/// Body: `{ emoji: "👍" }`
///
/// Idempotent: adding the same (user, emoji) twice is a no-op thanks to the
/// string-set ADD in the repo.
async fn add_reaction(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((thread_id, message_id)): Path<(String, String)>,
    axum::Json(body): axum::Json<AddReactionRequest>,
) -> Result<StatusCode, ApiError> {
    let emoji = body.emoji.trim();
    if emoji.is_empty() {
        return Err(ApiError::BadRequest("emoji cannot be empty".to_string()));
    }
    // Sanity bound so a hostile caller can't stuff megabytes into a SK.
    if emoji.chars().count() > 32 {
        return Err(ApiError::BadRequest("emoji too long".to_string()));
    }

    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;

    require_reaction_access(&state, &thread, &user_id).await?;

    // Reject reactions targeting a message that does not exist in the thread.
    // Without this, DynamoDB will happily create a REACTION# row for any
    // message_id the caller invents, producing orphaned rows that never
    // surface in list_messages output but consume storage indefinitely.
    let messages = state.thread_repo.list_messages(&thread_id).await?;
    if !messages.iter().any(|m| m.message_id == message_id) {
        return Err(ApiError::NotFound("Message not found".to_string()));
    }

    state
        .thread_repo
        .add_reaction(&thread_id, &message_id, emoji, &user_id)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /threads/:thread_id/messages/:message_id/reactions/:emoji
///
/// No-op if the caller was not in the set; removing the last user in a set
/// deletes the underlying row (handled by the repo).
async fn remove_reaction(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((thread_id, message_id, emoji)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;

    require_reaction_access(&state, &thread, &user_id).await?;

    state
        .thread_repo
        .remove_reaction(&thread_id, &message_id, &emoji, &user_id)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Notification level ─────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotifLevelResponse {
    level: ogrenotes_storage::models::NotifLevel,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetNotifLevelRequest {
    level: ogrenotes_storage::models::NotifLevel,
}

/// GET /threads/:thread_id/notification-level
async fn get_notification_level(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(thread_id): Path<String>,
) -> Result<axum::Json<NotifLevelResponse>, ApiError> {
    // Verify caller has access to the thread's parent document
    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;
    let _meta = super::documents::check_doc_access(
        &state,
        &thread.doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;

    let level = state
        .notification_repo
        .get_pref(&user_id, &thread_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map(|p| p.level)
        .unwrap_or_default();

    Ok(axum::Json(NotifLevelResponse { level }))
}

/// PUT /threads/:thread_id/notification-level
async fn set_notification_level(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(thread_id): Path<String>,
    axum::Json(body): axum::Json<SetNotifLevelRequest>,
) -> Result<StatusCode, ApiError> {
    // Verify caller has access to the thread's parent document
    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;
    let _meta = super::documents::check_doc_access(
        &state,
        &thread.doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;

    let pref = ogrenotes_storage::models::notification::NotifPref {
        user_id,
        thread_id,
        level: body.level,
    };
    state
        .notification_repo
        .set_pref(&pref)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Realtime broadcast ─────────────────────────────────────────

/// Snapshot of a thread sent inline with `thread_created` events so peers
/// can update their `inline_threads` signal without a follow-up
/// `GET /threads` round-trip. Field-for-field projection of `Thread`
/// minus the chat-only fields (`title`, `member_ids`); doc-attached
/// threads never populate those.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommentEventThread {
    thread_id: String,
    doc_id: String,
    thread_type: ThreadType,
    status: ThreadStatus,
    created_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor_start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor_end: Option<u32>,
    created_at: i64,
    updated_at: i64,
}

impl From<&Thread> for CommentEventThread {
    fn from(t: &Thread) -> Self {
        Self {
            thread_id: t.thread_id.clone(),
            doc_id: t.doc_id.clone(),
            thread_type: t.thread_type.clone(),
            status: t.status.clone(),
            created_by: t.created_by.clone(),
            block_id: t.block_id.clone(),
            anchor_start: t.anchor_start,
            anchor_end: t.anchor_end,
            created_at: t.created_at,
            updated_at: t.updated_at,
        }
    }
}

/// Snapshot of a message sent inline with `message_added` events.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommentEventMessage {
    thread_id: String,
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

impl From<&Message> for CommentEventMessage {
    fn from(m: &Message) -> Self {
        Self {
            thread_id: m.thread_id.clone(),
            message_id: m.message_id.clone(),
            user_id: m.user_id.clone(),
            content: m.content.clone(),
            created_at: m.created_at,
            parts: m.parts.clone(),
            mentions: m.mentions.clone(),
            attachments: m.attachments.clone(),
        }
    }
}

/// Side-channel notification carried in the `CommentEvent` WebSocket
/// frame. Each variant carries enough state for peers to update their
/// in-memory thread list directly — no follow-up `GET /threads` needed.
/// The `kind` discriminant matches what the previous wire format used,
/// so an older frontend that only reads `kind` keeps working (it just
/// falls back to refetching).
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CommentEventPayload {
    ThreadCreated {
        thread: CommentEventThread,
    },
    #[serde(rename_all = "camelCase")]
    ThreadStatusChanged {
        thread_id: String,
        status: ThreadStatus,
    },
    MessageAdded {
        message: CommentEventMessage,
    },
}

/// Tell every collaborator viewing `doc_id` that the comment threads on
/// this doc just changed, so they can refresh their thread list / inline
/// highlights without a manual reload.
///
/// Comments live in the thread DB, not in the CRDT, so the regular yrs
/// update broadcast doesn't carry them. We send a `CommentEvent` frame
/// alongside it: locally to clients connected to this server instance,
/// and via Redis pub/sub for clients on other instances. Failures are
/// logged but never propagated — the REST write already succeeded and
/// other peers will catch up the next time they refetch.
fn fanout_comment_event(state: &AppState, doc_id: &str, payload: CommentEventPayload) {
    let body = match serde_json::to_vec(&payload) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(doc_id = %doc_id, error = %e, "comment event serialize failed");
            return;
        }
    };
    let frame = encode_message(MessageType::CommentEvent, &body);

    if let Some(room) = state.room_registry.get(doc_id) {
        let frame_local = frame.clone();
        tokio::spawn(async move {
            // exclude_client = 0 ⇒ deliver to every connected client; the
            // creator's own client deduplicates by thread_id when it
            // refetches.
            room.broadcast(0, frame_local).await;
        });
    }

    let pubsub = state.redis_pubsub.clone();
    let doc_id_owned = doc_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = pubsub.publish_update(&doc_id_owned, &frame).await {
            tracing::warn!(doc_id = %doc_id_owned, error = %e, "comment event publish failed");
        }
    });
}
