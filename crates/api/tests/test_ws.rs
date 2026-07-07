// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

#[tokio::test]
async fn test_create_ws_token() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("wstoken@test.com").await;
    let doc_id = app.create_doc(&token, "WS Doc", None).await;

    let body = serde_json::json!({});
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/ws-token"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 200);
    assert!(json["token"].is_string());

    app.cleanup().await;
}

#[tokio::test]
async fn test_ws_token_unauthenticated() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("tempws@test.com").await;
    let doc_id = app.create_doc(&token, "Unauth WS Doc", None).await;

    let body = serde_json::json!({});
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/ws-token"),
            None,
            Some(body),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_ws_token_nonexistent_doc() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("wsbaddoc@test.com").await;

    let body = serde_json::json!({});
    let (status, _) = app
        .json_request(
            Method::POST,
            "/api/v1/documents/nonexistent-doc-id-12345/ws-token",
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn test_create_ws_token_with_version() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("wsver@test.com").await;
    let doc_id = app.create_doc(&token, "WS Version Doc", None).await;

    let body = serde_json::json!({ "clientVersion": "1.2.3" });
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/ws-token"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 200);
    assert!(json["token"].is_string());

    app.cleanup().await;
}

// Note: ws_upgrade handler tests require a real TCP WebSocket connection
// because axum's WebSocketUpgrade extractor returns 426 in oneshot mode.
// Testing the upgrade validation paths requires tokio-tungstenite (future work).
//
// The Origin-check tests below DO work in oneshot mode because the Origin
// extractor runs before WebSocketUpgrade and rejects with 403 when the
// Origin header doesn't match — the request never reaches the upgrade
// extractor that would otherwise need a real socket. (Issue #31.)

#[tokio::test]
async fn ws_upgrade_tolerates_cross_site_origin_in_dev_mode() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // TestApp runs with dev_mode=true. Under the dev-mode policy
    // (`validate_ws_origin`), any Origin clears the gate — symmetric
    // with the dev-mode CORS policy (`AllowOrigin::mirror_request()`)
    // which also accepts any Origin. This is necessary for tooling
    // like `wasm-pack test --headless` whose page server picks a
    // random localhost port we can't pre-register. Strict-equality
    // rejection in production is asserted by the unit test
    // `routes::ws::tests::mismatched_origin_rejected` (dev_mode=false).
    //
    // With the Origin gate cleared, the fake token still misses the
    // Redis GETDEL lookup and `validate_ws_token` returns None →
    // `ws_upgrade` maps that to 401. That's what we assert here so the
    // test fails loudly if dev_mode ever stops being permissive (we'd
    // see 403) or if the handler short-circuits into a 5xx.
    let req = hyper::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/documents/any-doc-id/ws?token=fake-token")
        .header("Origin", "https://attacker.example")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = app.raw_request(req).await;
    assert_eq!(
        status, 401,
        "dev_mode tolerates any Origin; fake token must yield 401 from validate_ws_token"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn ws_upgrade_passes_origin_check_with_matching_origin() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // FRONTEND_ORIGIN in TestApp is http://localhost:8080.
    let req = hyper::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/documents/any-doc-id/ws?token=fake-token")
        .header("Origin", "http://localhost:8080")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = app.raw_request(req).await;
    // Origin check passes; with Redis up, the fake token deterministically
    // misses the GETDEL lookup and validate_ws_token returns None, which
    // ws_upgrade maps to ApiError::Unauthorized (401). Asserting 401
    // specifically — rather than just `!= 403` — ensures this test fails
    // loudly if the Origin gate ever short-circuits into a 5xx, instead
    // of silently passing the way `assert_ne!(403)` would.
    assert_eq!(
        status, 401,
        "matching Origin clears the gate; fake token must yield 401 from validate_ws_token"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn ws_upgrade_allows_missing_origin_in_dev_mode() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // No Origin header — TestApp runs with dev_mode=true, so this is OK.
    let req = hyper::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/documents/any-doc-id/ws?token=fake-token")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = app.raw_request(req).await;
    assert_ne!(
        status, 403,
        "missing Origin in dev_mode is allowed (non-browser clients)"
    );

    app.cleanup().await;
}

// ─── Connection caps (issue #34) ─────────────────────────────────
//
// Verify the per-document and per-user-per-document caps fire at
// HTTP layer (429 with Retry-After) BEFORE the upgrade is accepted.
//
// We simulate "at cap" by pre-populating the room with synthetic
// clients via room.add_client() — this skips actual WebSocket
// machinery and lets the test drive the ws_upgrade extractor
// deterministically. TestApp config sets max_ws_connections_per_doc=3
// and max_ws_connections_per_user_per_doc=2.

async fn populate_room_with_clients(
    app: &common::TestApp,
    doc_id: &str,
    by_user: &[(&str, usize)],
) {
    let room = app
        .state
        .room_registry
        .get_or_insert(doc_id, ogrenotes_collab::document::OgreDoc::new());
    for (uid, n) in by_user {
        for _ in 0..*n {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            // Hold rx alive via leak so the channel doesn't close.
            // Without this the sender side errors on subsequent broadcasts
            // (harmless to this test, but noisy).
            std::mem::forget(rx);
            let cid = room.next_client_id();
            room.add_client(cid, uid.to_string(), tx).await;
        }
    }
}

async fn fetch_ws_token(app: &common::TestApp, token: &str, doc_id: &str) -> String {
    let body = serde_json::json!({});
    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/ws-token"),
            Some(token),
            Some(body),
        )
        .await;
    assert_eq!(status, 200);
    json["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn ws_upgrade_rejects_when_per_document_cap_reached() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("ws-cap-doc@test.com").await;
    let doc_id = app.create_doc(&token, "Crowded room", None).await;

    // Cap is 3. Pre-fill with 3 synthetic clients spread across users
    // to ensure the per-user cap (2) does not also fire — we want to
    // isolate the per-doc cap branch.
    populate_room_with_clients(&app, &doc_id, &[("u1", 1), ("u2", 1), ("u3", 1)]).await;

    let ws_token = fetch_ws_token(&app, &token, &doc_id).await;
    let req = hyper::Request::builder()
        .method(Method::GET)
        .uri(&format!("/api/v1/documents/{doc_id}/ws?token={ws_token}"))
        .header("Origin", "http://localhost:8080")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = app.raw_request(req).await;
    assert_eq!(status, 429, "at-cap document must reject WS upgrade with 429");
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("concurrent-connection cap"),
        "body must explain the cap, got {body_str:?}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn ws_upgrade_rejects_when_per_user_cap_reached() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("ws-cap-user@test.com").await;
    // The actual user_id minted by /auth/dev-login is hashed from the
    // email; we recover it via the issued JWT. Easier path: read it
    // from the user_repo by email after token issue.
    let by_email = app
        .state
        .user_repo
        .get_by_email("ws-cap-user@test.com")
        .await
        .unwrap()
        .unwrap();
    let user_id = by_email.user_id.clone();
    let doc_id = app.create_doc(&token, "Solo crowd", None).await;

    // Per-user cap is 2. Pre-fill with 2 connections from the same
    // user, leaving room (capacity-wise) under the per-doc cap of 3.
    populate_room_with_clients(&app, &doc_id, &[(&user_id, 2)]).await;

    let ws_token = fetch_ws_token(&app, &token, &doc_id).await;
    let req = hyper::Request::builder()
        .method(Method::GET)
        .uri(&format!("/api/v1/documents/{doc_id}/ws?token={ws_token}"))
        .header("Origin", "http://localhost:8080")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = app.raw_request(req).await;
    assert_eq!(
        status, 429,
        "user with 2 existing connections must hit the per-user-per-doc cap on the 3rd"
    );
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("close other tabs") || body_str.contains("concurrent-connection cap"),
        "body must explain the cap, got {body_str:?}"
    );

    app.cleanup().await;
}

/// #111 (behavior change): a View-only collaborator may now mint a WS token
/// — but a *read-only* one. The token encodes its access level (server-
/// authored), so the room rejects write frames from a read-only session and
/// a captured read-only token can't be replayed to write. Previously this
/// returned 403 (a WS token implied write access); #111 splits the levels.
#[tokio::test]
async fn create_ws_token_view_collaborator_gets_read_only() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_alice_id, alice_token) = app.create_user("wstok-alice@test.com").await;
    let (bob_id, bob_token) = app.create_user("wstok-bob@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Owned", None).await;

    // Share with Bob at VIEW only.
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" })),
        )
        .await;
    assert_eq!(status, 204);

    // Bob (View) now gets a token — and validating it (consuming the
    // single-use entry) shows it carries ReadOnly authority.
    let (status, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/ws-token"),
            Some(&bob_token),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, 200, "a View collaborator may open a read-only live subscription");
    let bob_ws_token = body["token"].as_str().unwrap().to_string();
    let (_, _, _, access) = app
        .state
        .redis_pubsub
        .validate_ws_token(&bob_ws_token)
        .await
        .unwrap()
        .expect("view token is valid");
    assert_eq!(
        access,
        ogrenotes_collab::redis_pubsub::WsAccess::ReadOnly,
        "a View collaborator's token must be read-only",
    );

    // The owner gets a read-write token from the same endpoint.
    let (status, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/ws-token"),
            Some(&alice_token),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, 200);
    let alice_ws_token = body["token"].as_str().unwrap().to_string();
    let (_, _, _, access) = app
        .state
        .redis_pubsub
        .validate_ws_token(&alice_ws_token)
        .await
        .unwrap()
        .expect("owner token is valid");
    assert_eq!(
        access,
        ogrenotes_collab::redis_pubsub::WsAccess::ReadWrite,
        "the owner's token must be read-write",
    );

    app.cleanup().await;
}

/// A WS token is scoped to the document it was minted for. Presenting a
/// token minted for document A on document B's WS endpoint must be rejected
/// (401) — the cross-document `token_doc_id != doc_id` guard. Both docs are
/// owned by the caller, so Edit access is satisfied for both; only the
/// doc-scoping check can produce the rejection.
#[tokio::test]
async fn ws_upgrade_rejects_token_for_different_doc() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("wstok-scope@test.com").await;
    let doc_a = app.create_doc(&token, "Doc A", None).await;
    let doc_b = app.create_doc(&token, "Doc B", None).await;

    // Mint a token for doc A.
    let ws_token = fetch_ws_token(&app, &token, &doc_a).await;

    // Attempt to upgrade doc B's socket with doc A's token.
    let req = hyper::Request::builder()
        .method(Method::GET)
        .uri(&format!("/api/v1/documents/{doc_b}/ws?token={ws_token}"))
        .header("Origin", "http://localhost:8080")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = app.raw_request(req).await;
    assert_eq!(status, 401, "a token minted for another document must be rejected");

    app.cleanup().await;
}

// ─── #111: end-to-end read-only WebSocket write enforcement ────────────
//
// The oneshot harness can't exercise `handle_ws` (axum's WebSocketUpgrade
// returns 426 in oneshot mode), so this binds the router to a real TCP port
// and drives it with a tokio-tungstenite client — the harness gap noted at
// the top of this file. It proves the security property end-to-end: a
// read-only session's CRDT Update frame is dropped server-side (never
// applied, never persisted), while a read-write session's identical frame
// lands.

/// A valid content-fragment yrs update (mirrors `make_doc_bytes` in
/// test_history.rs) — applies cleanly to a room's OgreDoc, inserting one
/// paragraph so the doc state vector changes iff the server applied it.
fn content_update_bytes(text: &str) -> Vec<u8> {
    use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
    use yrs::{ReadTxn, Text, Transact, WriteTxn};

    let doc = yrs::Doc::new();
    {
        let mut txn = doc.transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        let p = frag.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
        let t = p.insert(&mut txn, 0, XmlTextPrelim::new(""));
        t.push(&mut txn, text);
    }
    doc.transact()
        .encode_state_as_update_v1(&yrs::StateVector::default())
}

/// Bind the test app's router to an ephemeral port and serve it. Returns the
/// address; the server task lives until the test process exits.
async fn serve_router(app: &common::TestApp) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    addr
}

/// Open a real WS connection with `ws_token`, capture the room's state
/// vector baseline, send one Update frame, then a SyncStep1, and block until
/// the server's SyncStep2 reply. Frame ordering guarantees the recv loop
/// fully processed the Update (applied OR dropped) before producing the
/// SyncStep2, so the returned `(before, after)` state vectors are a sleep-
/// free, baseline-independent witness of whether the write took effect.
async fn ws_attempt_write(
    app: &common::TestApp,
    addr: std::net::SocketAddr,
    doc_id: &str,
    ws_token: &str,
    update_frame: &[u8],
) -> (Vec<u8>, Vec<u8>) {
    use futures_util::{SinkExt, StreamExt};
    use ogrenotes_collab::protocol::{decode_message, encode_message, MessageType};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use yrs::updates::encoder::Encode;

    let url = format!("ws://{addr}/api/v1/documents/{doc_id}/ws?token={ws_token}");
    let mut request = url.as_str().into_client_request().unwrap();
    // dev_mode tolerates any Origin, but send a realistic one anyway.
    request
        .headers_mut()
        .insert("origin", "http://localhost:8080".parse().unwrap());

    let (mut ws, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .expect("ws handshake should succeed for a View+ token");

    // The connection cold-loaded the room into the registry; capture its
    // state vector as the baseline (sync_client only reads, never mutates).
    let before = app
        .state
        .room_registry
        .get(doc_id)
        .expect("the WS connection cold-loaded the room")
        .state_vector()
        .await;

    // The write attempt.
    ws.send(WsMessage::Binary(update_frame.to_vec().into()))
        .await
        .unwrap();

    // SyncStep1 with an empty state vector → server replies SyncStep2.
    let sv = yrs::StateVector::default().encode_v1();
    let step1 = encode_message(MessageType::SyncStep1, &sv);
    ws.send(WsMessage::Binary(step1.into())).await.unwrap();

    // Drain until the SyncStep2 reply — the ordered-stream barrier.
    loop {
        match ws.next().await {
            Some(Ok(WsMessage::Binary(data))) => {
                if let Some((MessageType::SyncStep2, _)) = decode_message(data.as_ref()) {
                    break;
                }
            }
            Some(Ok(_)) => {} // ignore the server's initial sync / awareness / pings
            Some(Err(e)) => panic!("ws error before SyncStep2 barrier: {e}"),
            None => panic!("ws closed before SyncStep2 barrier"),
        }
    }

    let after = app
        .state
        .room_registry
        .get(doc_id)
        .expect("room still resident")
        .state_vector()
        .await;
    let _ = ws.close(None).await;
    (before, after)
}

/// Poll the persisted update log (the query side can lag the write under
/// DynamoDB-Local eventual consistency).
async fn persisted_update_count(app: &common::TestApp, doc_id: &str) -> usize {
    for _ in 0..20 {
        let updates = app
            .state
            .doc_repo
            .get_pending_updates(doc_id, 10_000_000)
            .await
            .unwrap();
        if !updates.is_empty() {
            return updates.len();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    0
}

/// #111: a read-only WS session cannot mutate the doc; a read-write session
/// can. Drives the real `handle_ws` loop over a TCP socket and asserts both
/// the in-memory room state (via a before/after state-vector witness) and the
/// persisted update log.
#[tokio::test]
async fn ws_read_only_session_write_frame_is_dropped() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let addr = serve_router(&app).await;

    let (_alice_id, alice_token) = app.create_user("ws-ro-alice@test.com").await;
    let (bob_id, bob_token) = app.create_user("ws-ro-bob@test.com").await;

    // Two separate docs so the read-only and read-write attempts don't
    // cross-contaminate each other's state.
    let ro_doc = app.create_doc(&alice_token, "RO target", None).await;
    let rw_doc = app.create_doc(&alice_token, "RW target", None).await;

    // Bob is a View-only member on the read-only doc.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{ro_doc}/members"),
            Some(&alice_token),
            Some(serde_json::json!({ "userId": bob_id, "accessLevel": "VIEW" })),
        )
        .await;
    assert_eq!(s, 204);

    // ── Read-only session: Bob's Update must be dropped. ──
    let ro_token = fetch_ws_token(&app, &bob_token, &ro_doc).await;
    let ro_update = ogrenotes_collab::protocol::encode_message(
        ogrenotes_collab::protocol::MessageType::Update,
        &content_update_bytes("read-only-write-attempt"),
    );
    let (ro_before, ro_after) = ws_attempt_write(&app, addr, &ro_doc, &ro_token, &ro_update).await;
    assert_eq!(
        ro_before, ro_after,
        "a read-only session's Update must NOT change the in-memory doc",
    );
    let ro_persisted = app
        .state
        .doc_repo
        .get_pending_updates(&ro_doc, 10_000_000)
        .await
        .unwrap();
    assert!(
        ro_persisted.is_empty(),
        "a read-only session's Update must NOT be persisted, got {} updates",
        ro_persisted.len(),
    );

    // ── Read-write session: the owner's identical-shape Update must land. ──
    let rw_token = fetch_ws_token(&app, &alice_token, &rw_doc).await;
    let rw_update = ogrenotes_collab::protocol::encode_message(
        ogrenotes_collab::protocol::MessageType::Update,
        &content_update_bytes("read-write-applied"),
    );
    let (rw_before, rw_after) = ws_attempt_write(&app, addr, &rw_doc, &rw_token, &rw_update).await;
    assert_ne!(
        rw_before, rw_after,
        "a read-write session's Update must change the in-memory doc",
    );
    assert!(
        persisted_update_count(&app, &rw_doc).await >= 1,
        "a read-write session's Update must be persisted",
    );

    app.cleanup().await;
}

/// Build an update-v1 that inserts a Kanban board (with one column and
/// one card carrying the given color) into a fresh empty doc. When
/// applied to the server's initial doc, the effect is "insert this
/// kanban tree". The card's color is what the strict-compare gate
/// keys off — passing "javascript:" triggers `validate_card_attrs`'s
/// enum-whitelist check.
fn kanban_insert_bytes(card_color: &str) -> Vec<u8> {
    use yrs::types::xml::{Xml, XmlElementPrelim, XmlFragment};
    use yrs::{ReadTxn, Transact, WriteTxn};

    let doc = yrs::Doc::new();
    {
        let mut txn = doc.transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        // Append after the seeded paragraph — matches what a real
        // client would do on Insert-Kanban from an empty doc.
        let n = frag.len(&txn);
        let board = frag.insert(&mut txn, n, XmlElementPrelim::empty("kanban"));
        let col = board.insert(&mut txn, 0, XmlElementPrelim::empty("kanban_column"));
        col.insert_attribute(&mut txn, "title", "To Do");
        let card = col.insert(&mut txn, 0, XmlElementPrelim::empty("kanban_card"));
        card.insert_attribute(&mut txn, "title", "Fix");
        card.insert_attribute(&mut txn, "color", card_color);
    }
    doc.transact()
        .encode_state_as_update_v1(&yrs::StateVector::default())
}

/// Open a WS with `ws_token`, send `frame`, and read incoming frames
/// until a `MessageType::Error` lands (or the connection closes).
/// Returns the error payload as a String. Panics if the connection
/// closes without an error frame — a silent-drop regression is
/// exactly the shape this test guards against.
async fn ws_send_and_await_error_frame(
    addr: std::net::SocketAddr,
    doc_id: &str,
    ws_token: &str,
    frame: &[u8],
) -> String {
    use futures_util::{SinkExt, StreamExt};
    use ogrenotes_collab::protocol::{decode_message, MessageType};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let url = format!("ws://{addr}/api/v1/documents/{doc_id}/ws?token={ws_token}");
    let mut request = url.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "http://localhost:8080".parse().unwrap());
    let (mut ws, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .expect("ws handshake");

    ws.send(WsMessage::Binary(frame.to_vec().into()))
        .await
        .unwrap();

    // Bounded read — a wired-in write-then-error round-trip should
    // land within a handful of frames.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(WsMessage::Binary(data)))) => {
                if let Some((MessageType::Error, payload)) = decode_message(data.as_ref()) {
                    let _ = ws.close(None).await;
                    return String::from_utf8_lossy(payload).into_owned();
                }
            }
            Ok(Some(Ok(_))) => {} // sync frames, awareness, etc. — keep waiting
            Ok(Some(Err(e))) => panic!("ws error before error frame: {e}"),
            Ok(None) => panic!("ws closed without sending an error frame"),
            Err(_) => panic!("timed out waiting for MessageType::Error frame"),
        }
    }
}

/// Phase 2a Option A wire-level coverage: a reject-mode gate
/// violation must produce a `MessageType::Error` frame with the
/// `liveapp-rejected:` prefix back to the offending client.
/// Without this end-to-end test, a regression in the WS handler
/// wiring (e.g. someone removes the `LiveAppRejected` branch or
/// swaps the prefix) would only surface at the frontend-toast
/// level in production, silently until a user reported "my change
/// vanished with no warning."
#[tokio::test]
async fn ws_liveapp_reject_sends_error_frame_to_client() {
    common::require_infra!();
    let mut app = common::TestApp::new().await;
    app.set_liveapp_validation_mode("reject");
    let addr = serve_router(&app).await;

    let (_, alice_token) = app.create_user("ws-liveapp-alice@test.com").await;
    let doc_id = app.create_doc(&alice_token, "LiveApp Reject", None).await;

    // "javascript:" is off the CARD_COLORS whitelist so
    // validate_card_attrs returns Err — the classic Err path.
    let bad_update = kanban_insert_bytes("javascript:");
    let update_frame = ogrenotes_collab::protocol::encode_message(
        ogrenotes_collab::protocol::MessageType::Update,
        &bad_update,
    );

    let ws_token = fetch_ws_token(&app, &alice_token, &doc_id).await;
    let error_payload =
        ws_send_and_await_error_frame(addr, &doc_id, &ws_token, &update_frame).await;

    assert!(
        error_payload.starts_with("liveapp-rejected:"),
        "expected liveapp-rejected error frame, got {error_payload:?}"
    );
    assert!(
        error_payload.contains("color"),
        "diagnostic should identify the offending field, got {error_payload:?}"
    );

    // And the doc's persisted update log must not include the
    // rejected write — confirms the gate refused BEFORE apply,
    // matching the DocError::LiveAppRejected → no-apply contract
    // in Room::apply_update_gated.
    let persisted = app
        .state
        .doc_repo
        .get_pending_updates(&doc_id, 10_000_000)
        .await
        .unwrap();
    assert!(
        persisted.is_empty(),
        "reject-mode gate must NOT persist the rejected update, got {} rows",
        persisted.len(),
    );

    app.cleanup().await;
}

/// gap-001 option 2 — under reject mode, an update that would
/// normally be rejected sails through when the doc-id is on the
/// operator-set exempt list. Pins the WS-path escape hatch that
/// mirrors the REST-path test in test_documents.rs.
#[tokio::test]
async fn ws_liveapp_gate_exemption_bypasses_reject() {
    common::require_infra!();
    let mut app = common::TestApp::new().await;
    app.set_liveapp_validation_mode("reject");

    let (_, alice_token) = app.create_user("ws-liveapp-exempt@test.com").await;
    let doc_id = app.create_doc(&alice_token, "Exempt WS Doc", None).await;

    // Move this doc onto the exempt list — the same payload the
    // sibling reject test proved gets an error frame should now
    // land in-doc without any error frame.
    app.set_liveapp_gate_exempt_doc_ids(&[&doc_id]);

    let addr = serve_router(&app).await;
    let bad_update = kanban_insert_bytes("javascript:");
    let update_frame = ogrenotes_collab::protocol::encode_message(
        ogrenotes_collab::protocol::MessageType::Update,
        &bad_update,
    );

    let ws_token = fetch_ws_token(&app, &alice_token, &doc_id).await;
    let (before, after) =
        ws_attempt_write(&app, addr, &doc_id, &ws_token, &update_frame).await;
    assert_ne!(
        before, after,
        "an exempted doc's Update must land even under reject mode"
    );
    assert!(
        persisted_update_count(&app, &doc_id).await >= 1,
        "exempted doc must persist the update"
    );

    app.cleanup().await;
}

/// gap-003 — an interactive Update that deletes a KanbanCard
/// produces a `LiveAppNodeDeleted` SecurityAudit row keyed on the
/// actor. Pins the end-to-end wire path from CRDT delta to audit
/// storage: the WS Update handler reads `apply_update_gated`'s
/// `report.deletions` and calls `record_security_event_by_actor`
/// for each entry.
#[tokio::test]
async fn ws_kanban_card_deletion_emits_audit_row() {
    use futures_util::{SinkExt, StreamExt};
    use ogrenotes_collab::document::OgreDoc;
    use ogrenotes_collab::protocol::MessageType;
    use ogrenotes_collab::schema::NodeType;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use yrs::types::xml::{Xml, XmlElementPrelim, XmlFragment, XmlOut};
    use yrs::{ReadTxn, Transact, WriteTxn};

    common::require_infra!();
    let mut app = common::TestApp::new().await;
    app.set_liveapp_validation_mode("reject");

    let (owner_id, owner_token) =
        app.create_user("ws-audit-delete-owner@test.com").await;
    let doc_id = app.create_doc(&owner_token, "Delete Target", None).await;

    // Land a Kanban board with one card via put_content
    // (exemption ON so the payload with a fresh card lands
    // cleanly under reject mode).
    app.set_liveapp_gate_exempt_doc_ids(&[&doc_id]);
    let baseline_doc = OgreDoc::new();
    {
        let mut txn = baseline_doc.inner().transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        let n = frag.len(&txn);
        if n > 0 {
            frag.remove_range(&mut txn, 0, n);
        }
        let board = frag.insert(
            &mut txn,
            0,
            XmlElementPrelim::empty(NodeType::Kanban.tag_name()),
        );
        let col = board.insert(
            &mut txn,
            0,
            XmlElementPrelim::empty(NodeType::KanbanColumn.tag_name()),
        );
        col.insert_attribute(&mut txn, "title", "To Do");
        let card = col.insert(
            &mut txn,
            0,
            XmlElementPrelim::empty(NodeType::KanbanCard.tag_name()),
        );
        card.insert_attribute(&mut txn, "title", "Delete me");
        card.insert_attribute(&mut txn, "color", "red");
    }
    let baseline = baseline_doc.to_state_bytes();
    let (status, _) = app
        .bytes_request(
            hyper::Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&owner_token),
            baseline,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);
    app.set_liveapp_gate_exempt_doc_ids(&[]);

    // Open a WS. The connection cold-loads the room by
    // reconstructing the doc from the snapshot, so the peer's
    // baseline state must match what we uploaded.
    let addr = serve_router(&app).await;
    let ws_token = fetch_ws_token(&app, &owner_token, &doc_id).await;
    let url = format!("ws://{addr}/api/v1/documents/{doc_id}/ws?token={ws_token}");
    let mut request = url.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "http://localhost:8080".parse().unwrap());
    let (mut ws, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .expect("ws handshake");

    // Load a peer doc from the room's current state, then
    // delete the card and encode the diff.
    let server_state = app
        .state
        .room_registry
        .get(&doc_id)
        .expect("room resident")
        .to_state_bytes()
        .await;
    let peer = OgreDoc::from_state_bytes(&server_state).unwrap();
    let card_block_id = {
        let txn = peer.inner().transact();
        let frag = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
        let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
        let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
        card.get_attribute(&txn, "blockId").unwrap_or_default()
    };
    {
        let mut txn = peer.inner().transact_mut();
        let frag = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
        let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
        col.remove_range(&mut txn, 0, 1);
    }
    let server_sv = app
        .state
        .room_registry
        .get(&doc_id)
        .unwrap()
        .state_vector()
        .await;
    let mutation = peer.encode_diff(&server_sv).unwrap();
    let delete_frame = ogrenotes_collab::protocol::encode_message(
        MessageType::Update,
        &mutation,
    );
    ws.send(WsMessage::Binary(delete_frame.into()))
        .await
        .unwrap();

    // Give the handler a moment to process the frame and land
    // the audit write (which spawns).
    let mut audit_found = false;
    for _ in 0..40 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(&owner_id, 20)
            .await
            .unwrap();
        if rows.iter().any(|r| matches!(
            &r.action,
            ogrenotes_storage::models::security_audit::SecurityAuditAction::LiveAppNodeDeleted { node_type, block_id, .. }
                if node_type == "kanban_card" && block_id == &card_block_id
        )) {
            audit_found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let _ = ws.close(None).await;
    assert!(audit_found, "expected LiveAppNodeDeleted audit row for the deleted card");

    app.cleanup().await;
}

/// Wire-level coverage of the `foreign-doc-subscribe-denied:` error
/// frame — a sibling error path to `liveapp-rejected:` that was
/// unrated at the wire level in the Option A review. If a client
/// asks to subscribe to a foreign doc they don't have View access
/// on, the server must respond with `MessageType::Error`
/// (`foreign-doc-subscribe-denied:<doc_id>`) so the client can
/// paint `#REF!` and stop waiting for updates.
#[tokio::test]
async fn ws_foreign_doc_subscribe_denied_sends_error_frame() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let addr = serve_router(&app).await;

    // Alice owns her own doc (used for the WS connection) and
    // Bob owns a private doc Alice has no access to (the
    // foreign-subscribe target).
    let (_alice_id, alice_token) = app.create_user("ws-fd-alice@test.com").await;
    let (_bob_id, bob_token) = app.create_user("ws-fd-bob@test.com").await;
    let alice_doc = app.create_doc(&alice_token, "Alice Home", None).await;
    let bob_private_doc = app.create_doc(&bob_token, "Bob Private", None).await;

    // Alice connects to her doc, then sends a
    // `SubscribeForeignDoc` frame targeting Bob's private doc.
    let ws_token = fetch_ws_token(&app, &alice_token, &alice_doc).await;
    let subscribe_frame = ogrenotes_collab::protocol::encode_message(
        ogrenotes_collab::protocol::MessageType::SubscribeForeignDoc,
        bob_private_doc.as_bytes(),
    );
    let error_payload =
        ws_send_and_await_error_frame(addr, &alice_doc, &ws_token, &subscribe_frame).await;

    assert!(
        error_payload.starts_with("foreign-doc-subscribe-denied:"),
        "expected foreign-doc-subscribe-denied error frame, got {error_payload:?}"
    );
    // The denied doc id is echoed after the colon so the client
    // can map back to the specific REFERENCE call.
    assert!(
        error_payload.contains(&bob_private_doc),
        "diagnostic should carry the denied doc id, got {error_payload:?}"
    );

    app.cleanup().await;
}
