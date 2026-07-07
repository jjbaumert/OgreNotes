// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Reproduction harness for issue #20 ("Comments not being broadcast
//! to other documents").
//!
//! Pre-populates the in-memory `RoomRegistry` with two fake WebSocket
//! clients (channel-backed, no real socket), then exercises
//! `POST /threads` and asserts both clients received the
//! `MessageType::CommentEvent` frame on the local broadcast path.
//!
//! If this test passes, the backend fanout works and the bug lives
//! elsewhere (frontend signal, network, multi-instance Redis fanout).
//! If it fails, we have a deterministic backend repro.

mod common;

use std::time::Duration;

use hyper::Method;
use ogrenotes_collab::document::OgreDoc;
use tokio::sync::mpsc;

const MSG_COMMENT_EVENT: u8 = 0x06;

/// Decode the JSON body of a CommentEvent frame. Returns None if the
/// frame isn't a CommentEvent or the body isn't valid UTF-8 JSON.
fn comment_event_payload(frame: &[u8]) -> Option<serde_json::Value> {
    if frame.first() != Some(&MSG_COMMENT_EVENT) {
        return None;
    }
    serde_json::from_slice(&frame[1..]).ok()
}

/// Find the first CommentEvent payload whose `kind` matches.
fn find_event<'a>(
    frames: &'a [Vec<u8>],
    kind: &str,
) -> Option<serde_json::Value> {
    frames
        .iter()
        .filter_map(|f| comment_event_payload(f))
        .find(|v| v.get("kind").and_then(|k| k.as_str()) == Some(kind))
}

#[tokio::test]
async fn test_thread_create_broadcasts_comment_event_to_local_clients() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, alice_token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Broadcast Doc", None).await;

    // Simulate two browser tabs already connected via WebSocket: insert
    // two fake clients into the doc's room. We don't need a real socket
    // — the room broadcasts via mpsc senders that the WS task owns.
    let room = app
        .state
        .room_registry
        .get_or_insert(&doc_id, OgreDoc::new());
    let (tx_a, mut rx_a) = mpsc::unbounded_channel::<Vec<u8>>();
    let (tx_b, mut rx_b) = mpsc::unbounded_channel::<Vec<u8>>();
    let alice_cid = room.next_client_id();
    let bob_cid = room.next_client_id();
    room.add_client(alice_cid, alice_id.clone(), tx_a).await;
    room.add_client(bob_cid, "bob".to_string(), tx_b).await;

    // Alice creates a thread. The route handler fires
    // `fanout_comment_event` which spawns a task to broadcast the
    // CommentEvent frame to every client in the room.
    let body = serde_json::json!({
        "threadType": "document",
        "message": "Hello",
    });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&alice_token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201, "create thread should return 201");
    let thread_id = json["threadId"].as_str().unwrap().to_string();

    // The broadcast is `tokio::spawn`'d after the response; give it a
    // moment to land on the channel ends.
    let frame_a = wait_for_recv(&mut rx_a, 500).await;
    let frame_b = wait_for_recv(&mut rx_b, 500).await;

    let frame_a = frame_a.expect("alice (creator) should receive the CommentEvent broadcast");
    let frame_b = frame_b.expect("bob (peer) should receive the CommentEvent broadcast");

    // Payload-in-frame: the broadcast must carry the full thread snapshot
    // so peers can update their inline_threads signal without a follow-up
    // GET /threads. Both recipients see the same body.
    for (label, frame) in [("alice", &frame_a), ("bob", &frame_b)] {
        let payload = comment_event_payload(frame)
            .unwrap_or_else(|| panic!("{label}'s frame must be a CommentEvent with JSON body"));
        assert_eq!(
            payload["kind"], "thread_created",
            "{label}: expected kind=thread_created"
        );
        let thread = &payload["thread"];
        assert_eq!(
            thread["threadId"], thread_id,
            "{label}: thread.threadId must match the created thread"
        );
        assert_eq!(
            thread["threadType"], "document",
            "{label}: thread.threadType must echo the create request"
        );
        assert_eq!(thread["status"], "open", "{label}: new threads start open");
        assert_eq!(
            thread["docId"], doc_id,
            "{label}: thread.docId must point at the parent document"
        );
        assert!(
            thread["createdAt"].is_i64(),
            "{label}: thread.createdAt must be present so peers can sort"
        );
    }

    app.cleanup().await;
}

/// Drain up to one message from `rx` within `timeout_ms`, polling at
/// 10ms intervals. Returns Some(frame) if anything arrived, None on
/// timeout.
async fn wait_for_recv(
    rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    timeout_ms: u64,
) -> Option<Vec<u8>> {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if let Ok(frame) = rx.try_recv() {
            return Some(frame);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
}

/// Drain every message from `rx` after a brief settling window. The
/// fanout path may emit several frames per logical event (e.g. a
/// thread create + initial message, both broadcasting). Tests asserting
/// "the CommentEvent for the action arrived" should look for at least
/// one CommentEvent frame in the drained set.
async fn drain_after(rx: &mut mpsc::UnboundedReceiver<Vec<u8>>, settle_ms: u64) -> Vec<Vec<u8>> {
    tokio::time::sleep(Duration::from_millis(settle_ms)).await;
    let mut out = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        out.push(frame);
    }
    out
}

#[tokio::test]
async fn test_thread_status_change_broadcasts_comment_event() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, alice_token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Status Doc", None).await;

    // Create the thread BEFORE wiring fake clients so we don't have to
    // filter the create-thread frame out of the assertion.
    let body = serde_json::json!({ "threadType": "document", "message": "first" });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&alice_token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);
    let thread_id = json["threadId"].as_str().unwrap().to_string();

    let room = app
        .state
        .room_registry
        .get_or_insert(&doc_id, OgreDoc::new());
    let (tx_a, mut rx_a) = mpsc::unbounded_channel::<Vec<u8>>();
    let (tx_b, mut rx_b) = mpsc::unbounded_channel::<Vec<u8>>();
    room.add_client(room.next_client_id(), alice_id.clone(), tx_a)
        .await;
    room.add_client(room.next_client_id(), "bob".to_string(), tx_b)
        .await;

    // Resolve the thread.
    let body = serde_json::json!({ "status": "resolved" });
    let (status, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/threads/{thread_id}"),
            Some(&alice_token),
            Some(body),
        )
        .await;
    assert_eq!(status, 204);

    let a_frames = drain_after(&mut rx_a, 200).await;
    let b_frames = drain_after(&mut rx_b, 0).await;

    for (label, frames) in [("alice", &a_frames), ("bob", &b_frames)] {
        let payload = find_event(frames, "thread_status_changed")
            .unwrap_or_else(|| panic!("{label} should see a thread_status_changed event"));
        assert_eq!(
            payload["threadId"], thread_id,
            "{label}: payload.threadId must match the resolved thread"
        );
        assert_eq!(
            payload["status"], "resolved",
            "{label}: payload.status must reflect the new thread status"
        );
    }

    app.cleanup().await;
}

#[tokio::test]
async fn test_thread_reply_broadcasts_comment_event() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, alice_token) = app.create_user("alice@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Reply Doc", None).await;

    let body = serde_json::json!({ "threadType": "document", "message": "first" });
    let (_, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&alice_token),
            Some(body),
        )
        .await;
    let thread_id = json["threadId"].as_str().unwrap().to_string();

    let room = app
        .state
        .room_registry
        .get_or_insert(&doc_id, OgreDoc::new());
    let (tx_a, mut rx_a) = mpsc::unbounded_channel::<Vec<u8>>();
    let (tx_b, mut rx_b) = mpsc::unbounded_channel::<Vec<u8>>();
    room.add_client(room.next_client_id(), alice_id.clone(), tx_a)
        .await;
    room.add_client(room.next_client_id(), "bob".to_string(), tx_b)
        .await;

    // Reply on the thread.
    let body = serde_json::json!({ "content": "second" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&alice_token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);

    let a_frames = drain_after(&mut rx_a, 200).await;
    let b_frames = drain_after(&mut rx_b, 0).await;

    for (label, frames) in [("alice", &a_frames), ("bob", &b_frames)] {
        let payload = find_event(frames, "message_added")
            .unwrap_or_else(|| panic!("{label} should see a message_added event"));
        let message = &payload["message"];
        assert_eq!(
            message["threadId"], thread_id,
            "{label}: message.threadId must match the parent thread"
        );
        assert_eq!(
            message["content"], "second",
            "{label}: message.content must carry the reply body so peers don't refetch"
        );
        assert!(
            message["messageId"].as_str().is_some_and(|s| !s.is_empty()),
            "{label}: message.messageId must be a non-empty server-assigned id"
        );
        assert!(
            message["createdAt"].is_i64(),
            "{label}: message.createdAt must be present for ordering"
        );
    }

    app.cleanup().await;
}
