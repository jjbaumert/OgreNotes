//! WebSocket collaboration client for real-time document sync.
//!
//! Connects to the server's WebSocket endpoint, handles the yrs sync protocol,
//! and bridges between the editor's Transaction system and yrs incremental updates.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

use yrs::{Doc, ReadTxn, StateVector, Transact, Update, WriteTxn};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;

use crate::editor::model::Node;
use crate::editor::yrs_bridge;

// Protocol constants (must match crates/collab/src/protocol.rs)
const MSG_AUTH: u8 = 0x00;
const MSG_SYNC_STEP1: u8 = 0x01;
const MSG_SYNC_STEP2: u8 = 0x02;
const MSG_UPDATE: u8 = 0x03;
const MSG_AWARENESS: u8 = 0x04;
const MSG_ERROR: u8 = 0xFF;

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

/// WebSocket collaboration client.
/// Maintains a yrs Doc for incremental sync and bridges to the editor model.
pub struct CollabClient {
    /// WebSocket connection (None if disconnected).
    ws: Rc<RefCell<Option<WebSocket>>>,
    /// Connection state.
    state: Rc<RefCell<ConnectionState>>,
    /// The yrs Doc that accumulates all updates (local and remote).
    ydoc: Rc<RefCell<yrs::Doc>>,
    /// Document ID.
    doc_id: String,
    /// Callback when remote update changes the document.
    on_remote_update: Rc<RefCell<Option<OnRemoteUpdate>>>,
    /// Stored closures (prevent GC).
    _closures: Rc<RefCell<Vec<Closure<dyn Fn(web_sys::Event)>>>>,
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

        Self {
            ws: Rc::new(RefCell::new(None)),
            state: Rc::new(RefCell::new(ConnectionState::Disconnected)),
            ydoc: Rc::new(RefCell::new(ydoc)),
            doc_id,
            on_remote_update: Rc::new(RefCell::new(None)),
            _closures: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Set the callback for remote document updates.
    pub fn set_on_remote_update(&self, callback: OnRemoteUpdate) {
        *self.on_remote_update.borrow_mut() = Some(callback);
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

        let token = token.to_string();
        let state = Rc::clone(&self.state);
        let ydoc = Rc::clone(&self.ydoc);
        let ws_ref = Rc::clone(&self.ws);
        let on_remote = Rc::clone(&self.on_remote_update);
        let closures = Rc::clone(&self._closures);

        // onopen: mark connected (auth is via URL query param)
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
        let flag_for_msg = std::sync::Arc::clone(&connected_flag);
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
                        let mut ydoc = ydoc_for_msg.borrow_mut();
                        let mut txn = ydoc.transact_mut();
                        if let Ok(update) = yrs::Update::decode_v1(payload) {
                            let _ = txn.apply_update(update);
                        }
                    }
                    *state_for_msg.borrow_mut() = ConnectionState::Synced;
                    flag_for_msg.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                MSG_UPDATE => {
                    // Remote incremental update — apply to our yrs Doc
                    crate::editor::debug::log("collab", "received update", &[
                        ("size", &payload.len().to_string()),
                    ]);
                    let mut ydoc = ydoc_for_msg.borrow_mut();
                    let mut txn = ydoc.transact_mut();
                    if let Ok(update) = yrs::Update::decode_v1(payload) {
                        let _ = txn.apply_update(update);
                    }
                    drop(txn);

                    // Convert the updated yrs doc to editor model
                    let txn = ydoc.transact();
                    let state_bytes = txn.encode_state_as_update_v1(&yrs::StateVector::default());
                    drop(txn);
                    drop(ydoc);

                    if let Ok(doc) = yrs_bridge::ydoc_bytes_to_doc(&state_bytes) {
                        if let Some(callback) = on_remote_for_msg.borrow().as_ref() {
                            callback(doc);
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

    /// Send a local editor change as a yrs incremental update.
    /// Called by the editor dispatch when the document changes.
    pub fn send_update(&self, new_doc: &Node) {
        if *self.state.borrow() != ConnectionState::Synced {
            return;
        }

        // Convert the full editor model to yrs bytes
        let new_bytes = yrs_bridge::doc_to_ydoc_bytes(new_doc);

        // Create a fresh yrs Doc from the new state, compute diff against our stored doc
        let new_ydoc = yrs::Doc::new();
        {
            let mut txn = new_ydoc.transact_mut();
            if let Ok(update) = yrs::Update::decode_v1(&new_bytes) {
                let _ = txn.apply_update(update);
            }
        }

        // Compute what changed: encode the new state as an update from our current state vector
        let ydoc = self.ydoc.borrow();
        let our_sv = ydoc.transact().state_vector();
        let diff = new_ydoc.transact().encode_state_as_update_v1(&our_sv);
        drop(ydoc);

        // Apply the diff to our stored yrs doc to keep it in sync
        {
            let mut ydoc = self.ydoc.borrow_mut();
            let mut txn = ydoc.transact_mut();
            if let Ok(update) = yrs::Update::decode_v1(&diff) {
                let _ = txn.apply_update(update);
            }
        }

        // Send the diff over WebSocket
        if !diff.is_empty() {
            if let Some(ws) = self.ws.borrow().as_ref() {
                let mut msg = vec![MSG_UPDATE];
                msg.extend_from_slice(&diff);
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
