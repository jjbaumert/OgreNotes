# Comment Editing + "edited" Marker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a comment author edit their own inline/document comments, with a subtle "edited" marker (exact time on hover). No stored prior versions.

**Architecture:** Add an author-only `PATCH /threads/{thread_id}/messages/{message_id}` handler that overwrites message `content` and sets the already-existing-but-unused `Message.updated_at`. A new `update_message` repo method does the DynamoDB write (same key, so ordering is preserved). `updated_at` is surfaced on the `MessageResponse` DTO and the frontend `MessageItem`; a `MessageEdited` live-sync event triggers peers to refetch. The comment popup gains an author-only edit affordance and renders the marker.

**Tech Stack:** Rust (axum, aws-sdk-dynamodb), Leptos/WASM frontend, Fluent (i18n).

## Global Constraints

- **Comments only.** Editing is allowed only when `thread.thread_type ∈ {Inline, Document}`. Chat/DM messages are not editable (return `403 Forbidden`).
- **Author only.** Only `msg.user_id == user_id` may edit; mirror `delete_message`'s check. Others get `403 Forbidden`.
- **No re-notify on edit; do not bump `thread.updated_at`.** Editing is not new thread activity.
- **No stored prior versions, no audit row, no activity-feed event.**
- **Don't `git add -A`** in this repo — stage only the files each task names.
- **Frontend is outside the workspace** — build/typecheck it with `cd frontend` first.
- Identifiers stay raw `String` (project convention). DTOs use `#[serde(rename_all = "camelCase")]`.

---

### Task 1: Backend edit endpoint

Adds the repo method, the DTO field, the handler, the route, and the live-sync variant. Verified by integration tests written first against the real test harness (`common::require_infra!()` — needs local DynamoDB, same as every other `test_comments.rs` test).

**Files:**
- Modify: `crates/storage/src/repo/thread_repo.rs` (add `update_message`; extend the top `use` of `crate::models::thread`)
- Modify: `crates/api/src/routes/comments.rs` (add `updated_at` to `MessageResponse` + its populate site; add `EditMessageRequest`; add `edit_message` handler; add route; add `MessageEdited` payload variant)
- Test: `crates/api/tests/test_comments.rs` (append five tests)

**Interfaces:**
- Consumes: existing `ThreadRepo::list_messages`, `ThreadRepo::get_thread`, `Message::sk()`, `super::documents::check_comment_access`, `enforce_comments_rate_limit`, `fanout_comment_event`, `CommentEventMessage::from(&Message)`, `now_usec()`.
- Produces:
  - `ThreadRepo::update_message(&self, thread_id: &str, sk: &str, content: &str, parts: &[MessagePart], mentions: &[Mention], updated_at: i64) -> Result<(), RepoError>`
  - Route `PATCH /api/v1/threads/{thread_id}/messages/{message_id}` → `204 No Content` on success.
  - `MessageResponse.updated_at: Option<i64>` (camelCase `updatedAt`, omitted when `None`).

---

- [ ] **Step 1: Write the failing integration tests**

Append to the end of `crates/api/tests/test_comments.rs` (before the final line if there is trailing content; otherwise at EOF):

```rust
// ─── Edit message ──────────────────────────────────────────────

#[tokio::test]
async fn test_edit_message_author() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, msg_id) = seed_thread_with_message(&app, &token, &doc_id).await;

    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}"),
            Some(&token),
            Some(serde_json::json!({ "content": "edited text" })),
        )
        .await;
    assert_eq!(status, 204);

    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(msgs["messages"][0]["content"], "edited text");
    let updated_at = msgs["messages"][0]["updatedAt"].as_i64();
    assert!(updated_at.is_some(), "updatedAt must be set after an edit");
    let created_at = msgs["messages"][0]["createdAt"].as_i64().unwrap();
    assert!(
        updated_at.unwrap() >= created_at,
        "updatedAt ({:?}) must be >= createdAt ({created_at})",
        updated_at
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_edit_message_non_author_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;
    share_folder(&app, &token_a, &folder_id, &bob_id, "COMMENT").await;

    let (thread_id, msg_id) = seed_thread_with_message(&app, &token_a, &doc_id).await;

    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}"),
            Some(&token_b),
            Some(serde_json::json!({ "content": "hijacked" })),
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_edit_message_empty_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, msg_id) = seed_thread_with_message(&app, &token, &doc_id).await;

    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}"),
            Some(&token),
            Some(serde_json::json!({ "content": "   " })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_edit_chat_message_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _token_b) = app.create_user("bob@test.com").await;

    // Create a chat room (thread_type = Chat) with a message.
    let body = serde_json::json!({ "chatType": "chat", "title": "Team", "memberIds": [bob_id] });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap().to_string();

    app.json_request(
        Method::POST,
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&token_a),
        Some(serde_json::json!({ "content": "hey team" })),
    )
    .await;
    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_a),
            None,
        )
        .await;
    let msg_id = msgs["messages"][0]["messageId"].as_str().unwrap().to_string();

    // Alice authored it, but chat/DM messages are not editable via the
    // comments edit path — the thread_type gate must reject with 403.
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/threads/{chat_id}/messages/{msg_id}"),
            Some(&token_a),
            Some(serde_json::json!({ "content": "edit attempt" })),
        )
        .await;
    assert_eq!(status, 403, "chat messages must not be editable via the comments path");

    app.cleanup().await;
}

#[tokio::test]
async fn test_edit_message_preserves_created_at_and_thread_updated_at() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, msg_id) = seed_thread_with_message(&app, &token, &doc_id).await;

    // Capture message createdAt and thread updatedAt BEFORE the edit.
    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    let created_before = msgs["messages"][0]["createdAt"].as_i64().unwrap();

    let (_, threads) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            None,
        )
        .await;
    let thread_updated_before = threads["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["threadId"].as_str() == Some(&thread_id))
        .unwrap()["updatedAt"]
        .as_i64()
        .unwrap();

    // Sleep so a bug that bumped either timestamp would move it measurably.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}"),
            Some(&token),
            Some(serde_json::json!({ "content": "changed" })),
        )
        .await;
    assert_eq!(status, 204);

    let (_, msgs_after) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(
        msgs_after["messages"][0]["createdAt"].as_i64().unwrap(),
        created_before,
        "editing must not change the message createdAt"
    );

    let (_, threads_after) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            None,
        )
        .await;
    let thread_updated_after = threads_after["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["threadId"].as_str() == Some(&thread_id))
        .unwrap()["updatedAt"]
        .as_i64()
        .unwrap();
    assert_eq!(
        thread_updated_after, thread_updated_before,
        "editing a message must not resurface the thread (thread.updatedAt unchanged)"
    );

    app.cleanup().await;
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ogrenotes-api --test test_comments edit_message edit_chat_message -- --nocapture`
Expected: compile error / FAIL — `PATCH .../messages/{id}` isn't routed yet, so edits return `405`/`404` and `updatedAt` is absent. (If local DynamoDB isn't running, `require_infra!()` skips the tests — start it the same way the other `test_comments.rs` tests are run in this repo, e.g. the CI `docker`-backed DynamoDB Local, then re-run.)

- [ ] **Step 3: Add the `update_message` repo method**

In `crates/storage/src/repo/thread_repo.rs`, extend the model import near the top:

```rust
use crate::models::thread::{Mention, Message, MessagePart, Reaction, ReadReceipt, Thread, ThreadStatus, ThreadType};
```

Then, in `impl ThreadRepo`, add this method directly after `delete_message` (after the closing `}` of `delete_message`, before the `// ─── Reactions ───` divider):

```rust
    /// Edit an existing message: overwrite `content`, set `updated_at`, and
    /// replace the rich `parts`/`mentions` — removing those attributes when
    /// the edit carries none, so a message that used to be rich doesn't keep
    /// stale segments that no longer match the new text. `attachments` are
    /// intentionally left untouched (editing the text shouldn't drop a file
    /// the author attached). The SK is unchanged, so message ordering and
    /// the `created_at` embedded in it are preserved.
    pub async fn update_message(
        &self,
        thread_id: &str,
        sk: &str,
        content: &str,
        parts: &[MessagePart],
        mentions: &[Mention],
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("THREAD#{thread_id}");

        // `content` is aliased via #content out of caution (reserved-word
        // safety); `updated_at` is written raw elsewhere (bump_updated_at),
        // so it needs no alias.
        let mut set_clauses = vec![
            "#content = :content".to_string(),
            "updated_at = :updated_at".to_string(),
        ];
        let mut remove_clauses: Vec<&str> = Vec::new();

        let mut values = HashMap::new();
        values.insert(":content".to_string(), AttributeValue::S(content.to_string()));
        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));

        if parts.is_empty() {
            remove_clauses.push("parts");
        } else {
            let json = serde_json::to_string(parts)
                .map_err(|e| RepoError::MissingField(format!("parts: {e}")))?;
            values.insert(":parts".to_string(), AttributeValue::S(json));
            set_clauses.push("parts = :parts".to_string());
        }
        if mentions.is_empty() {
            remove_clauses.push("mentions");
        } else {
            let json = serde_json::to_string(mentions)
                .map_err(|e| RepoError::MissingField(format!("mentions: {e}")))?;
            values.insert(":mentions".to_string(), AttributeValue::S(json));
            set_clauses.push("mentions = :mentions".to_string());
        }

        let mut expr = format!("SET {}", set_clauses.join(", "));
        if !remove_clauses.is_empty() {
            expr.push_str(&format!(" REMOVE {}", remove_clauses.join(", ")));
        }

        let mut names = HashMap::new();
        names.insert("#content".to_string(), "content".to_string());

        self.db
            .update_item(&pk, sk, &expr, values, Some(names))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
```

- [ ] **Step 4: Surface `updated_at` on the response DTO**

In `crates/api/src/routes/comments.rs`, add the field to `MessageResponse` (after `created_at`):

```rust
    created_at: i64,
    /// Present only when the message has been edited. Drives the frontend's
    /// "edited" marker; omitted (not null) for never-edited messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<i64>,
```

Then populate it in `list_messages`, in the `msg_responses.push(MessageResponse { … })` block — add after `created_at: m.created_at,`:

```rust
            created_at: m.created_at,
            updated_at: m.updated_at,
```

- [ ] **Step 5: Add the request DTO, the `MessageEdited` variant, and the handler**

In `crates/api/src/routes/comments.rs`, add the request DTO next to `AddMessageRequest`:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditMessageRequest {
    content: String,
    /// Optional rich-text segments, mirroring `AddMessageRequest`. The
    /// current web client sends plain `content` only, so these default to
    /// empty and the stored rich fields are cleared on edit.
    #[serde(default)]
    parts: Vec<MessagePart>,
    #[serde(default)]
    mentions: Vec<Mention>,
}
```

Add the `MessageEdited` variant to `CommentEventPayload` (after `MessageAdded`):

```rust
    MessageAdded {
        message: CommentEventMessage,
    },
    MessageEdited {
        message: CommentEventMessage,
    },
}
```

Add the handler directly after `delete_message`:

```rust
/// PATCH /threads/:thread_id/messages/:message_id — edit a message.
///
/// Author-only, comments-only. Mirrors `delete_message`'s auth check and
/// adds a thread-type gate: chat/DM messages are not editable here. Sets
/// `updated_at` (drives the client's "edited" marker) but never bumps
/// `thread.updated_at` — an edit is not new activity — and fires no
/// notifications.
async fn edit_message(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((thread_id, message_id)): Path<(String, String)>,
    axum::Json(body): axum::Json<EditMessageRequest>,
) -> Result<StatusCode, ApiError> {
    enforce_comments_rate_limit(&state, &user_id).await?;

    let thread = state
        .thread_repo
        .get_thread(&thread_id)
        .await?
        .ok_or(ApiError::NotFound("Thread not found".to_string()))?;

    // Comments only — chat/DM messages are not editable via this path.
    // Checked before doc-access so we never call check_comment_access with
    // a chat thread's empty doc_id.
    if !matches!(thread.thread_type, ThreadType::Inline | ThreadType::Document) {
        return Err(ApiError::Forbidden);
    }

    // Require Comment access on the parent doc (also 404s on a trashed doc).
    let _meta = super::documents::check_comment_access(&state, &thread.doc_id, &user_id).await?;

    if body.content.trim().is_empty() {
        return Err(ApiError::BadRequest("Message cannot be empty".to_string()));
    }

    // Find the message and verify the caller authored it.
    let messages = state.thread_repo.list_messages(&thread_id).await?;
    let msg = messages
        .iter()
        .find(|m| m.message_id == message_id)
        .ok_or(ApiError::NotFound("Message not found".to_string()))?;

    if msg.user_id != user_id {
        return Err(ApiError::Forbidden);
    }

    let now = now_usec();
    state
        .thread_repo
        .update_message(&thread_id, &msg.sk(), &body.content, &body.parts, &body.mentions, now)
        .await?;

    // Tell peers viewing this doc to refresh the thread so the edited text
    // and marker appear without a manual reload.
    let edited = Message {
        thread_id: thread_id.clone(),
        message_id: msg.message_id.clone(),
        user_id: msg.user_id.clone(),
        content: body.content.clone(),
        created_at: msg.created_at,
        updated_at: Some(now),
        parts: body.parts.clone(),
        mentions: body.mentions.clone(),
        attachments: msg.attachments.clone(),
    };
    fanout_comment_event(
        &state,
        &thread.doc_id,
        CommentEventPayload::MessageEdited {
            message: CommentEventMessage::from(&edited),
        },
    );

    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 6: Wire the route**

In `thread_router()`, change the message-id route to also accept PATCH:

```rust
        .route("/{thread_id}/messages/{message_id}", delete(delete_message).patch(edit_message))
```

(`patch` is already imported at the top of the file.)

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p ogrenotes-api --test test_comments edit_message edit_chat_message -- --nocapture`
Expected: PASS — all five new tests green.

- [ ] **Step 8: Run the full comment suite + storage tests for regressions**

Run: `cargo test -p ogrenotes-api --test test_comments && cargo test -p ogrenotes-storage thread`
Expected: PASS — existing comment behavior and the thread_repo unit tests are unaffected.

- [ ] **Step 9: Commit**

```bash
git add crates/storage/src/repo/thread_repo.rs crates/api/src/routes/comments.rs crates/api/tests/test_comments.rs
git commit -m "$(cat <<'EOF'
feat(comments): author-only edit endpoint + updatedAt

PATCH /threads/{id}/messages/{id} edits inline/document comment
messages (author-only, comments-only), sets Message.updated_at, and
broadcasts a MessageEdited event. Does not bump thread.updated_at or
notify. Chat/DM messages remain non-editable.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Frontend edit affordance + "edited" marker

Adds the client API call, the `updatedAt` field on the client DTO, and the popup UI: an author-only Edit button, inline edit textarea with Save/Cancel, and the "edited" marker (exact time on hover). Peers already refresh on any `CommentEvent` (`document.rs` bumps `comments_dirty`), so no new WS-callback wiring is needed.

**Files:**
- Modify: `frontend/src/api/comments.rs` (add `updated_at` to `MessageItem`; add `edit_message`)
- Modify: `frontend/src/components/comment_popup.rs` (extend `PopupMessage`; add a `to_popup` mapper; edit signals + UI + marker)
- Modify: `frontend/locales/en-US/main.ftl` (three strings; other locales fall back to en-US)
- Modify: `frontend/style/main.css` (marker + edit-affordance styles)

**Interfaces:**
- Consumes: `comments::list_messages`, `comments::MessageItem`, `crate::api::client::get_auth()` (returns `Option<_>` with `.user_id: String`), `crate::i18n::{format_date, DateStyle}`, `crate::t!`, `api_patch`.
- Produces: `comments::edit_message(thread_id: &str, message_id: &str, content: &str) -> Result<(), ApiClientError>`.

- [ ] **Step 1: Add the client DTO field + API call**

In `frontend/src/api/comments.rs`, add `updated_at` to `MessageItem` (after `content`):

```rust
    pub content: String,
    /// Present only when the message was edited; drives the "edited" marker.
    #[serde(default)]
    pub updated_at: Option<i64>,
}
```

Add the API call after `add_message`:

```rust
pub async fn edit_message(
    thread_id: &str,
    message_id: &str,
    content: &str,
) -> Result<(), ApiClientError> {
    api_patch(
        &format!("/threads/{thread_id}/messages/{message_id}"),
        &serde_json::json!({ "content": content }),
    )
    .await
}
```

- [ ] **Step 2: Add the i18n strings**

In `frontend/locales/en-US/main.ftl`, add next to the other `comment-*` keys (after `comment-placeholder-reply` at line ~218):

```
comment-edited = edited
comment-edit = Edit
comment-save = Save
```

(`common-cancel = Cancel` already exists and will be reused for the Cancel button.)

- [ ] **Step 3: Extend `PopupMessage` and add a shared mapper**

In `frontend/src/components/comment_popup.rs`, replace the `PopupMessage` struct (bottom of file) with:

```rust
#[derive(Clone)]
struct PopupMessage {
    message_id: String,
    user_id: String,
    user_name: String,
    content: String,
    created_at: i64,
    updated_at: Option<i64>,
}

/// Map a wire `MessageItem` into the popup's view model. Centralizes the
/// user_name fallback + field copy shared by the initial load, reply, and
/// edit reload paths.
fn to_popup(m: comments::MessageItem) -> PopupMessage {
    PopupMessage {
        message_id: m.message_id,
        user_id: m.user_id.clone(),
        user_name: if m.user_name.is_empty() { m.user_id } else { m.user_name },
        content: m.content,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}
```

- [ ] **Step 4: Route the two existing mapping sites through `to_popup`**

In the load-messages `Effect` (the `resp.messages.into_iter().map(|m| PopupMessage { … })` around line 108), replace the closure body with the mapper:

```rust
                    set_messages.set(resp.messages.into_iter().map(to_popup).collect());
```

Do the same in `send_reply` (the identical `map(|m| PopupMessage { … })` around line 153):

```rust
                    set_messages.set(resp.messages.into_iter().map(to_popup).collect());
```

- [ ] **Step 5: Add edit state + the current user id**

In `CommentPopup`, just after the existing `let (reply_text, …)` signal declarations (around line 55), add:

```rust
    let (editing_id, set_editing_id) = signal::<Option<String>>(None);
    let (edit_text, set_edit_text) = signal(String::new());
    // Author id for gating the Edit affordance. Stable for the session, so
    // read once; the server also enforces author-only, this is UX only.
    let current_uid = crate::api::client::get_auth()
        .map(|a| a.user_id)
        .unwrap_or_default();
```

- [ ] **Step 6: Render the edit affordance + marker**

In the `PopupMode::Thread(_)` body, replace the message-rendering closure (the `{move || messages.get().into_iter().map(|m| { view! { … } }).collect::<Vec<_>>()}` block around lines 258-268) with:

```rust
                                    {move || {
                                        let current_uid = current_uid.clone();
                                        let editing = editing_id.get();
                                        messages.get().into_iter().map(|m| {
                                            let is_author = m.user_id == current_uid;
                                            let is_editing = editing.as_deref() == Some(m.message_id.as_str());
                                            let mid = m.message_id.clone();

                                            if is_editing {
                                                let save = {
                                                    let mid = mid.clone();
                                                    move || {
                                                        let text = edit_text.get_untracked();
                                                        if text.trim().is_empty() { return; }
                                                        let PopupMode::Thread(tid) = mode.get_untracked() else { return };
                                                        let mid = mid.clone();
                                                        set_editing_id.set(None);
                                                        leptos::task::spawn_local(async move {
                                                            if comments::edit_message(&tid, &mid, &text).await.is_ok() {
                                                                if let Ok(resp) = comments::list_messages(&tid).await {
                                                                    set_messages.set(resp.messages.into_iter().map(to_popup).collect());
                                                                }
                                                            }
                                                        });
                                                    }
                                                };
                                                let save_click = save.clone();
                                                view! {
                                                    <div class="comment-popup-msg">
                                                        <div class="comment-popup-msg-header">
                                                            <span class="comment-popup-author">{m.user_name.clone()}</span>
                                                        </div>
                                                        <textarea
                                                            class="comment-popup-textarea"
                                                            prop:value=move || edit_text.get()
                                                            on:input=move |e| set_edit_text.set(event_target_value(&e))
                                                            on:keydown={
                                                                let save = save.clone();
                                                                move |e: web_sys::KeyboardEvent| {
                                                                    if e.key() == "Enter" && !e.shift_key() {
                                                                        e.prevent_default();
                                                                        save();
                                                                    }
                                                                }
                                                            }
                                                        ></textarea>
                                                        <div class="comment-edit-actions">
                                                            <button
                                                                class="comment-popup-send"
                                                                on:click=move |_| save_click()
                                                            >{crate::t!("comment-save")}</button>
                                                            <button
                                                                class="comment-edit-cancel"
                                                                on:click=move |_| set_editing_id.set(None)
                                                            >{crate::t!("common-cancel")}</button>
                                                        </div>
                                                    </div>
                                                }.into_any()
                                            } else {
                                                let edit_btn = is_author.then(|| {
                                                    let mid = mid.clone();
                                                    let content = m.content.clone();
                                                    view! {
                                                        <button
                                                            class="comment-edit-btn"
                                                            on:click=move |_| {
                                                                set_edit_text.set(content.clone());
                                                                set_editing_id.set(Some(mid.clone()));
                                                            }
                                                        >{crate::t!("comment-edit")}</button>
                                                    }
                                                });
                                                let edited_marker = m.updated_at.map(|ts| view! {
                                                    <span
                                                        class="comment-popup-edited"
                                                        title=crate::i18n::format_date(ts, crate::i18n::DateStyle::Long)
                                                    >{crate::t!("comment-edited")}</span>
                                                });
                                                view! {
                                                    <div class="comment-popup-msg">
                                                        <div class="comment-popup-msg-header">
                                                            <span class="comment-popup-author">{m.user_name.clone()}</span>
                                                            <span class="comment-popup-time">{format_relative(m.created_at)}</span>
                                                            {edited_marker}
                                                        </div>
                                                        <div class="comment-popup-text">{m.content.clone()}</div>
                                                        {edit_btn}
                                                    </div>
                                                }.into_any()
                                            }
                                        }).collect::<Vec<_>>()
                                    }}
```

- [ ] **Step 7: Add the CSS**

In `frontend/style/main.css`, immediately after the `.comment-popup-text { … }` rule (ends at line ~2148), insert:

```css
.comment-popup-edited {
  font-size: 11px;
  font-style: italic;
  color: var(--color-text-tertiary);
  cursor: default;
}

.comment-edit-btn,
.comment-edit-cancel {
  background: none;
  border: none;
  padding: 2px 4px;
  margin-top: 2px;
  font-size: 11px;
  color: var(--color-text-tertiary);
  cursor: pointer;
}

.comment-edit-btn:hover,
.comment-edit-cancel:hover {
  color: var(--color-text);
  text-decoration: underline;
}

.comment-edit-actions {
  display: flex;
  gap: 8px;
  margin-top: 6px;
}
```

- [ ] **Step 8: Typecheck the frontend**

Run: `cd frontend && cargo check`
Expected: compiles clean. (This change touches no `cfg(target_arch = "wasm32")` code, so a native `cargo check` fully typechecks it.)

- [ ] **Step 9: Manual smoke test**

Build/serve the frontend the way this repo does (e.g. `cd frontend && trunk serve`, or the deployed test stack). In a document:
1. Open a comment thread you authored → an **Edit** button shows on your own message; none shows on another user's message.
2. Click Edit → the text becomes a textarea prefilled with the current content; **Save** and **Cancel** appear.
3. Edit + Save (or press Enter) → text updates and an italic **edited** marker appears next to the timestamp; hovering it shows the exact edit time.
4. Cancel → leaves the message unchanged.
5. In a second browser/tab on the same doc, confirm the edit appears after the `CommentEvent` refresh.

- [ ] **Step 10: Commit**

```bash
git add frontend/src/api/comments.rs frontend/src/components/comment_popup.rs frontend/locales/en-US/main.ftl frontend/style/main.css
git commit -m "$(cat <<'EOF'
feat(comments): edit affordance + "edited" marker in comment popup

Author-only Edit button with inline Save/Cancel, and an italic "edited"
marker (exact time on hover) surfaced from the new updatedAt field.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the implementer

- **Order matters in `edit_message`:** the `thread_type` gate runs *before* `check_comment_access` so a chat thread's empty `doc_id` never reaches the access check. Keep it that way.
- **`created_at` is inside the SK.** `update_message` never touches PK/SK, so ordering and `created_at` are preserved automatically — that's what `test_edit_message_preserves_created_at_and_thread_updated_at` locks.
- **`MessageEdited` is only a refresh trigger.** The frontend reacts to *any* `CommentEvent` by bumping `comments_dirty` and refetching, so the payload's contents aren't parsed field-by-field — don't add frontend WS-callback code for it.
- **Slash commands are not re-run on edit** (unlike `add_message`). Editing is plain text; this is deliberate.
