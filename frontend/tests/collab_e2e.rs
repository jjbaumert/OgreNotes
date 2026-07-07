// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! End-to-end collaboration tests.
//!
//! These tests require a running backend server at localhost:3000 with DEV_MODE=true.
//! Run with: wasm-pack test --headless --firefox -- --test collab_e2e
//!
//! The tests create two WebSocket connections to the same document and verify
//! that edits from one client appear on the other.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::*;
use yrs::{Transact, Text, ReadTxn, WriteTxn};
use yrs::updates::encoder::Encode;

wasm_bindgen_test_configure!(run_in_browser);

const API_BASE: &str = "http://localhost:3000/api/v1";
const WS_BASE: &str = "ws://localhost:3000/api/v1";

/// Helper: dev-login and return (access_token, user_id).
async fn dev_login(email: &str, name: &str) -> Result<(String, String), String> {
    let body = serde_json::json!({ "email": email, "name": name });
    let resp = gloo_net::http::Request::post(&format!("{API_BASE}/auth/dev-login"))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&body).unwrap())
        .map_err(|e| format!("request build: {e}"))?
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;

    if !resp.ok() {
        return Err(format!("login failed: HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    let token = json["accessToken"].as_str().ok_or("no accessToken")?.to_string();
    let user_id = json["userId"].as_str().ok_or("no userId")?.to_string();
    Ok((token, user_id))
}

/// Helper: create a document and return (doc_id).
async fn create_doc(token: &str) -> Result<String, String> {
    let body = serde_json::json!({ "title": "E2E Test Doc" });
    let resp = gloo_net::http::Request::post(&format!("{API_BASE}/documents"))
        .header("Content-Type", "application/json")
        .header("Authorization", &format!("Bearer {token}"))
        .body(serde_json::to_string(&body).unwrap())
        .map_err(|e| format!("request build: {e}"))?
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;

    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("create doc failed: HTTP {} — {text}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    let id = json["id"].as_str().ok_or("no id")?.to_string();
    Ok(id)
}

/// Helper: request a WS token for a document.
async fn get_ws_token(token: &str, doc_id: &str) -> Result<String, String> {
    let resp = gloo_net::http::Request::post(&format!("{API_BASE}/documents/{doc_id}/ws-token"))
        .header("Authorization", &format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;

    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("ws-token failed: HTTP {} — {text}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    let ws_token = json["token"].as_str().ok_or("no token")?.to_string();
    Ok(ws_token)
}

/// Helper: open a WebSocket connection and collect received messages.
/// Returns a handle that can be used to send messages and read received ones.
struct WsHandle {
    ws: web_sys::WebSocket,
    received: std::rc::Rc<std::cell::RefCell<Vec<Vec<u8>>>>,
    _closures: Vec<Closure<dyn Fn(web_sys::Event)>>,
}

impl WsHandle {
    async fn connect(doc_id: &str, ws_token: &str) -> Result<Self, String> {
        let url = format!("{WS_BASE}/documents/{doc_id}/ws?token={ws_token}");
        let ws = web_sys::WebSocket::new(&url)
            .map_err(|e| format!("WebSocket::new failed: {e:?}"))?;
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        let received = std::rc::Rc::new(std::cell::RefCell::new(Vec::<Vec<u8>>::new()));
        let mut closures = Vec::new();

        // Wait for open
        let (open_tx, open_rx) = futures_channel::oneshot::channel::<()>();
        let open_tx = std::rc::Rc::new(std::cell::RefCell::new(Some(open_tx)));
        let on_open = Closure::wrap(Box::new(move |_: web_sys::Event| {
            if let Some(tx) = open_tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        }) as Box<dyn Fn(web_sys::Event)>);
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        closures.push(on_open);

        // Collect messages
        let recv = received.clone();
        let on_message = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(me) = event.dyn_ref::<web_sys::MessageEvent>() else { return };
            let Ok(buf) = me.data().dyn_into::<js_sys::ArrayBuffer>() else { return };
            let array = js_sys::Uint8Array::new(&buf);
            recv.borrow_mut().push(array.to_vec());
        }) as Box<dyn Fn(web_sys::Event)>);
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        closures.push(on_message);

        // Error handler
        let on_error = Closure::wrap(Box::new(move |_: web_sys::Event| {
            web_sys::console::error_1(&"WS error in test".into());
        }) as Box<dyn Fn(web_sys::Event)>);
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        closures.push(on_error);

        // Wait for connection to open (timeout 5s)
        let timeout = gloo_timers::future::TimeoutFuture::new(5_000);
        futures_util::future::select(
            Box::pin(open_rx),
            Box::pin(timeout),
        ).await;

        if ws.ready_state() != web_sys::WebSocket::OPEN {
            return Err(format!("WebSocket failed to open, state={}", ws.ready_state()));
        }

        Ok(Self { ws, received, _closures: closures })
    }

    fn send_binary(&self, data: &[u8]) -> Result<(), String> {
        self.ws.send_with_u8_array(data)
            .map_err(|e| format!("send failed: {e:?}"))
    }

    fn messages(&self) -> Vec<Vec<u8>> {
        self.received.borrow().clone()
    }

    fn close(&self) {
        let _ = self.ws.close();
    }
}

/// Test 1: WebSocket connects and stays connected.
#[wasm_bindgen_test]
async fn test_ws_connects_and_stays_open() {
    let (token, _) = dev_login("e2e-alice@test.com", "Alice E2E").await
        .expect("dev login failed");
    let doc_id = create_doc(&token).await.expect("create doc failed");
    let ws_token = get_ws_token(&token, &doc_id).await.expect("ws token failed");

    let handle = WsHandle::connect(&doc_id, &ws_token).await
        .expect("WS connect failed");

    assert_eq!(handle.ws.ready_state(), web_sys::WebSocket::OPEN,
        "WebSocket should be OPEN");

    // Wait 2 seconds — should still be open.
    gloo_timers::future::TimeoutFuture::new(2_000).await;

    assert_eq!(handle.ws.ready_state(), web_sys::WebSocket::OPEN,
        "WebSocket should still be OPEN after 2 seconds");

    // Should have received at least a SyncStep1 from the server.
    let msgs = handle.messages();
    assert!(!msgs.is_empty(),
        "Should have received at least one message (SyncStep1)");

    // First message should be SyncStep1 (type byte 0x01).
    assert_eq!(msgs[0][0], 0x01,
        "First message should be SyncStep1 (0x01), got 0x{:02x}", msgs[0][0]);

    handle.close();
}

/// Test 2: Two clients sync — update from A reaches B.
#[wasm_bindgen_test]
async fn test_two_clients_sync_update() {
    // Login as two users
    let (token_a, _) = dev_login("e2e-sync-a@test.com", "Sync A").await
        .expect("login A failed");
    let (token_b, _) = dev_login("e2e-sync-b@test.com", "Sync B").await
        .expect("login B failed");

    // A creates a doc, shares it with B's folder (skip for now — both can access via owner)
    // For this test, A creates and both connect. B can connect because the WS endpoint
    // checks the token, not folder membership.
    let doc_id = create_doc(&token_a).await.expect("create doc failed");

    // Get WS tokens
    let ws_token_a = get_ws_token(&token_a, &doc_id).await.expect("ws token A failed");
    let ws_token_b = get_ws_token(&token_a, &doc_id).await.expect("ws token B failed");

    // Connect both
    let client_a = WsHandle::connect(&doc_id, &ws_token_a).await
        .expect("client A connect failed");
    let client_b = WsHandle::connect(&doc_id, &ws_token_b).await
        .expect("client B connect failed");

    // Wait for initial sync to complete
    gloo_timers::future::TimeoutFuture::new(500).await;

    // Client A sends a yrs Update message.
    // Create a simple yrs doc with some content.
    let update_bytes = {
        let doc = yrs::Doc::new();
        let mut txn = doc.transact_mut();
        let text = txn.get_or_insert_text("test");
        text.insert(&mut txn, 0, "Hello from A");
        drop(txn);

        let txn = doc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    };

    // Send as MSG_UPDATE (0x03)
    let mut msg = vec![0x03]; // MSG_UPDATE
    msg.extend_from_slice(&update_bytes);
    client_a.send_binary(&msg).expect("send update failed");

    // Wait for B to receive the update
    gloo_timers::future::TimeoutFuture::new(500).await;

    // Check that client B received an Update message (0x03)
    let b_msgs = client_b.messages();
    let update_msgs: Vec<_> = b_msgs.iter()
        .filter(|m| !m.is_empty() && m[0] == 0x03)
        .collect();

    assert!(!update_msgs.is_empty(),
        "Client B should have received at least one Update message. \
         Total messages: {}, types: {:?}",
        b_msgs.len(),
        b_msgs.iter().map(|m| m.first().copied()).collect::<Vec<_>>());

    client_a.close();
    client_b.close();
}

/// Helper: create a document-attached comment thread with an initial message.
/// Returns the new thread_id.
async fn create_thread(token: &str, doc_id: &str, message: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "threadType": "document",
        "message": message,
    });
    let resp = gloo_net::http::Request::post(&format!("{API_BASE}/documents/{doc_id}/threads"))
        .header("Content-Type", "application/json")
        .header("Authorization", &format!("Bearer {token}"))
        .body(serde_json::to_string(&body).unwrap())
        .map_err(|e| format!("request build: {e}"))?
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;

    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("create thread failed: HTTP {} — {text}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    let id = json["threadId"].as_str().ok_or("no threadId")?.to_string();
    Ok(id)
}

/// Helper: POST /threads/:id/messages — add a reply to a thread.
async fn add_message(token: &str, thread_id: &str, content: &str) -> Result<(), String> {
    let body = serde_json::json!({ "content": content });
    let resp = gloo_net::http::Request::post(&format!("{API_BASE}/threads/{thread_id}/messages"))
        .header("Content-Type", "application/json")
        .header("Authorization", &format!("Bearer {token}"))
        .body(serde_json::to_string(&body).unwrap())
        .map_err(|e| format!("request build: {e}"))?
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;

    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("add message failed: HTTP {} — {text}", resp.status()));
    }
    Ok(())
}

/// Drain `handle.messages()` for the first CommentEvent (type byte 0x06)
/// frame whose decoded JSON has `kind == expected_kind`. Polls every 50ms
/// up to `timeout_ms`. Returns the parsed JSON or an error describing what
/// arrived instead.
async fn wait_for_comment_event(
    handle: &WsHandle,
    expected_kind: &str,
    timeout_ms: u32,
) -> Result<serde_json::Value, String> {
    let deadline_steps = (timeout_ms / 50).max(1);
    let mut seen_kinds: Vec<String> = Vec::new();
    for _ in 0..deadline_steps {
        for frame in handle.messages() {
            if frame.first() != Some(&0x06) {
                continue;
            }
            let body = &frame[1..];
            match serde_json::from_slice::<serde_json::Value>(body) {
                Ok(v) => {
                    let k = v.get("kind").and_then(|k| k.as_str()).unwrap_or("").to_string();
                    if k == expected_kind {
                        return Ok(v);
                    }
                    if !seen_kinds.contains(&k) {
                        seen_kinds.push(k);
                    }
                }
                Err(e) => return Err(format!("CommentEvent body not JSON: {e}")),
            }
        }
        gloo_timers::future::TimeoutFuture::new(50).await;
    }
    Err(format!(
        "no CommentEvent with kind={expected_kind} received within {timeout_ms}ms \
         (saw kinds: {seen_kinds:?})"
    ))
}

/// Test 3: A reply posted via REST broadcasts a `message_added` CommentEvent
/// frame to every WebSocket peer on the same document, with the full message
/// snapshot embedded.
///
/// This is the regression test for the bug where a peer's typed reply did
/// not show up in another browser's open comment dialog: the WS broadcast
/// is the wire-level signal that drives the frontend refetch. If this test
/// breaks, every two-browser comment-sync scenario also breaks.
#[wasm_bindgen_test]
async fn test_reply_broadcasts_comment_event_to_peers() {
    let (token, _) = dev_login("e2e-comment-reply@test.com", "Comment Reply E2E").await
        .expect("dev login failed");
    let doc_id = create_doc(&token).await.expect("create doc failed");

    // Pre-seed a thread via REST so both peers have something to reply into.
    let thread_id = create_thread(&token, &doc_id, "first").await
        .expect("create thread failed");

    // Two WS peers on the same doc (same user; the room broadcasts to all
    // clients regardless of identity, so this is enough to pin the path).
    let ws_token_a = get_ws_token(&token, &doc_id).await.expect("ws token A failed");
    let ws_token_b = get_ws_token(&token, &doc_id).await.expect("ws token B failed");

    let peer_a = WsHandle::connect(&doc_id, &ws_token_a).await
        .expect("peer A connect failed");
    let peer_b = WsHandle::connect(&doc_id, &ws_token_b).await
        .expect("peer B connect failed");

    // Let initial sync settle before clearing the message cursor; otherwise
    // SyncStep1/2 frames sit at the head of `messages()` and the helper has
    // to skip past them. They're harmless (different type byte) but the
    // intent is cleaner if the comment frames are the only late arrivals.
    gloo_timers::future::TimeoutFuture::new(500).await;

    // Trigger the reply. Backend fanout fires from add_message → broadcast.
    let reply_body = "second-reply-from-rest";
    add_message(&token, &thread_id, reply_body).await
        .expect("add message failed");

    // Both peers must observe the message_added event within 2s. The frame
    // body must be the typed CommentEventPayload, with the reply content
    // inlined so a frontend that *did* parse the payload could update its
    // dialog without an extra GET /messages.
    let payload_a = wait_for_comment_event(&peer_a, "message_added", 2_000).await
        .expect("peer A: no message_added CommentEvent");
    let payload_b = wait_for_comment_event(&peer_b, "message_added", 2_000).await
        .expect("peer B: no message_added CommentEvent");

    for (label, payload) in [("peer A", &payload_a), ("peer B", &payload_b)] {
        let msg = &payload["message"];
        assert_eq!(
            msg["threadId"].as_str(), Some(thread_id.as_str()),
            "{label}: message.threadId must match the parent thread"
        );
        assert_eq!(
            msg["content"].as_str(), Some(reply_body),
            "{label}: message.content must be the reply body so peers don't refetch"
        );
        assert!(
            msg["messageId"].as_str().is_some_and(|s| !s.is_empty()),
            "{label}: message.messageId must be a non-empty server-assigned id"
        );
    }

    peer_a.close();
    peer_b.close();
}

#[wasm_bindgen_test]
async fn test_ws_no_immediate_disconnect() {
    let (token, _) = dev_login("e2e-stable@test.com", "Stable E2E").await
        .expect("dev login failed");
    let doc_id = create_doc(&token).await.expect("create doc failed");
    let ws_token = get_ws_token(&token, &doc_id).await.expect("ws token failed");

    let handle = WsHandle::connect(&doc_id, &ws_token).await
        .expect("WS connect failed");

    // Check state at 100ms intervals for 3 seconds
    for i in 0..30 {
        gloo_timers::future::TimeoutFuture::new(100).await;
        let state = handle.ws.ready_state();
        assert!(
            state == web_sys::WebSocket::OPEN || state == web_sys::WebSocket::CONNECTING,
            "WebSocket disconnected at {}ms (state={})", (i + 1) * 100, state
        );
    }

    handle.close();
}
