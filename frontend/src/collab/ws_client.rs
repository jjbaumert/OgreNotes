//! WebSocket collaboration client for real-time document sync.
//!
//! Connects to the server's WebSocket endpoint, handles the yrs sync protocol,
//! and bridges between the editor's Transaction system and yrs incremental updates.
//!
//! ## Architecture
//!
//! Each `CollabClient` maintains a **single persistent `yrs::Doc`** for the session.
//! Local edits are applied to this Doc via `sync_model_to_ydoc`, and an
//! `observe_update_v1` callback captures incremental update bytes for transmission.
//! Remote updates are applied via `apply_update` on the same Doc, and a boolean flag
//! prevents the observer from re-sending remote changes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

use yrs::{ReadTxn, Transact};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;

use crate::editor::model::Node;
use crate::editor::yrs_bridge;

use serde::{Deserialize, Serialize};

/// RAII guard that sets `is_applying_remote` to `true` on creation and resets
/// to `false` on drop. Ensures the flag is always reset even if yrs panics
/// (WASM catches panics as JS exceptions without unwinding, but Drop still runs
/// for values in the current scope).
struct RemoteApplyGuard(Rc<Cell<bool>>);

impl RemoteApplyGuard {
    fn new(flag: &Rc<Cell<bool>>) -> Self {
        flag.set(true);
        Self(Rc::clone(flag))
    }
}

impl Drop for RemoteApplyGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Configurable debounce for outgoing WS updates (ms).
/// Reduced from 50ms since incremental updates are tiny.
pub const WS_SEND_DEBOUNCE_MS: u32 = 16;

/// Configurable debounce for incoming WS update model conversion (ms).
/// Reduced from 30ms since apply_update is fast on incremental payloads.
pub const WS_RECV_DEBOUNCE_MS: u32 = 16;

// Protocol constants (must match crates/collab/src/protocol.rs)
const MSG_AUTH: u8 = 0x00;
const MSG_SYNC_STEP1: u8 = 0x01;
const MSG_SYNC_STEP2: u8 = 0x02;
const MSG_UPDATE: u8 = 0x03;
const MSG_AWARENESS: u8 = 0x04;
const MSG_ERROR: u8 = 0xFF;

/// Color palette for collaborator cursors (must match backend).
const CURSOR_COLORS: [&str; 12] = [
    "#E57373", "#64B5F6", "#81C784", "#FFB74D",
    "#BA68C8", "#4DD0E1", "#F06292", "#AED581",
    "#FFD54F", "#7986CB", "#4DB6AC", "#A1887F",
];

/// JSON payload for awareness messages (matches backend AwarenessState).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AwarenessPayload {
    user_id: String,
    name: String,
    color: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor_pos: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_anchor: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_head: Option<u32>,
}

/// State of the WebSocket connection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Synced,
}

/// Callback for when the document is updated by a remote collaborator.
pub type OnRemoteUpdate = Box<dyn Fn(Node)>;

/// Remote user's cursor/selection state for presence rendering.
#[derive(Debug, Clone)]
pub struct RemoteCursor {
    pub user_id: String,
    pub name: String,
    pub color: String,
    pub cursor_pos: Option<u32>,
    pub selection_anchor: Option<u32>,
    pub selection_head: Option<u32>,
}

/// Callback for when remote cursors change.
pub type OnAwarenessUpdate = Box<dyn Fn(Vec<RemoteCursor>)>;

// ─── Message handling helpers ──────────────────────────────────

/// Wrap a closure as a `Closure<dyn Fn(web_sys::Event)>`.
fn wrap_event(f: impl Fn(web_sys::Event) + 'static) -> Closure<dyn Fn(web_sys::Event)> {
    Closure::wrap(Box::new(f) as Box<dyn Fn(web_sys::Event)>)
}

/// Extract binary payload from a WebSocket MessageEvent.
fn extract_binary_payload(event: &web_sys::Event) -> Option<Vec<u8>> {
    let me = event.dyn_ref::<MessageEvent>()?;
    let buf = me.data().dyn_into::<js_sys::ArrayBuffer>().ok()?;
    Some(js_sys::Uint8Array::new(&buf).to_vec())
}

/// Send a binary message over the WebSocket (type byte + payload).
fn ws_send(ws_ref: &Rc<RefCell<Option<WebSocket>>>, msg_type: u8, payload: &[u8]) {
    if let Some(ws) = ws_ref.borrow().as_ref() {
        let mut msg = vec![msg_type];
        msg.extend_from_slice(payload);
        let _ = ws.send_with_u8_array(&msg);
    }
}

/// Handle MSG_SYNC_STEP1: server sent its state vector — respond with our diff + our SV.
fn handle_sync_step1(
    payload: &[u8],
    ydoc: &Rc<RefCell<yrs::Doc>>,
    ws_ref: &Rc<RefCell<Option<WebSocket>>>,
) {
    crate::editor::debug::log("collab", "received SyncStep1", &[]);
    let ydoc = ydoc.borrow();
    let txn = ydoc.transact();
    if let Ok(sv) = yrs::StateVector::decode_v1(payload) {
        let diff = txn.encode_state_as_update_v1(&sv);
        ws_send(ws_ref, MSG_SYNC_STEP2, &diff);

        let our_sv = txn.state_vector().encode_v1();
        ws_send(ws_ref, MSG_SYNC_STEP1, &our_sv);
    }
}

/// Apply a remote yrs update with the remote-apply guard (suppresses observer).
fn apply_remote_update(
    payload: &[u8],
    ydoc: &Rc<RefCell<yrs::Doc>>,
    is_remote: &Rc<Cell<bool>>,
) {
    crate::editor::debug::log("collab", "received update", &[
        ("size", &payload.len().to_string()),
    ]);
    let _guard = RemoteApplyGuard::new(is_remote);
    let ydoc = ydoc.borrow();
    let mut txn = ydoc.transact_mut();
    if let Ok(update) = yrs::Update::decode_v1(payload) {
        let _ = txn.apply_update(update);
    }
}

/// Schedule a debounced callback to read the model from yrs and notify the UI.
/// Uses gloo_timers::Timeout (not spawn_local) to avoid re-entrant task queue polling.
fn schedule_remote_callback(
    ydoc: &Rc<RefCell<yrs::Doc>>,
    on_remote: &Rc<RefCell<Option<OnRemoteUpdate>>>,
    timer: &Rc<RefCell<Option<gloo_timers::callback::Timeout>>>,
) {
    let ydoc_ref = ydoc.clone();
    let on_remote_ref = on_remote.clone();
    *timer.borrow_mut() = Some(gloo_timers::callback::Timeout::new(
        WS_RECV_DEBOUNCE_MS,
        move || {
            let doc = {
                let ydoc = ydoc_ref.borrow();
                yrs_bridge::read_doc_from_ydoc(&ydoc).ok()
            };
            if let Some(doc) = doc {
                if let Some(callback) = on_remote_ref.borrow().as_ref() {
                    callback(doc);
                }
            }
        },
    ));
}

/// Handle MSG_AWARENESS: update remote cursor state and notify callback.
fn handle_awareness(
    payload: &[u8],
    remote_cursors: &Rc<RefCell<std::collections::HashMap<String, RemoteCursor>>>,
    on_awareness: &Rc<RefCell<Option<OnAwarenessUpdate>>>,
) {
    let Ok(state) = serde_json::from_slice::<AwarenessPayload>(payload) else { return };
    let color_idx = (state.color as usize) % CURSOR_COLORS.len();
    let cursor = RemoteCursor {
        user_id: state.user_id.clone(),
        name: state.name.clone(),
        color: CURSOR_COLORS[color_idx].to_string(),
        cursor_pos: state.cursor_pos,
        selection_anchor: state.selection_anchor,
        selection_head: state.selection_head,
    };
    remote_cursors.borrow_mut().insert(state.user_id, cursor);

    if let Some(callback) = on_awareness.borrow().as_ref() {
        let cursors: Vec<RemoteCursor> = remote_cursors.borrow().values().cloned().collect();
        callback(cursors);
    }
}

// ─── CollabClient ──────────────────────────────────────────────

/// WebSocket collaboration client.
/// Maintains a **persistent** yrs Doc for incremental sync.
pub struct CollabClient {
    /// WebSocket connection (None if disconnected).
    ws: Rc<RefCell<Option<WebSocket>>>,
    /// Connection state.
    state: Rc<RefCell<ConnectionState>>,
    /// The persistent yrs Doc that accumulates all updates (local and remote).
    /// Never replaced mid-session — preserves client_id for correct CRDT behavior.
    ydoc: Rc<RefCell<yrs::Doc>>,
    /// Document ID.
    doc_id: String,
    /// Callback when remote update changes the document.
    on_remote_update: Rc<RefCell<Option<OnRemoteUpdate>>>,
    /// Callback when remote cursors change.
    on_awareness_update: Rc<RefCell<Option<OnAwarenessUpdate>>>,
    /// Remote user awareness states.
    remote_cursors: Rc<RefCell<std::collections::HashMap<String, RemoteCursor>>>,
    /// Stored closures (prevent GC).
    _closures: Rc<RefCell<Vec<Closure<dyn Fn(web_sys::Event)>>>>,
    /// Incremental updates queued by observe_update_v1 for sending.
    pending_updates: Rc<RefCell<Vec<Vec<u8>>>>,
    /// Flag to suppress observer when applying remote updates.
    is_applying_remote: Rc<Cell<bool>>,
    /// Subscription for observe_update_v1 (must stay alive).
    _update_sub: yrs::Subscription,
}

impl CollabClient {
    /// Create a new collab client for a document.
    /// `initial_bytes` is the full yrs state from the REST API (initial load).
    pub fn new(doc_id: String, initial_bytes: Option<&[u8]>) -> Self {
        let ydoc = yrs::Doc::new();

        // Apply initial state if provided
        if let Some(bytes) = initial_bytes {
            let mut txn = ydoc.transact_mut();
            if let Ok(update) = yrs::Update::decode_v1(bytes) {
                let _ = txn.apply_update(update);
            }
        }

        // Set up incremental update observer
        let pending_updates: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let is_applying_remote: Rc<Cell<bool>> = Rc::new(Cell::new(false));

        let pending_ref = Rc::clone(&pending_updates);
        let remote_flag_ref = Rc::clone(&is_applying_remote);

        let update_sub = ydoc.observe_update_v1(move |_txn, event| {
            // Only queue local changes; skip when applying remote updates
            if !remote_flag_ref.get() {
                pending_ref.borrow_mut().push(event.update.clone());
            }
        }).expect("observe_update_v1 should not fail on a fresh Doc");

        Self {
            ws: Rc::new(RefCell::new(None)),
            state: Rc::new(RefCell::new(ConnectionState::Disconnected)),
            ydoc: Rc::new(RefCell::new(ydoc)),
            doc_id,
            on_remote_update: Rc::new(RefCell::new(None)),
            on_awareness_update: Rc::new(RefCell::new(None)),
            remote_cursors: Rc::new(RefCell::new(std::collections::HashMap::new())),
            _closures: Rc::new(RefCell::new(Vec::new())),
            pending_updates,
            is_applying_remote,
            _update_sub: update_sub,
        }
    }

    /// Set the callback for remote document updates.
    pub fn set_on_remote_update(&self, callback: OnRemoteUpdate) {
        *self.on_remote_update.borrow_mut() = Some(callback);
    }

    /// Set the callback for remote awareness updates (cursor presence).
    pub fn set_on_awareness_update(&self, callback: OnAwarenessUpdate) {
        *self.on_awareness_update.borrow_mut() = Some(callback);
    }

    /// Get the current connection state.
    pub fn connection_state(&self) -> ConnectionState {
        *self.state.borrow()
    }

    /// Connect to the WebSocket server.
    /// `connected_flag` is set to true when synced, false on disconnect.
    pub fn connect(&self, ws_url: &str, _token: &str, connected_flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        *self.state.borrow_mut() = ConnectionState::Connecting;

        let ws = match WebSocket::new(ws_url) {
            Ok(ws) => ws,
            Err(e) => {
                web_sys::console::error_1(&format!("WebSocket connect failed: {e:?}").into());
                *self.state.borrow_mut() = ConnectionState::Disconnected;
                return;
            }
        };
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        let state = Rc::clone(&self.state);
        let ydoc = Rc::clone(&self.ydoc);
        let ws_ref = Rc::clone(&self.ws);
        let closures = Rc::clone(&self._closures);

        // ── onopen ──
        let state_for_open = Rc::clone(&state);
        let on_open = wrap_event(move |_| {
            *state_for_open.borrow_mut() = ConnectionState::Connected;
            crate::editor::debug::log("collab", "WebSocket connected", &[]);
        });

        // ── onmessage ──
        let on_message = {
            let state = Rc::clone(&state);
            let ydoc = Rc::clone(&ydoc);
            let ws_ref = Rc::clone(&ws_ref);
            let is_remote = Rc::clone(&self.is_applying_remote);
            let on_remote = Rc::clone(&self.on_remote_update);
            let on_awareness = Rc::clone(&self.on_awareness_update);
            let remote_cursors = Rc::clone(&self.remote_cursors);
            let flag = std::sync::Arc::clone(&connected_flag);
            let pending_recv_timer: Rc<RefCell<Option<gloo_timers::callback::Timeout>>> =
                Rc::new(RefCell::new(None));

            wrap_event(move |event: web_sys::Event| {
                let Some(data) = extract_binary_payload(&event) else { return };
                if data.is_empty() { return; }

                let msg_type = data[0];
                let payload = &data[1..];

                match msg_type {
                    MSG_SYNC_STEP1 => handle_sync_step1(payload, &ydoc, &ws_ref),
                    MSG_SYNC_STEP2 => {
                        apply_remote_update(payload, &ydoc, &is_remote);
                        *state.borrow_mut() = ConnectionState::Synced;
                        flag.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    MSG_UPDATE => {
                        apply_remote_update(payload, &ydoc, &is_remote);
                        schedule_remote_callback(&ydoc, &on_remote, &pending_recv_timer);
                    }
                    MSG_AWARENESS => handle_awareness(payload, &remote_cursors, &on_awareness),
                    MSG_ERROR => {
                        if let Ok(error) = std::str::from_utf8(payload) {
                            web_sys::console::error_1(&format!("WebSocket error: {error}").into());
                        }
                    }
                    _ => {}
                }
            })
        };

        // ── onclose ──
        let state_for_close = Rc::clone(&state);
        let flag_for_close = std::sync::Arc::clone(&connected_flag);
        let on_close = wrap_event(move |_| {
            *state_for_close.borrow_mut() = ConnectionState::Disconnected;
            flag_for_close.store(false, std::sync::atomic::Ordering::Relaxed);
            crate::editor::debug::log("collab", "WebSocket disconnected", &[]);
        });

        // ── onerror ──
        let on_error = wrap_event(move |_| {
            web_sys::console::error_1(&"WebSocket error".into());
        });

        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        *self.ws.borrow_mut() = Some(ws);
        let mut cls = closures.borrow_mut();
        cls.push(on_open);
        cls.push(on_message);
        cls.push(on_close);
        cls.push(on_error);
    }

    /// Send a local editor change as an incremental yrs update.
    ///
    /// Applies the model diff to the persistent Doc via `sync_model_to_ydoc`.
    /// The `observe_update_v1` callback captures only the incremental bytes,
    /// which are then sent over WebSocket. This preserves the Doc's client_id
    /// and produces minimal network payloads.
    pub fn send_update(&self, new_doc: &Node) {
        if *self.state.borrow() != ConnectionState::Synced {
            crate::editor::debug::log("collab", "send_update skipped (not synced)", &[]);
            return;
        }

        // Apply model diff to persistent Doc — observer captures incremental bytes
        {
            let ydoc = self.ydoc.borrow();
            yrs_bridge::sync_model_to_ydoc(&ydoc, new_doc);
        }

        // Drain pending updates and send each as an Update message
        let updates: Vec<Vec<u8>> = self.pending_updates.borrow_mut().drain(..).collect();
        if updates.is_empty() {
            crate::editor::debug::log("collab", "send_update: no changes detected", &[]);
        }
        for update_bytes in &updates {
            crate::editor::debug::log("collab", "sending incremental update", &[
                ("size", &update_bytes.len().to_string()),
            ]);
            if let Some(ws) = self.ws.borrow().as_ref() {
                let mut msg = vec![MSG_UPDATE];
                msg.extend_from_slice(update_bytes);
                let _ = ws.send_with_u8_array(&msg);
            }
        }
    }

    /// Send local cursor/selection position as an awareness update.
    pub fn send_awareness(&self, user_id: &str, name: &str, color: u8, cursor_pos: Option<u32>, selection_anchor: Option<u32>, selection_head: Option<u32>) {
        if *self.state.borrow() != ConnectionState::Synced {
            return;
        }
        let payload = AwarenessPayload {
            user_id: user_id.to_string(),
            name: name.to_string(),
            color,
            cursor_pos,
            selection_anchor,
            selection_head,
        };
        if let Ok(json) = serde_json::to_vec(&payload) {
            if let Some(ws) = self.ws.borrow().as_ref() {
                let mut msg = vec![MSG_AWARENESS];
                msg.extend_from_slice(&json);
                let _ = ws.send_with_u8_array(&msg);
            }
        }
    }

    /// Disconnect the WebSocket.
    pub fn disconnect(&self) {
        if let Some(ws) = self.ws.borrow().as_ref() {
            let _ = ws.close();
        }
        *self.ws.borrow_mut() = None;
        *self.state.borrow_mut() = ConnectionState::Disconnected;
    }

    /// Whether the client is connected and synced.
    pub fn is_synced(&self) -> bool {
        *self.state.borrow() == ConnectionState::Synced
    }
}

impl Drop for CollabClient {
    fn drop(&mut self) {
        self.disconnect();
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::{Fragment, Node, NodeType};
    use crate::editor::yrs_bridge;

    fn simple_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        )
    }

    // ── Construction ──

    #[test]
    fn new_creates_empty_doc() {
        let client = CollabClient::new("doc1".into(), None);
        assert!(!client.is_synced());
        assert_eq!(*client.state.borrow(), ConnectionState::Disconnected);
        // ydoc has no content fragment yet (empty)
        let ydoc = client.ydoc.borrow();
        let txn = ydoc.transact();
        assert!(txn.get_xml_fragment("content").is_none());
    }

    #[test]
    fn new_with_initial_bytes_loads_state() {
        let doc = simple_doc();
        let bytes = yrs_bridge::doc_to_ydoc_bytes(&doc);
        let client = CollabClient::new("doc1".into(), Some(&bytes));

        let ydoc = client.ydoc.borrow();
        let restored = yrs_bridge::read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(restored.text_content(), "Hello world");
    }

    // ── send_update ──

    #[test]
    fn send_update_when_not_synced_is_noop() {
        let doc = simple_doc();
        let bytes = yrs_bridge::doc_to_ydoc_bytes(&doc);
        let client = CollabClient::new("doc1".into(), Some(&bytes));
        // State is Disconnected, not Synced
        assert!(!client.is_synced());

        let modified = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Modified")]),
            )]),
        );
        client.send_update(&modified);

        // No crash, pending_updates should be empty (send_update returned early)
        // But the observer may have fired during sync_model_to_ydoc... no, send_update
        // returns before calling sync_model_to_ydoc when not synced.
        assert!(client.pending_updates.borrow().is_empty());
    }

    #[test]
    fn send_update_captures_and_applies_changes() {
        let doc = simple_doc();
        let bytes = yrs_bridge::doc_to_ydoc_bytes(&doc);
        let client = CollabClient::new("doc1".into(), Some(&bytes));

        // Force state to Synced so send_update proceeds
        *client.state.borrow_mut() = ConnectionState::Synced;

        let modified = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Modified text")]),
            )]),
        );
        client.send_update(&modified);

        // After send_update, the ydoc should have the new content
        let ydoc = client.ydoc.borrow();
        let restored = yrs_bridge::read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(restored.text_content(), "Modified text");

        // pending_updates should be drained (send_update drains them)
        assert!(client.pending_updates.borrow().is_empty());
    }

    #[test]
    fn send_update_no_change_produces_no_updates() {
        let doc = simple_doc();
        let bytes = yrs_bridge::doc_to_ydoc_bytes(&doc);
        let client = CollabClient::new("doc1".into(), Some(&bytes));
        *client.state.borrow_mut() = ConnectionState::Synced;

        // Send the same doc — no changes
        client.send_update(&doc);

        // ydoc content unchanged
        let ydoc = client.ydoc.borrow();
        let restored = yrs_bridge::read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(restored.text_content(), "Hello world");
    }

    // ── RemoteApplyGuard ──

    #[test]
    fn remote_apply_guard_sets_and_resets_flag() {
        let flag = Rc::new(Cell::new(false));
        {
            let _guard = RemoteApplyGuard::new(&flag);
            assert!(flag.get(), "flag should be true while guard is alive");
        }
        assert!(!flag.get(), "flag should be false after guard is dropped");
    }

    #[test]
    fn remote_apply_guard_resets_on_panic() {
        let flag = Rc::new(Cell::new(false));
        let flag_clone = Rc::clone(&flag);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = RemoteApplyGuard::new(&flag_clone);
            assert!(flag_clone.get());
            panic!("simulated panic");
        }));
        assert!(result.is_err());
        assert!(!flag.get(), "flag should be reset even after panic");
    }

    // ── Observer suppression ──

    #[test]
    fn observer_suppressed_during_remote_apply() {
        let doc = simple_doc();
        let bytes = yrs_bridge::doc_to_ydoc_bytes(&doc);
        let client = CollabClient::new("doc1".into(), Some(&bytes));

        // Simulate remote apply: set flag, then modify ydoc directly
        client.is_applying_remote.set(true);
        {
            let ydoc = client.ydoc.borrow();
            let modified = Node::element_with_content(
                NodeType::Doc,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Remote change")]),
                )]),
            );
            yrs_bridge::sync_model_to_ydoc(&ydoc, &modified);
        }
        client.is_applying_remote.set(false);

        // Observer should NOT have captured the remote change
        assert!(client.pending_updates.borrow().is_empty(),
            "observer should be suppressed during remote apply");
    }

    #[test]
    fn observer_captures_local_edit() {
        let doc = simple_doc();
        let bytes = yrs_bridge::doc_to_ydoc_bytes(&doc);
        let client = CollabClient::new("doc1".into(), Some(&bytes));

        // is_applying_remote is false (default) — local edit
        {
            let ydoc = client.ydoc.borrow();
            let modified = Node::element_with_content(
                NodeType::Doc,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Local change")]),
                )]),
            );
            yrs_bridge::sync_model_to_ydoc(&ydoc, &modified);
        }

        // Observer SHOULD have captured the local change
        assert!(!client.pending_updates.borrow().is_empty(),
            "observer should capture local edits");
    }

    // ── Disconnect ──

    #[test]
    fn disconnect_sets_state() {
        let client = CollabClient::new("doc1".into(), None);
        *client.state.borrow_mut() = ConnectionState::Connected;
        client.disconnect();
        assert_eq!(*client.state.borrow(), ConnectionState::Disconnected);
        assert!(client.ws.borrow().is_none());
    }

    // ── AwarenessPayload serialization ──

    #[test]
    fn awareness_payload_roundtrip() {
        let payload = AwarenessPayload {
            user_id: "user1".into(),
            name: "Alice".into(),
            color: 3,
            cursor_pos: Some(42),
            selection_anchor: Some(10),
            selection_head: Some(20),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"user_id\":\"user1\""));
        assert!(json.contains("\"cursor_pos\":42"));
        let back: AwarenessPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.user_id, "user1");
        assert_eq!(back.cursor_pos, Some(42));
    }

    #[test]
    fn awareness_payload_optional_fields_omitted() {
        let payload = AwarenessPayload {
            user_id: "user1".into(),
            name: "Alice".into(),
            color: 0,
            cursor_pos: None,
            selection_anchor: None,
            selection_head: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("cursor_pos"), "None fields should be omitted: {json}");
        assert!(!json.contains("selection_anchor"), "None fields should be omitted: {json}");
    }
}
