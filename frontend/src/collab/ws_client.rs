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
    pub fn connect(&self, ws_url: &str, token: &str, connected_flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
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

        let _token = token.to_string();
        let state = Rc::clone(&self.state);
        let ydoc = Rc::clone(&self.ydoc);
        let ws_ref = Rc::clone(&self.ws);
        let on_remote = Rc::clone(&self.on_remote_update);
        let closures = Rc::clone(&self._closures);
        let is_applying_remote = Rc::clone(&self.is_applying_remote);

        // onopen: mark connected
        let state_for_open = Rc::clone(&state);
        let on_open = Closure::wrap(Box::new(move |_: web_sys::Event| {
            *state_for_open.borrow_mut() = ConnectionState::Connected;
            crate::editor::debug::log("collab", "WebSocket connected", &[]);
        }) as Box<dyn Fn(web_sys::Event)>);

        // onmessage: handle sync protocol
        let state_for_msg = Rc::clone(&state);
        let ydoc_for_msg = Rc::clone(&ydoc);
        let ws_for_msg = Rc::clone(&ws_ref);
        let on_remote_for_msg = Rc::clone(&on_remote);
        let on_awareness_for_msg = Rc::clone(&self.on_awareness_update);
        let remote_cursors_for_msg = Rc::clone(&self.remote_cursors);
        let flag_for_msg = std::sync::Arc::clone(&connected_flag);
        let is_remote_for_msg = Rc::clone(&is_applying_remote);
        let recv_generation = Rc::new(std::cell::Cell::new(0u64));
        let on_message = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(me) = event.dyn_ref::<MessageEvent>() else { return };
            let Ok(buf) = me.data().dyn_into::<js_sys::ArrayBuffer>() else { return };
            let array = js_sys::Uint8Array::new(&buf);
            let data = array.to_vec();

            if data.is_empty() { return; }
            let msg_type = data[0];
            let payload = &data[1..];

            match msg_type {
                MSG_SYNC_STEP1 => {
                    // Server sent its state vector — respond with our diff
                    crate::editor::debug::log("collab", "received SyncStep1", &[]);
                    let ydoc = ydoc_for_msg.borrow();
                    let txn = ydoc.transact();
                    if let Ok(sv) = yrs::StateVector::decode_v1(payload) {
                        let diff = txn.encode_state_as_update_v1(&sv);
                        // Send SyncStep2 with our diff
                        if let Some(ws) = ws_for_msg.borrow().as_ref() {
                            let mut msg = vec![MSG_SYNC_STEP2];
                            msg.extend_from_slice(&diff);
                            let _ = ws.send_with_u8_array(&msg);
                        }

                        // Also send our state vector so the server can send us what we're missing
                        let our_sv = txn.state_vector().encode_v1();
                        if let Some(ws) = ws_for_msg.borrow().as_ref() {
                            let mut msg = vec![MSG_SYNC_STEP1];
                            msg.extend_from_slice(&our_sv);
                            let _ = ws.send_with_u8_array(&msg);
                        }
                    }

                }
                MSG_SYNC_STEP2 => {
                    // Server sent us what we're missing — apply and mark synced
                    crate::editor::debug::log("collab", "received SyncStep2 (synced)", &[
                        ("size", &payload.len().to_string()),
                    ]);
                    {
                        let _guard = RemoteApplyGuard::new(&is_remote_for_msg);
                        let ydoc = ydoc_for_msg.borrow();
                        let mut txn = ydoc.transact_mut();
                        if let Ok(update) = yrs::Update::decode_v1(payload) {
                            let _ = txn.apply_update(update);
                        }
                    }
                    *state_for_msg.borrow_mut() = ConnectionState::Synced;
                    flag_for_msg.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                MSG_UPDATE => {
                    // Remote incremental update — apply to persistent Doc
                    crate::editor::debug::log("collab", "received update", &[
                        ("size", &payload.len().to_string()),
                    ]);
                    {
                        let _guard = RemoteApplyGuard::new(&is_remote_for_msg);
                        let ydoc = ydoc_for_msg.borrow();
                        let mut txn = ydoc.transact_mut();
                        if let Ok(update) = yrs::Update::decode_v1(payload) {
                            let _ = txn.apply_update(update);
                        }
                    }

                    // Debounce the model read-back + callback
                    let recv_gen = recv_generation.get() + 1;
                    recv_generation.set(recv_gen);
                    let gen_ref = recv_generation.clone();
                    let on_remote_ref = on_remote_for_msg.clone();
                    let ydoc_ref = ydoc_for_msg.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        gloo_timers::future::TimeoutFuture::new(WS_RECV_DEBOUNCE_MS).await;
                        if gen_ref.get() != recv_gen {
                            return;
                        }
                        let ydoc = ydoc_ref.borrow();
                        if let Ok(doc) = yrs_bridge::read_doc_from_ydoc(&ydoc) {
                            if let Some(callback) = on_remote_ref.borrow().as_ref() {
                                callback(doc);
                            }
                        }
                    });
                }
                MSG_AWARENESS => {
                    // Remote user's cursor/selection update.
                    if let Ok(state) = serde_json::from_slice::<AwarenessPayload>(payload) {
                        let color_idx = (state.color as usize) % CURSOR_COLORS.len();
                        let cursor = RemoteCursor {
                            user_id: state.user_id.clone(),
                            name: state.name.clone(),
                            color: CURSOR_COLORS[color_idx].to_string(),
                            cursor_pos: state.cursor_pos,
                            selection_anchor: state.selection_anchor,
                            selection_head: state.selection_head,
                        };
                        remote_cursors_for_msg.borrow_mut().insert(state.user_id, cursor);

                        // Notify callback with all remote cursors.
                        if let Some(callback) = on_awareness_for_msg.borrow().as_ref() {
                            let cursors: Vec<RemoteCursor> = remote_cursors_for_msg
                                .borrow()
                                .values()
                                .cloned()
                                .collect();
                            callback(cursors);
                        }
                    }
                }
                MSG_ERROR => {
                    if let Ok(error) = std::str::from_utf8(payload) {
                        web_sys::console::error_1(&format!("WebSocket error: {error}").into());
                    }
                }
                _ => {}
            }
        }) as Box<dyn Fn(web_sys::Event)>);

        // onclose: mark disconnected
        let state_for_close = Rc::clone(&state);
        let flag_for_close = std::sync::Arc::clone(&connected_flag);
        let on_close = Closure::wrap(Box::new(move |_: web_sys::Event| {
            *state_for_close.borrow_mut() = ConnectionState::Disconnected;
            flag_for_close.store(false, std::sync::atomic::Ordering::Relaxed);
            crate::editor::debug::log("collab", "WebSocket disconnected", &[]);
        }) as Box<dyn Fn(web_sys::Event)>);

        // onerror
        let on_error = Closure::wrap(Box::new(move |_: web_sys::Event| {
            web_sys::console::error_1(&"WebSocket error".into());
        }) as Box<dyn Fn(web_sys::Event)>);

        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        *self.ws.borrow_mut() = Some(ws);

        // Store closures to prevent them from being dropped
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
