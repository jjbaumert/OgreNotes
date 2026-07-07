// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

// ─── Helpers ───────────────────────────────────────────────────

/// Share a folder with a user at the given access level and return the user's token.
async fn share_folder(
    app: &common::TestApp,
    owner_token: &str,
    folder_id: &str,
    target_id: &str,
    level: &str,
) {
    let body = serde_json::json!({ "userId": target_id, "accessLevel": level });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/folders/{folder_id}/members"),
            Some(owner_token),
            Some(body),
        )
        .await;
    assert_eq!(status, 204, "share_folder failed");
}

// ─── Create thread ─────────────────────────────────────────────

#[tokio::test]
async fn test_create_document_thread() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 201);
    assert!(json["threadId"].is_string());

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_inline_thread() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({
        "threadType": "inline",
        "blockId": "blk1",
        "anchorStart": 0,
        "anchorEnd": 5
    });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 201);
    assert!(json["threadId"].is_string());

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_thread_with_message() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({
        "threadType": "document",
        "message": "Hello"
    });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;

    assert_eq!(status, 201);
    let thread_id = json["threadId"].as_str().unwrap();

    // Verify the message was created
    let (msg_status, msg_json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(msg_status, 200);
    let messages = msg_json["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"], "Hello");

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_thread_view_only_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;

    // Share folder with Bob at VIEW level
    share_folder(&app, &token_a, &folder_id, &bob_id, "VIEW").await;

    let body = serde_json::json!({ "threadType": "document" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token_b),
            Some(body),
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_duplicate_block_comment() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({
        "threadType": "inline",
        "blockId": "blk2"
    });

    // First block comment succeeds
    let (status1, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body.clone()),
        )
        .await;
    assert_eq!(status1, 201);

    // Second block comment on the same blockId gets 409
    let (status2, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status2, 409);

    app.cleanup().await;
}

/// Spreadsheet cell-comments use a deterministic
/// `cell-s<sheet>r<row>c<col>` block_id so that two tabs proposing
/// the same cell get the same block_id. When the race loser gets
/// a 409 Conflict, the client must be able to discover the winner's
/// thread via `GET /documents/:doc_id/threads` and adopt it. This
/// test pins the contract that backs that adopt path: the winner's
/// thread is listable with its original block_id intact.
#[tokio::test]
async fn test_cell_block_id_listable_after_conflict() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({
        "threadType": "inline",
        "blockId": "cell-s0r9c3",
        "message": "first tab"
    });

    let (status1, json1) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body.clone()),
        )
        .await;
    assert_eq!(status1, 201);
    let winner_tid = json1["threadId"].as_str().unwrap().to_string();

    // Simultaneous attempt from the other tab — same block_id, gets 409.
    let conflict_body = serde_json::json!({
        "threadType": "inline",
        "blockId": "cell-s0r9c3",
        "message": "second tab"
    });
    let (status2, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(conflict_body),
        )
        .await;
    assert_eq!(status2, 409);

    // The client's adopt path is: list threads, find the one with the
    // requested block_id, use its thread_id. Pin the contract that
    // this discovery works.
    let (list_status, list_json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(list_status, 200);
    let threads = list_json["threads"].as_array().unwrap();
    let matches: Vec<_> = threads
        .iter()
        .filter(|t| t["blockId"].as_str() == Some("cell-s0r9c3"))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one thread for cell-s0r9c3, got {}",
        matches.len(),
    );
    assert_eq!(
        matches[0]["threadId"].as_str(),
        Some(winner_tid.as_str()),
        "list_threads must surface the race winner so the loser can adopt",
    );

    app.cleanup().await;
}

// ─── List threads ──────────────────────────────────────────────

#[tokio::test]
async fn test_list_threads() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    // Create two threads
    for i in 0..2 {
        let body = serde_json::json!({ "threadType": "document", "message": format!("Thread {i}") });
        app.json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    }

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 200);
    let threads = json["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);

    app.cleanup().await;
}

// ─── Update thread ─────────────────────────────────────────────

#[tokio::test]
async fn test_update_thread_resolve() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap();

    let patch_body = serde_json::json!({ "status": "resolved" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/threads/{thread_id}"),
            Some(&token),
            Some(patch_body),
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

// ─── Delete thread ─────────────────────────────────────────────

#[tokio::test]
async fn test_delete_thread_by_creator() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Alice owns the folder+doc, shares at COMMENT with Bob.
    // Bob creates a thread, then deletes it.
    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;
    share_folder(&app, &token_a, &folder_id, &bob_id, "COMMENT").await;

    // Bob creates a thread
    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token_b),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap();

    // Bob (creator, COMMENT access) deletes it
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/threads/{thread_id}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_thread_non_creator_comment_forbidden() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Alice owns the folder+doc, creates a thread.
    // Bob has COMMENT access, tries to delete Alice's thread.
    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;
    share_folder(&app, &token_a, &folder_id, &bob_id, "COMMENT").await;

    // Alice creates a thread
    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token_a),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap();

    // Bob (non-creator, COMMENT access) tries to delete
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/threads/{thread_id}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_thread_non_creator_edit_ok() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Alice owns the folder+doc, creates a thread.
    // Bob has EDIT access, can delete Alice's thread.
    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;
    share_folder(&app, &token_a, &folder_id, &bob_id, "EDIT").await;

    // Alice creates a thread
    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token_a),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap();

    // Bob (non-creator, EDIT access) deletes it
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/threads/{thread_id}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

// ─── Messages ──────────────────────────────────────────────────

#[tokio::test]
async fn test_add_message() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap();

    let msg_body = serde_json::json!({ "content": "Hi" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(msg_body),
        )
        .await;

    assert_eq!(status, 201);

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_empty_message_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap();

    let msg_body = serde_json::json!({ "content": "   " });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(msg_body),
        )
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_list_messages() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap();

    // Add two messages
    for text in &["First", "Second"] {
        let msg_body = serde_json::json!({ "content": text });
        app.json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(msg_body),
        )
        .await;
    }

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 200);
    let messages = json["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["userId"], alice_id);
    assert!(messages[0]["messageId"].is_string());
    assert!(messages[0]["content"].is_string());

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_message_author() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap();

    let msg_body = serde_json::json!({ "content": "Delete me" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/threads/{thread_id}/messages"),
        Some(&token),
        Some(msg_body),
    )
    .await;

    // Get the message ID
    let (_, msgs_json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    let msg_id = msgs_json["messages"][0]["messageId"].as_str().unwrap();

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_message_non_author() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;
    share_folder(&app, &token_a, &folder_id, &bob_id, "COMMENT").await;

    // Alice creates a thread and posts a message
    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token_a),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap();

    let msg_body = serde_json::json!({ "content": "Alice's message" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/threads/{thread_id}/messages"),
        Some(&token_a),
        Some(msg_body),
    )
    .await;

    // Get the message ID
    let (_, msgs_json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token_a),
            None,
        )
        .await;
    let msg_id = msgs_json["messages"][0]["messageId"].as_str().unwrap();

    // Bob tries to delete Alice's message
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}"),
            Some(&token_b),
            None,
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

/// Regression: thread.updatedAt must exactly equal the last message's createdAt.
/// Previously, add_message called now_usec() twice — once for the message, once for
/// bump_updated_at — producing a microsecond discrepancy.
#[tokio::test]
async fn test_message_timestamp_matches_thread_updated_at() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Timestamp Doc", None).await;

    // Create thread
    let body = serde_json::json!({ "threadType": "document" });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);
    let thread_id = json["threadId"].as_str().unwrap().to_string();

    // Add a message
    let msg_body = serde_json::json!({ "content": "Hello timestamp" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(msg_body),
        )
        .await;
    assert_eq!(status, 201);

    // Get the message's createdAt
    let (_, msg_json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    let messages = msg_json["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    let msg_created_at = messages[0]["createdAt"].as_i64().unwrap();

    // Get the thread's updatedAt
    let (_, thread_json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            None,
        )
        .await;
    let threads = thread_json["threads"].as_array().unwrap();
    let thread = threads.iter().find(|t| t["threadId"] == thread_id).unwrap();
    let thread_updated_at = thread["updatedAt"].as_i64().unwrap();

    // They must be exactly equal (both use the same `now` value)
    assert_eq!(
        msg_created_at, thread_updated_at,
        "Message createdAt ({msg_created_at}) must equal thread updatedAt ({thread_updated_at})"
    );

    app.cleanup().await;
}

/// Regression: message preview truncation must not panic on multi-byte characters.
/// Previously used byte-index slicing which panics on emoji/CJK text.
#[tokio::test]
async fn test_thread_preview_with_emoji() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Emoji Doc", None).await;

    // Create thread with initial message containing emoji (multi-byte chars)
    // Make it long enough to trigger the 120-char truncation
    let long_emoji_msg = "🎉".repeat(150); // 150 emoji = 600 bytes, >120 chars
    let body = serde_json::json!({
        "threadType": "document",
        "message": long_emoji_msg,
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);

    // List threads — this triggers the preview truncation. Must not panic.
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let threads = json["threads"].as_array().unwrap();
    assert!(!threads.is_empty());
    // Preview should be truncated with "..."
    let preview = threads[0]["firstMessage"].as_str().unwrap();
    assert!(preview.ends_with("..."), "Preview should be truncated: {preview}");

    app.cleanup().await;
}

// ─── Regression: no comment mutation on trashed doc ───────────
//
// Every write-path on comments goes through `check_doc_access` (strict,
// 404s on soft-deleted docs). These tests lock that invariant so a future
// refactor can't accidentally let a collaborator edit a trashed doc.

async fn trash_doc(app: &common::TestApp, token: &str, doc_id: &str) {
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}"),
            Some(token),
            None,
        )
        .await;
    assert_eq!(status, 204, "failed to trash doc");
}

#[tokio::test]
async fn test_create_thread_on_trashed_doc_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doomed", None).await;
    trash_doc(&app, &token, &doc_id).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_message_to_thread_on_trashed_doc_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    // Create thread while doc is live.
    let body = serde_json::json!({ "threadType": "document", "message": "Hi" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap().to_string();

    trash_doc(&app, &token, &doc_id).await;

    // Post should 404.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(serde_json::json!({ "content": "late" })),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_patch_thread_on_trashed_doc_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap().to_string();

    trash_doc(&app, &token, &doc_id).await;

    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/threads/{thread_id}"),
            Some(&token),
            Some(serde_json::json!({ "resolved": true })),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_delete_thread_on_trashed_doc_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap().to_string();

    trash_doc(&app, &token, &doc_id).await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/threads/{thread_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

// ─── Reactions ─────────────────────────────────────────────────

/// Set up a doc, a document-level thread, and one message. Returns
/// (thread_id, message_id). Used by every reaction test below.
async fn seed_thread_with_message(
    app: &common::TestApp,
    token: &str,
    doc_id: &str,
) -> (String, String) {
    let body = serde_json::json!({ "threadType": "document", "message": "hello" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap().to_string();

    let (_, msgs_json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(token),
            None,
        )
        .await;
    let msg_id = msgs_json["messages"][0]["messageId"]
        .as_str()
        .unwrap()
        .to_string();

    (thread_id, msg_id)
}

#[tokio::test]
async fn test_add_reaction_appears_in_message_list() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, msg_id) = seed_thread_with_message(&app, &token, &doc_id).await;

    let body = serde_json::json!({ "emoji": "👍" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}/reactions"),
            Some(&token),
            Some(body),
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
    let reactions = &msgs["messages"][0]["reactions"];
    assert_eq!(reactions[0]["emoji"], "👍");
    let user_ids: Vec<&str> = reactions[0]["userIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(user_ids, vec![alice_id.as_str()]);

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_reaction_is_idempotent() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, msg_id) = seed_thread_with_message(&app, &token, &doc_id).await;

    for _ in 0..3 {
        let body = serde_json::json!({ "emoji": "🎉" });
        let (status, _) = app
            .json_request(
                Method::POST,
                &format!("/api/v1/threads/{thread_id}/messages/{msg_id}/reactions"),
                Some(&token),
                Some(body),
            )
            .await;
        assert_eq!(status, 204);
    }

    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    let reactions = msgs["messages"][0]["reactions"].as_array().unwrap();
    assert_eq!(reactions.len(), 1, "duplicate reactions should not stack");
    let user_ids = reactions[0]["userIds"].as_array().unwrap();
    assert_eq!(user_ids.len(), 1, "alice should appear exactly once");
    assert_eq!(user_ids[0], alice_id);

    app.cleanup().await;
}

#[tokio::test]
async fn test_remove_reaction_drops_last_user() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, msg_id) = seed_thread_with_message(&app, &token, &doc_id).await;

    let body = serde_json::json!({ "emoji": "🔥" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/threads/{thread_id}/messages/{msg_id}/reactions"),
        Some(&token),
        Some(body),
    )
    .await;

    // URL-encode the emoji for the DELETE path.
    let encoded = urlencoding::encode("🔥").into_owned();
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}/reactions/{encoded}"),
            Some(&token),
            None,
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
    let reactions = msgs["messages"][0]["reactions"].as_array();
    // reactions field may be absent (serde skips empty Vec) or present and empty.
    assert!(
        reactions.map(|a| a.is_empty()).unwrap_or(true),
        "expected no reactions after last user removed, got {reactions:?}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_remove_reaction_when_not_reacted_is_noop() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, msg_id) = seed_thread_with_message(&app, &token, &doc_id).await;

    let encoded = urlencoding::encode("💯").into_owned();
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}/reactions/{encoded}"),
            Some(&token),
            None,
        )
        .await;
    // No error — DynamoDB DELETE on a non-existent set element is a no-op.
    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_view_only_user_cannot_react() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;
    share_folder(&app, &token_a, &folder_id, &bob_id, "VIEW").await;

    let (thread_id, msg_id) = seed_thread_with_message(&app, &token_a, &doc_id).await;

    let body = serde_json::json!({ "emoji": "👀" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}/reactions"),
            Some(&token_b),
            Some(body),
        )
        .await;
    assert!(status == 403 || status == 404, "expected 403/404, got {status}");

    app.cleanup().await;
}

#[tokio::test]
async fn test_react_to_missing_message_returns_404() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, _) = seed_thread_with_message(&app, &token, &doc_id).await;

    let body = serde_json::json!({ "emoji": "👍" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages/does-not-exist/reactions"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(
        status, 404,
        "reacting to a nonexistent message should 404 rather than silently planting an orphan row"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_chat_member_can_react() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    // Create a chat room that includes Bob.
    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap().to_string();

    // Post a message via the chat router.
    let body = serde_json::json!({ "content": "hey team" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&token_a),
        Some(body),
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

    // Bob reacts via the shared /threads endpoint. This regressed before the
    // member-list gate was added — the handler went down check_doc_access
    // with an empty doc_id and returned 404 for every chat reaction.
    let body = serde_json::json!({ "emoji": "👍" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{chat_id}/messages/{msg_id}/reactions"),
            Some(&token_b),
            Some(body),
        )
        .await;
    assert_eq!(status, 204, "chat members must be able to react");

    app.cleanup().await;
}

#[tokio::test]
async fn test_non_member_cannot_react_to_chat() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let (_, token_c) = app.create_user("carol@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap().to_string();

    let body = serde_json::json!({ "content": "hey team" });
    app.json_request(
        Method::POST,
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&token_a),
        Some(body),
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

    // Carol is not a member.
    let body = serde_json::json!({ "emoji": "👎" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{chat_id}/messages/{msg_id}/reactions"),
            Some(&token_c),
            Some(body),
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

// ─── Read receipts ─────────────────────────────────────────────

#[tokio::test]
async fn test_get_messages_records_read_receipt() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, _) = seed_thread_with_message(&app, &token, &doc_id).await;

    let (status, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);

    let read_by = msgs["readBy"].as_array().expect("readBy should be present");
    assert_eq!(read_by.len(), 1, "expected exactly one reader, got {read_by:?}");
    assert_eq!(read_by[0]["userId"], alice_id);
    assert!(read_by[0]["lastReadAt"].as_i64().unwrap() > 0);

    app.cleanup().await;
}

#[tokio::test]
async fn test_two_readers_both_in_read_by() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;
    share_folder(&app, &token_a, &folder_id, &bob_id, "COMMENT").await;

    let (thread_id, _) = seed_thread_with_message(&app, &token_a, &doc_id).await;

    for tok in [&token_a, &token_b] {
        app.json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(tok),
            None,
        )
        .await;
    }

    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token_a),
            None,
        )
        .await;
    let mut ids: Vec<&str> = msgs["readBy"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["userId"].as_str().unwrap())
        .collect();
    ids.sort();
    let mut expected = vec![alice_id.as_str(), bob_id.as_str()];
    expected.sort();
    assert_eq!(ids, expected);

    app.cleanup().await;
}

#[tokio::test]
async fn test_re_reading_moves_last_read_at_forward() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, _) = seed_thread_with_message(&app, &token, &doc_id).await;

    let (_, first) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    let ts1 = first["readBy"][0]["lastReadAt"].as_i64().unwrap();

    // Sleep past any plausible clock stall so ts2 is strictly greater than
    // ts1. The repo upsert gate is `last_read_at < :ts` (strict), so if
    // `now_usec()` returned the same value for both calls the second write
    // would be a no-op and the assertion below would fail.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let (_, second) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    let ts2 = second["readBy"][0]["lastReadAt"].as_i64().unwrap();

    assert!(ts2 > ts1, "second read should advance lastReadAt, got {ts1} -> {ts2}");
    assert_eq!(
        second["readBy"].as_array().unwrap().len(),
        1,
        "still only one reader"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_thread_list_includes_read_by() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, _) = seed_thread_with_message(&app, &token, &doc_id).await;

    // Trigger the read-receipt record.
    app.json_request(
        Method::GET,
        &format!("/api/v1/threads/{thread_id}/messages"),
        Some(&token),
        None,
    )
    .await;

    let (status, threads) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);

    let entry = threads["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["threadId"].as_str() == Some(&thread_id))
        .expect("thread should appear in list");
    let read_by = entry["readBy"].as_array().expect("readBy in thread list");
    assert_eq!(read_by.len(), 1);
    assert_eq!(read_by[0]["userId"], alice_id);

    app.cleanup().await;
}

#[tokio::test]
async fn test_two_users_reacting_same_emoji_groups_together() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;
    share_folder(&app, &token_a, &folder_id, &bob_id, "COMMENT").await;

    let (thread_id, msg_id) = seed_thread_with_message(&app, &token_a, &doc_id).await;

    for tok in [&token_a, &token_b] {
        let body = serde_json::json!({ "emoji": "❤️" });
        app.json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages/{msg_id}/reactions"),
            Some(tok),
            Some(body),
        )
        .await;
    }

    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token_a),
            None,
        )
        .await;
    let reactions = msgs["messages"][0]["reactions"].as_array().unwrap();
    assert_eq!(reactions.len(), 1, "same emoji should collapse into one group");
    assert_eq!(reactions[0]["emoji"], "❤️");
    let mut user_ids: Vec<&str> = reactions[0]["userIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    user_ids.sort();
    let mut expected = vec![alice_id.as_str(), bob_id.as_str()];
    expected.sort();
    assert_eq!(user_ids, expected);

    app.cleanup().await;
}

// ─── Slash commands ────────────────────────────────────────────

#[tokio::test]
async fn test_slash_invite_adds_doc_member_and_announces() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app
        .create_user_with_name("alice@test.com", "Alice Owner")
        .await;
    let (_bob_id, token_b) = app
        .create_user_with_name("bob@test.com", "Bob Invitee")
        .await;

    let doc_id = app.create_doc(&token_a, "Briefing", None).await;

    // Bob has no access prior to the invite.
    let (pre_status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert!(pre_status == 403 || pre_status == 404, "expected 403/404 pre-invite, got {pre_status}");

    let body = serde_json::json!({ "threadType": "document" });
    let (_, tj) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token_a),
            Some(body),
        )
        .await;
    let thread_id = tj["threadId"].as_str().unwrap().to_string();

    // Alice runs the slash command.
    let body = serde_json::json!({ "content": "/invite @bob@test.com" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);

    // Announcement must reference both names and never leak the invitee's
    // email — the thread history is visible to everyone with read access.
    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token_a),
            None,
        )
        .await;
    let content = msgs["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("Alice Owner"), "expected caller name, got {content}");
    assert!(content.contains("Bob Invitee"), "expected invitee name, got {content}");
    assert!(
        !content.contains("bob@test.com"),
        "announcement must not surface invitee email, got {content}"
    );

    // Bob can now open the doc.
    let (post_status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(post_status, 200, "bob should have access after /invite");

    app.cleanup().await;
}

/// Regression: the `/invite` write uses an exclusive put, so a second
/// call racing past the early check-then-write must return 409 rather than
/// silently producing a duplicate announcement.
#[tokio::test]
async fn test_slash_invite_duplicate_is_conflict() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app
        .create_user_with_name("alice@test.com", "Alice Owner")
        .await;
    let (_bob_id, _) = app
        .create_user_with_name("bob@test.com", "Bob Invitee")
        .await;
    let doc_id = app.create_doc(&token_a, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, tj) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token_a),
            Some(body),
        )
        .await;
    let thread_id = tj["threadId"].as_str().unwrap().to_string();

    // First invite succeeds.
    let body = serde_json::json!({ "content": "/invite @bob@test.com" });
    let (status1, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status1, 201);

    // Second invite for the same user must be rejected as a conflict.
    let body = serde_json::json!({ "content": "/invite @bob@test.com" });
    let (status2, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status2, 409);

    // Thread should still have exactly one announcement.
    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token_a),
            None,
        )
        .await;
    let invite_count = msgs["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| {
            m["content"]
                .as_str()
                .map(|s| s.contains("invited"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(invite_count, 1, "expected exactly one invite announcement");

    app.cleanup().await;
}

#[tokio::test]
async fn test_slash_invite_without_share_permission_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let (_, _token_c) = app.create_user("carol@test.com").await;

    let folder_id = app.create_folder(&token_a, "Shared", None).await;
    let doc_id = app.create_doc(&token_a, "Doc", Some(&folder_id)).await;
    // Bob has Edit on the folder — enough to comment, not enough to share.
    share_folder(&app, &token_a, &folder_id, &bob_id, "EDIT").await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, tj) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token_a),
            Some(body),
        )
        .await;
    let thread_id = tj["threadId"].as_str().unwrap().to_string();

    // Bob tries to invite Carol — should fail because only Own can share.
    let body = serde_json::json!({ "content": "/invite @carol@test.com" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token_b),
            Some(body),
        )
        .await;
    assert!(
        status == 403 || status == 404,
        "expected 403/404 for non-owner invite, got {status}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_slash_invite_unknown_user_returns_404() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, tj) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = tj["threadId"].as_str().unwrap().to_string();

    let body = serde_json::json!({ "content": "/invite @nobody@nowhere.test" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

/// `/shrug who knows` round-trips as a body-styled message with the
/// kaomoji appended. Replaces the old `/giphy` integration that depended
/// on a third-party API.
#[tokio::test]
async fn test_slash_shrug_appends_kaomoji() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let body = serde_json::json!({ "threadType": "document" });
    let (_, tj) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = tj["threadId"].as_str().unwrap().to_string();

    let body = serde_json::json!({ "content": "/shrug who knows" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);

    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    let content = msgs["messages"][0]["content"].as_str().unwrap();
    assert_eq!(content, "who knows ¯\\_(ツ)_/¯");
    let style = msgs["messages"][0]["parts"][0]["style"].as_str().unwrap();
    assert_eq!(style, "body", "kaomoji is the user's own line, not a system announcement");

    app.cleanup().await;
}

/// `/me <action>` rewrites the message to "{actor_name} {action}" and
/// stores it with `PartStyle::System` so rich clients render it as an
/// emote rather than a regular reply.
#[tokio::test]
async fn test_slash_me_renders_emote_with_system_style() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app
        .create_user_with_name("alice@test.com", "Alice Owner")
        .await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let body = serde_json::json!({ "threadType": "document" });
    let (_, tj) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = tj["threadId"].as_str().unwrap().to_string();

    let body = serde_json::json!({ "content": "/me waves hello" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);

    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    let content = msgs["messages"][0]["content"].as_str().unwrap();
    assert_eq!(content, "Alice Owner waves hello");
    let style = msgs["messages"][0]["parts"][0]["style"].as_str().unwrap();
    assert_eq!(style, "system", "/me must use System styling");

    app.cleanup().await;
}

#[tokio::test]
async fn test_rich_message_roundtrips_parts_mentions_attachments() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, tj) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = tj["threadId"].as_str().unwrap().to_string();

    // Post a message carrying all three rich fields.
    let body = serde_json::json!({
        "content": "see @bob",
        "parts": [
            { "style": "body", "text": "see " },
            { "style": "monospace", "text": "@bob" }
        ],
        "mentions": [
            { "mentionType": "person", "id": "u-bob", "label": "@bob" }
        ],
        "attachments": ["blob-123", "blob-456"]
    });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);

    let (_, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            None,
        )
        .await;
    let msg = &msgs["messages"][0];
    assert_eq!(msg["content"], "see @bob");

    let parts = msg["parts"].as_array().expect("parts should round-trip");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["style"], "body");
    assert_eq!(parts[1]["style"], "monospace");
    assert_eq!(parts[1]["text"], "@bob");

    let mentions = msg["mentions"].as_array().expect("mentions should round-trip");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0]["id"], "u-bob");
    assert_eq!(mentions[0]["mentionType"], "person");

    let atts: Vec<&str> = msg["attachments"]
        .as_array()
        .expect("attachments should round-trip")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(atts, vec!["blob-123", "blob-456"]);

    app.cleanup().await;
}

#[tokio::test]
async fn test_non_slash_messages_unaffected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;
    let (thread_id, _) = seed_thread_with_message(&app, &token, &doc_id).await;

    // Plain text containing a slash mid-sentence must not trigger parsing.
    let body = serde_json::json!({ "content": "TODO: / revisit this" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);

    app.cleanup().await;
}
