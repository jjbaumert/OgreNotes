// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

// ─── Create chat ───────────────────────────────────────────────

#[tokio::test]
async fn test_create_group_chat() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;

    assert_eq!(status, 201);
    assert!(json["id"].is_string());

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_dm() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "directMessage",
        "memberIds": [bob_id]
    });
    let (status, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;

    assert_eq!(status, 201);
    assert!(json["id"].is_string());

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_dm_with_self() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;

    let body = serde_json::json!({
        "chatType": "directMessage",
        "memberIds": [alice_id]
    });
    let (status, _) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_group_no_title() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "memberIds": [bob_id]
    });
    let (status, _) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

// ─── List chats ────────────────────────────────────────────────

#[tokio::test]
async fn test_list_chats() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    // Create a group chat
    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    app.json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;

    let (status, json) = app
        .json_request(Method::GET, "/api/v1/chats", Some(&token_a), None)
        .await;

    assert_eq!(status, 200);
    let chats = json["chats"].as_array().unwrap();
    assert!(!chats.is_empty());

    app.cleanup().await;
}

/// Regression for the paginated `list_user_chats`: every chat the user is a
/// member of must come back, not just whatever fit in the first scanned
/// page. (Can't cheaply force a multi-page scan here without >1 MB of data,
/// so this pins the all-members-returned contract; the pagination loop
/// itself mirrors the proven `DynamoClient::query` loop.)
#[tokio::test]
async fn test_list_chats_returns_all_member_chats() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    const N: usize = 5;
    for i in 0..N {
        let body = serde_json::json!({
            "chatType": "chat",
            "title": format!("Room {i}"),
            "memberIds": [bob_id]
        });
        let (status, _) = app
            .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
            .await;
        assert_eq!(status, 201, "chat {i} should be created");
    }

    let (status, json) = app
        .json_request(Method::GET, "/api/v1/chats", Some(&token_a), None)
        .await;
    assert_eq!(status, 200);
    let chats = json["chats"].as_array().unwrap();
    assert_eq!(
        chats.len(),
        N,
        "all {N} chats the user is a member of must be listed, got {}",
        chats.len()
    );

    app.cleanup().await;
}

/// #34: the reverse-edge `list_user_chats` must surface a chat to a
/// member who is NOT the creator — proving `create_thread` writes an
/// edge for every member, not just the caller.
#[tokio::test]
async fn test_invited_member_sees_chat_in_own_list() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (status, created) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    assert_eq!(status, 201);
    let chat_id = created["id"].as_str().unwrap().to_string();

    // Bob, an invited (non-creator) member, must see the chat in his list.
    let (status, json) = app
        .json_request(Method::GET, "/api/v1/chats", Some(&token_b), None)
        .await;
    assert_eq!(status, 200);
    let ids: Vec<&str> = json["chats"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["id"].as_str())
        .collect();
    assert!(
        ids.contains(&chat_id.as_str()),
        "invited member must see the chat via its membership edge; got {ids:?}"
    );

    app.cleanup().await;
}

/// #49: `create_thread` writes a membership edge for *every* member the
/// loop iterates (not just the first) and still returns success. Two
/// non-creator members must both find the chat in their own lists —
/// exercising the multi-member path the best-effort edge loop walks.
#[tokio::test]
async fn test_create_group_chat_indexes_every_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let (carol_id, token_c) = app.create_user("carol@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id, carol_id]
    });
    let (status, created) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    assert_eq!(status, 201);
    let chat_id = created["id"].as_str().unwrap().to_string();

    // Every non-creator member must see the chat via their own edge.
    for (who, token) in [("bob", &token_b), ("carol", &token_c)] {
        let (status, json) = app
            .json_request(Method::GET, "/api/v1/chats", Some(token), None)
            .await;
        assert_eq!(status, 200);
        let ids: Vec<&str> = json["chats"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["id"].as_str())
            .collect();
        assert!(
            ids.contains(&chat_id.as_str()),
            "member {who} must see the chat via its membership edge; got {ids:?}"
        );
    }

    app.cleanup().await;
}

/// #34: adding a member writes their edge (the chat appears in their
/// list) and removing them deletes it (it disappears) — end to end
/// through the `/members` endpoints.
#[tokio::test]
async fn test_add_then_remove_member_updates_their_chat_list() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let (charlie_id, token_c) = app.create_user("charlie@test.com").await;

    // Alice creates a group chat with Bob; Charlie is not a member yet.
    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (status, created) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    assert_eq!(status, 201);
    let chat_id = created["id"].as_str().unwrap().to_string();

    // Helper: does Charlie's chat list contain `chat_id`?
    async fn charlie_has(app: &common::TestApp, token: &str, chat_id: &str) -> bool {
        let (status, json) = app
            .json_request(Method::GET, "/api/v1/chats", Some(token), None)
            .await;
        assert_eq!(status, 200);
        json["chats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"].as_str() == Some(chat_id))
    }

    assert!(
        !charlie_has(&app, &token_c, &chat_id).await,
        "charlie must not see the chat before he is added"
    );

    // Alice adds Charlie.
    let add = serde_json::json!({ "userId": charlie_id });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/members"),
            Some(&token_a),
            Some(add),
        )
        .await;
    assert_eq!(status, 204);
    assert!(
        charlie_has(&app, &token_c, &chat_id).await,
        "charlie must see the chat after being added (edge written)"
    );

    // Alice removes Charlie.
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/chats/{chat_id}/members/{charlie_id}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 204);
    assert!(
        !charlie_has(&app, &token_c, &chat_id).await,
        "charlie must not see the chat after removal (edge deleted)"
    );

    app.cleanup().await;
}

// ─── Get chat ──────────────────────────────────────────────────

#[tokio::test]
async fn test_get_chat_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap();

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}"),
            Some(&token_a),
            None,
        )
        .await;

    assert_eq!(status, 200);
    assert_eq!(json["id"], chat_id);
    assert_eq!(json["chatType"], "chat");

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_chat_non_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let (_, token_c) = app.create_user("charlie@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Private",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap();

    // Charlie is not a member
    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}"),
            Some(&token_c),
            None,
        )
        .await;

    assert_eq!(status, 403);

    app.cleanup().await;
}

// ─── Add member ────────────────────────────────────────────────

#[tokio::test]
async fn test_add_member_group() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let (charlie_id, _) = app.create_user("charlie@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap();

    let add_body = serde_json::json!({ "userId": charlie_id });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/members"),
            Some(&token_a),
            Some(add_body),
        )
        .await;

    assert_eq!(status, 204);

    app.cleanup().await;
}

#[tokio::test]
async fn test_add_member_dm_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let (charlie_id, _) = app.create_user("charlie@test.com").await;

    let body = serde_json::json!({
        "chatType": "directMessage",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap();

    let add_body = serde_json::json!({ "userId": charlie_id });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/members"),
            Some(&token_a),
            Some(add_body),
        )
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

// ─── Remove member ─────────────────────────────────────────────

#[tokio::test]
async fn test_remove_creator_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap();

    // Try to remove the creator
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/chats/{chat_id}/members/{alice_id}"),
            Some(&token_a),
            None,
        )
        .await;

    assert_eq!(status, 400);

    app.cleanup().await;
}

// ─── Messages ──────────────────────────────────────────────────

#[tokio::test]
async fn test_send_message() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap();

    let msg_body = serde_json::json!({ "content": "Hello" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_a),
            Some(msg_body),
        )
        .await;

    assert_eq!(status, 201);

    app.cleanup().await;
}

#[tokio::test]
async fn test_send_empty_message() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap();

    let msg_body = serde_json::json!({ "content": "" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_a),
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

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap();

    // Send two messages
    for text in &["Hello", "World"] {
        let msg_body = serde_json::json!({ "content": text });
        app.json_request(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_a),
            Some(msg_body),
        )
        .await;
    }

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_a),
            None,
        )
        .await;

    assert_eq!(status, 200);
    let messages = json["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);

    app.cleanup().await;
}

// ─── Read receipts ─────────────────────────────────────────────

#[tokio::test]
async fn test_chat_get_messages_records_read_receipt() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap().to_string();

    // Alice posts a message so Bob has something to read.
    app.json_request(
        Method::POST,
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&token_a),
        Some(serde_json::json!({ "content": "hi" })),
    )
    .await;

    // Both users GET messages, which should record their read receipt.
    for tok in [&token_a, &token_b] {
        app.json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(tok),
            None,
        )
        .await;
    }

    let (status, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let mut ids: Vec<&str> = msgs["readBy"]
        .as_array()
        .expect("chat list_messages should surface readBy")
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
async fn test_chat_list_exposes_read_by() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, json) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = json["id"].as_str().unwrap().to_string();

    // Alice reads once.
    app.json_request(
        Method::GET,
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&token_a),
        None,
    )
    .await;

    let (_, chats_json) = app
        .json_request(Method::GET, "/api/v1/chats", Some(&token_a), None)
        .await;

    let entry = chats_json["chats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_str() == Some(&chat_id))
        .unwrap();
    let read_by = entry["readBy"].as_array().expect("readBy on chat list");
    assert_eq!(read_by.len(), 1);
    assert_eq!(read_by[0]["userId"], alice_id);

    app.cleanup().await;
}

// ─── Slash commands ────────────────────────────────────────────

#[tokio::test]
async fn test_slash_invite_adds_chat_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app
        .create_user_with_name("alice@test.com", "Alice Creator")
        .await;
    let (bob_id, _) = app
        .create_user_with_name("bob@test.com", "Bob Member")
        .await;
    let (_, token_c) = app
        .create_user_with_name("carol@test.com", "Carol Invitee")
        .await;

    // Alice starts a group chat with just Bob initially.
    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, j) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = j["id"].as_str().unwrap().to_string();

    // Carol isn't a member yet — she shouldn't see the chat.
    let (pre_status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_c),
            None,
        )
        .await;
    assert_eq!(pre_status, 403);

    // Alice invites Carol via slash command.
    let body = serde_json::json!({ "content": "/invite @carol@test.com" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);

    // Carol can now read the chat. The announcement must reference names,
    // never the invitee's email (PII safety).
    let (post_status, msgs) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_c),
            None,
        )
        .await;
    assert_eq!(post_status, 200);
    let content = msgs["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("Alice Creator"), "expected caller name, got {content}");
    assert!(content.contains("Carol Invitee"), "expected invitee name, got {content}");
    assert!(
        !content.contains("carol@test.com"),
        "announcement must not leak invitee email, got {content}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn test_slash_invite_by_non_creator_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;
    let (_, _token_c) = app.create_user("carol@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, j) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = j["id"].as_str().unwrap().to_string();

    // Bob (member but not creator) tries to invite Carol.
    let body = serde_json::json!({ "content": "/invite @carol@test.com" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_b),
            Some(body),
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn test_slash_invite_in_dm_rejected() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, _) = app.create_user("bob@test.com").await;
    let (_, _token_c) = app.create_user("carol@test.com").await;

    let body = serde_json::json!({
        "chatType": "directMessage",
        "memberIds": [bob_id]
    });
    let (_, j) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let dm_id = j["id"].as_str().unwrap().to_string();

    let body = serde_json::json!({ "content": "/invite @carol@test.com" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/chats/{dm_id}/messages"),
            Some(&token_a),
            Some(body),
        )
        .await;
    assert_eq!(status, 400, "DMs should reject /invite");

    app.cleanup().await;
}

// ─── Access-gate defense-in-depth ─────────────────────────────
//
// Chat endpoints surface `readBy` — who has seen the thread and when.
// That data is only meaningful inside the thread and must never leak to
// non-members. These tests pin the gate so a future refactor that drops
// a membership check fails loudly.

async fn seed_chat_with_messages_and_receipts(
    app: &common::TestApp,
) -> (String, String, String) {
    // Alice and Bob co-own a chat and each read one message so
    // `readBy` has content the non-member is trying to see.
    let (_, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    let body = serde_json::json!({
        "chatType": "chat",
        "title": "Team",
        "memberIds": [bob_id]
    });
    let (_, j) = app
        .json_request(Method::POST, "/api/v1/chats", Some(&token_a), Some(body))
        .await;
    let chat_id = j["id"].as_str().unwrap().to_string();

    app.json_request(
        Method::POST,
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&token_a),
        Some(serde_json::json!({ "content": "hello" })),
    )
    .await;
    for tok in [&token_a, &token_b] {
        app.json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(tok),
            None,
        )
        .await;
    }

    (chat_id, token_a, token_b)
}

/// Regression: `GET /api/v1/chats` filters on member_ids, so a non-member
/// must not see a chat they don't belong to in their own list.
#[tokio::test]
async fn test_list_chats_hides_non_member_chats() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_chat_id, _token_a, _token_b) = seed_chat_with_messages_and_receipts(&app).await;
    let (_, token_c) = app.create_user("carol@test.com").await;

    let (status, json) = app
        .json_request(Method::GET, "/api/v1/chats", Some(&token_c), None)
        .await;
    assert_eq!(status, 200);

    let chats = json["chats"].as_array().expect("chats array");
    // Strong form: carol was never added to anything, so her list must
    // be empty. A weaker `!contains(chat_id)` check would silently pass
    // if a future regression auto-enrolled users in a default channel.
    assert_eq!(
        chats.len(),
        0,
        "carol should have no chats in her list, got {json}",
    );

    app.cleanup().await;
}

/// Regression: `GET /api/v1/chats/:id` must 403 for a non-member.
#[tokio::test]
async fn test_get_chat_denied_to_non_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (chat_id, _token_a, _token_b) = seed_chat_with_messages_and_receipts(&app).await;
    let (_, token_c) = app.create_user("carol@test.com").await;

    let (status, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}"),
            Some(&token_c),
            None,
        )
        .await;
    assert_eq!(status, 403);
    // The 403 body shape is the canonical ApiError envelope — asserting
    // the `error` field ensures the access gate produced the rejection
    // via the error path rather than via some unrelated 403 origin.
    assert_eq!(body["error"], "forbidden");

    app.cleanup().await;
}

/// Regression: `GET /api/v1/chats/:id/messages` must 403 for a non-member.
/// This is the endpoint that actually lists `readBy` alongside the
/// messages, so a regression to 200 here would be the biggest privacy
/// hole.
#[tokio::test]
async fn test_list_chat_messages_denied_to_non_member() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (chat_id, _token_a, _token_b) = seed_chat_with_messages_and_receipts(&app).await;
    let (_, token_c) = app.create_user("carol@test.com").await;

    let (status, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_c),
            None,
        )
        .await;
    assert_eq!(status, 403);
    assert_eq!(body["error"], "forbidden");

    app.cleanup().await;
}
