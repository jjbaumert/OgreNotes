// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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
const MSG_PING: u8 = 0x05;
const MSG_COMMENT_EVENT: u8 = 0x06;
const MSG_SUBSCRIBE_FOREIGN_DOC: u8 = 0x07;
const MSG_UNSUBSCRIBE_FOREIGN_DOC: u8 = 0x08;
const MSG_FOREIGN_DOC_UPDATE: u8 = 0x09;
const MSG_AWARENESS_LEAVE: u8 = 0x0A;
const MSG_ERROR: u8 = 0xFF;

/// How often the client sends an application-level Ping while the user is
/// considered active. Must be < the AWS ALB `idle_timeout.timeout_seconds`
/// (we set it to 120s in aws-test-deploy.sh; AWS default is 60s) so the
/// load balancer never sees a 60s+ silence on a "live" session.
pub const PING_INTERVAL_MS: u32 = 25_000;

/// After this much idle time with no user activity, the client stops
/// sending pings and deliberately closes the WebSocket. The session
/// reconnects on next user activity (the activity tracker in
/// pages/document.rs owns that path; CollabClient itself does not
/// auto-reconnect from `onclose`).
pub const IDLE_DISCONNECT_MS: f64 = 30.0 * 60.0 * 1000.0;

/// Pure predicate: "is now within the active window relative to last
/// activity?" Extracted so it can be unit-tested without a browser.
pub fn should_heartbeat(now_ms: f64, last_activity_at_ms: f64, idle_window_ms: f64) -> bool {
    now_ms - last_activity_at_ms < idle_window_ms
}

/// Wall-clock-ish "now" in milliseconds. Uses `performance.now()` so it
/// is monotonic across the page's lifetime; absolute values are not
/// meaningful, only differences. Returns 0.0 if no window is available
/// (test contexts).
fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

/// Color palette for collaborator cursors (must match backend).
const CURSOR_COLORS: [&str; 12] = [
    "#E57373", "#64B5F6", "#81C784", "#FFB74D",
    "#BA68C8", "#4DD0E1", "#F06292", "#AED581",
    "#FFD54F", "#7986CB", "#4DB6AC", "#A1887F",
];

/// JSON payload for awareness messages (matches backend AwarenessState).
/// Uses block-relative positions (block_id + character offset within the block)
/// instead of absolute model positions, because absolute positions are not
/// portable between clients with different DOM structures.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AwarenessPayload {
    user_id: String,
    name: String,
    color: u8,
    /// Cursor position: block ID + character offset within the block.
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor_block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor_offset: Option<u32>,
    /// Selection anchor (start of selection).
    #[serde(skip_serializing_if = "Option::is_none")]
    sel_anchor_block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sel_anchor_offset: Option<u32>,
    /// Selection head (end of selection).
    #[serde(skip_serializing_if = "Option::is_none")]
    sel_head_block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sel_head_offset: Option<u32>,
    // Legacy fields (ignored on receive, kept for backwards compat during rollout)
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor_pos: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_anchor: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_head: Option<u32>,
    // Declared so serde preserves the field on pass-through decode/encode
    // even though the frontend doesn't currently render typing indicators
    // from awareness. Keeps the frontend in symmetry with the backend
    // `AwarenessState` so golden-fixture tests on both sides pass the same
    // JSON unchanged — see `tests/fixtures/protocol/awareness/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    typing_thread_id: Option<String>,
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

/// #92: provides the editor's CURRENT model on demand, so the recv path
/// can fold not-yet-sent local keystrokes into the ydoc before applying
/// a remote update. Registered by the page after client construction;
/// returns None when no editor state is available (e.g. mid-teardown).
pub type LocalDocProvider = Box<dyn Fn() -> Option<Node>>;

/// Remote user's cursor/selection state for presence rendering.
/// Uses block-relative positions for cross-client portability.
#[derive(Debug, Clone)]
pub struct RemoteCursor {
    pub user_id: String,
    pub name: String,
    pub color: String,
    /// Cursor position: (block_id, char_offset_within_block)
    pub cursor_block: Option<(String, u32)>,
    /// Selection anchor: (block_id, char_offset_within_block)
    pub selection_anchor_block: Option<(String, u32)>,
    /// Selection head: (block_id, char_offset_within_block)
    pub selection_head_block: Option<(String, u32)>,
    /// Comment thread the user is currently typing into, if any.
    /// Drives the "X is typing…" indicator in the conversation pane.
    pub typing_thread_id: Option<String>,
}

/// Callback for when remote cursors change.
pub type OnAwarenessUpdate = Box<dyn Fn(Vec<RemoteCursor>)>;

/// Callback fired when a peer creates / updates / replies to a comment
/// thread on this document. The payload is the JSON object emitted by the
/// server's `fanout_comment_event` (e.g.
/// `{"kind":"thread_created","threadId":"…"}`); the page-level handler
/// typically reacts by refetching the thread list. Errors parsing the
/// payload aren't surfaced — the callback receives the raw JSON so it
/// can decide whether finer-grained handling is worthwhile.
pub type OnCommentEvent = Box<dyn Fn(String)>;

/// Callback signature for `MSG_FOREIGN_DOC_UPDATE` frames. The
/// argument is the foreign doc id whose CRDT advanced; the page-
/// level handler invalidates the spreadsheet engine's foreign-doc
/// cache for that id so the next recompute re-fetches via HTTP.
/// (v1 doesn't apply the incremental update bytes directly — that
/// would require maintaining per-foreign-doc yrs::Doc sidecars in
/// the frontend; the push is treated as a "stale, please refetch"
/// signal instead.)
pub type OnForeignDocUpdate = Box<dyn Fn(String)>;

/// Callback signature for `MSG_ERROR` frames carrying the
/// `liveapp-rejected:` prefix. The argument is the server-side
/// diagnostic (e.g. `"kanban_card: title: value was
/// canonicalized: input \"...\", canonical \"...\""`). The
/// page-level handler shows a toast — the user's local yrs
/// still holds the rejected write, so the toast is the only
/// signal they get that the change didn't reach the server.
///
/// Server side is Phase 2a Option A of finding #5. A follow-up
/// (tracked in the ticket referenced in the ws.rs err handler)
/// will add server-triggered state recovery so the client's
/// local view converges back to the server's authoritative
/// state without a manual refresh.
pub type OnLiveAppError = Box<dyn Fn(String)>;

// ─── Message handling helpers ──────────────────────────────────

/// Wrap a closure as a `Closure<dyn Fn(web_sys::Event)>`.
fn wrap_event(f: impl Fn(web_sys::Event) + 'static) -> Closure<dyn Fn(web_sys::Event)> {
    Closure::wrap(Box::new(f) as Box<dyn Fn(web_sys::Event)>)
}

/// Get the total model position count from the editor DOM.
/// Used to bounds-check remote cursor positions.
fn get_editor_doc_length() -> usize {
    let Some(window) = web_sys::window() else { return 0 };
    let Some(document) = window.document() else { return 0 };
    let Some(editor) = document.query_selector(".editor-content").ok().flatten() else { return 0 };
    // The text content length is a rough approximation of the model position count.
    // The actual model uses open/close boundary tokens for block elements, so the
    // real count is higher. We use 2x the character count as a generous upper bound.
    // (chars().count() not .len() — .len() is bytes, which overcounts for non-ASCII)
    let text_len = editor.text_content().unwrap_or_default().chars().count();
    text_len * 2
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

/// Drain `pending_updates` over `ws_ref` as a series of `MSG_UPDATE`
/// frames. Shared by the normal send_update path and the SyncStep2
/// handler that flushes anything buffered during the sync handshake.
/// Silently no-ops on missing socket or empty buffer — caller decides
/// whether to log.
fn flush_pending_updates_to_socket(
    pending_updates: &Rc<RefCell<Vec<Vec<u8>>>>,
    ws_ref: &Rc<RefCell<Option<WebSocket>>>,
) {
    let updates: Vec<Vec<u8>> = pending_updates.borrow_mut().drain(..).collect();
    if updates.is_empty() {
        crate::editor::debug::log("collab", "flush: no pending updates", &[]);
        return;
    }
    // Phase 1 obs — fires once per drain that actually had bytes to
    // send. A high COLLAB_PENDING_DRAINED with a low WS_FRAMES_SENT
    // ratio means drains run but the socket rejects sends — the
    // signal we want for the current bug class.
    crate::observability::inc(crate::observability::COLLAB_PENDING_DRAINED);
    let Some(ws) = ws_ref.borrow().as_ref().cloned() else {
        // Lost the socket between buffering and flushing — leave the
        // updates dropped (no socket to send on). Reconnect-on-sync
        // is a separate code path; this just protects against panic.
        // Counted as a send error rather than a sent-frame because
        // the user's bytes never reached the wire.
        crate::observability::add(
            crate::observability::WS_SEND_ERRORS,
            updates.len() as u64,
        );
        crate::editor::debug::log("collab", "flush: no socket, dropping", &[
            ("count", &updates.len().to_string()),
        ]);
        return;
    };
    for update_bytes in &updates {
        crate::editor::debug::log("collab", "flushing buffered update", &[
            ("size", &update_bytes.len().to_string()),
        ]);
        let mut msg = vec![MSG_UPDATE];
        msg.extend_from_slice(update_bytes);
        match ws.send_with_u8_array(&msg) {
            Ok(()) => crate::observability::inc(
                crate::observability::WS_FRAMES_SENT,
            ),
            Err(_) => crate::observability::inc(
                crate::observability::WS_SEND_ERRORS,
            ),
        }
    }
}

/// #92: fold the editor's current model into the ydoc BEFORE a remote
/// update is applied. A keystroke lives only in the editor model for up
/// to the send debounce; a remote update landing inside that window
/// would otherwise rebuild the view from a ydoc that has never seen the
/// keystroke — silently dropping it (the "first keystrokes after mount"
/// bug). Folding first turns the subsequent apply into a true CRDT
/// merge. Ordering matters: folding AFTER the apply treats remote
/// content the stale model has never seen as local deletions — see the
/// `fold_after_remote_apply_deletes_peer_content` tripwire test in
/// yrs_bridge.
///
/// No-op without a provider (page registered none) or a baseline (a
/// full-doc fold is exactly the unsafe overwrite we're avoiding; the
/// baseline is initialized at construction so this shouldn't occur).
/// The yrs observer captures the fold's bytes into `pending_updates`,
/// so folded keystrokes also reach the server via the flush paths.
fn fold_local_before_remote(
    ydoc: &Rc<RefCell<yrs::Doc>>,
    provider: &Rc<RefCell<Option<LocalDocProvider>>>,
    last_synced: &Rc<RefCell<Option<Node>>>,
) {
    let Some(model) = provider.borrow().as_ref().and_then(|p| p()) else {
        return;
    };
    let Some(baseline) = last_synced.borrow().clone() else {
        return;
    };
    let normalized = {
        let ydoc = ydoc.borrow();
        yrs_bridge::sync_model_to_ydoc_diffed(&ydoc, &model, Some(&baseline))
    };
    *last_synced.borrow_mut() = Some(normalized);
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
    last_synced: &Rc<RefCell<Option<Node>>>,
    provider: &Rc<RefCell<Option<LocalDocProvider>>>,
    pending_updates: &Rc<RefCell<Vec<Vec<u8>>>>,
    ws_ref: &Rc<RefCell<Option<WebSocket>>>,
    state: &Rc<RefCell<ConnectionState>>,
) {
    let ydoc_ref = ydoc.clone();
    let on_remote_ref = on_remote.clone();
    let last_synced_ref = last_synced.clone();
    let provider_ref = provider.clone();
    let pending_ref = pending_updates.clone();
    let ws_ref = ws_ref.clone();
    let state_ref = state.clone();
    *timer.borrow_mut() = Some(gloo_timers::callback::Timeout::new(
        WS_RECV_DEBOUNCE_MS,
        move || {
            let doc = {
                let ydoc = ydoc_ref.borrow();
                let Ok(mut doc) = yrs_bridge::read_doc_from_ydoc(&ydoc) else {
                    return;
                };
                // #92 (swap-window half): keystrokes typed between the
                // frame's apply and THIS debounced read exist only in the
                // editor model — the swap below would clobber them. Fold
                // them in first, but only when the model and the
                // post-merge ydoc agree on top-level structure: with the
                // lists equal, reconciliation is per-block (blocks the
                // user didn't touch are skipped by the equality check and
                // keep their remote content). When a remote STRUCTURAL
                // change landed, folding a model that predates it would
                // delete the new block (the tripwire test) — skip, accept
                // the legacy swap for that rare concurrent case.
                let folded = (|| {
                    let model = provider_ref.borrow().as_ref().and_then(|p| p())?;
                    let baseline = last_synced_ref.borrow().clone()?;
                    if model == baseline {
                        return None; // nothing pending — plain swap
                    }
                    if !yrs_bridge::same_top_level_block_ids(&model, &doc) {
                        return None; // remote structural change — unsafe
                    }
                    yrs_bridge::sync_model_to_ydoc_diffed(&ydoc, &model, Some(&baseline));
                    yrs_bridge::read_doc_from_ydoc(&ydoc).ok()
                })();
                if let Some(refolded) = folded {
                    doc = refolded;
                }
                doc
            };
            // The fold's bytes (if any) must reach the server as well.
            if *state_ref.borrow() == ConnectionState::Synced {
                flush_pending_updates_to_socket(&pending_ref, &ws_ref);
            }
            // #121: the ydoc just changed under us — rebase the
            // diff-sync baseline on its post-merge content so the
            // next local sync's skip decisions stay aligned with
            // what the ydoc actually holds. `read_doc_from_ydoc`
            // already returns a normalized doc — the same form
            // sync_model_to_ydoc_diffed caches on the local path —
            // so store it directly (re-normalizing would be O(doc)
            // waste and a drift hazard if the two paths diverged).
            *last_synced_ref.borrow_mut() = Some(doc.clone());
            if let Some(callback) = on_remote_ref.borrow().as_ref() {
                callback(doc);
            }
        },
    ));
}

/// Handle MSG_AWARENESS: update remote cursor state and notify callback.
/// Validates that cursor/selection positions are within the current document bounds
/// to prevent misplaced overlays when documents are temporarily out of sync.
fn handle_awareness(
    payload: &[u8],
    remote_cursors: &Rc<RefCell<std::collections::HashMap<String, RemoteCursor>>>,
    on_awareness: &Rc<RefCell<Option<OnAwarenessUpdate>>>,
) {
    let Ok(state) = serde_json::from_slice::<AwarenessPayload>(payload) else { return };

    let block_pos = |bid: &Option<String>, off: &Option<u32>| -> Option<(String, u32)> {
        Some((bid.as_ref()?.clone(), (*off)?))
    };

    let color_idx = (state.color as usize) % CURSOR_COLORS.len();
    let cursor = RemoteCursor {
        user_id: state.user_id.clone(),
        name: state.name.clone(),
        color: CURSOR_COLORS[color_idx].to_string(),
        cursor_block: block_pos(&state.cursor_block_id, &state.cursor_offset),
        selection_anchor_block: block_pos(&state.sel_anchor_block_id, &state.sel_anchor_offset),
        selection_head_block: block_pos(&state.sel_head_block_id, &state.sel_head_offset),
        typing_thread_id: state.typing_thread_id.clone(),
    };
    remote_cursors.borrow_mut().insert(state.user_id, cursor);

    if let Some(callback) = on_awareness.borrow().as_ref() {
        let cursors: Vec<RemoteCursor> = remote_cursors.borrow().values().cloned().collect();
        callback(cursors);
    }
}

/// Handle MSG_AWARENESS_LEAVE: a peer disconnected, so drop their cursor.
/// Payload is the departing user's id (UTF-8). Without this the cursor
/// would stay frozen at its last position until the local user refreshes
/// (#9). No-op if we weren't tracking that user; only fires the callback
/// when something was actually removed.
fn handle_awareness_leave(
    payload: &[u8],
    remote_cursors: &Rc<RefCell<std::collections::HashMap<String, RemoteCursor>>>,
    on_awareness: &Rc<RefCell<Option<OnAwarenessUpdate>>>,
) {
    let Ok(user_id) = std::str::from_utf8(payload) else { return };
    let removed = remote_cursors.borrow_mut().remove(user_id).is_some();
    if !removed {
        return;
    }
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
    /// Callback when a peer changes a comment thread on this doc.
    on_comment_event: Rc<RefCell<Option<OnCommentEvent>>>,
    /// Callback when a foreign doc subscribed via `subscribe_foreign_doc`
    /// pushes an update.
    on_foreign_doc_update: Rc<RefCell<Option<OnForeignDocUpdate>>>,
    /// Callback for `liveapp-rejected:` MSG_ERROR frames — see
    /// `OnLiveAppError`. Absent → the frame is still logged to
    /// the console but no user-facing toast fires.
    on_liveapp_error: Rc<RefCell<Option<OnLiveAppError>>>,
    /// #92: on-demand access to the editor's current model, used to fold
    /// keystrokes still inside the send-debounce window into the ydoc
    /// before a remote update is applied (merge, don't clobber).
    local_doc_provider: Rc<RefCell<Option<LocalDocProvider>>>,
    /// Remote user awareness states.
    remote_cursors: Rc<RefCell<std::collections::HashMap<String, RemoteCursor>>>,
    /// Stored closures (prevent GC).
    _closures: Rc<RefCell<Vec<Closure<dyn Fn(web_sys::Event)>>>>,
    /// Incremental updates queued by observe_update_v1 for sending.
    pending_updates: Rc<RefCell<Vec<Vec<u8>>>>,
    /// The normalized model doc as of the last ydoc sync in either
    /// direction (#121): set from `sync_model_to_ydoc_diffed`'s return
    /// after a local send, refreshed from `read_doc_from_ydoc` after a
    /// remote update lands. Lets the next local sync skip unchanged
    /// subtrees instead of walking the whole doc through yrs reads.
    last_synced_doc: Rc<RefCell<Option<Node>>>,
    /// Flag to suppress observer when applying remote updates.
    is_applying_remote: Rc<Cell<bool>>,
    /// Subscription for observe_update_v1 (must stay alive).
    _update_sub: yrs::Subscription,
    /// Wall-clock millis (`performance.now()`-relative) of last user
    /// activity. Drives the heartbeat: while `now - last_activity_at <
    /// IDLE_DISCONNECT_MS`, we keep sending Pings; past that, we let the
    /// connection drop and only resume when activity resumes.
    last_activity_at: Rc<Cell<f64>>,
    /// Heartbeat interval — Some while the WebSocket is open. Cleared on
    /// `onclose` and `disconnect()` so the timer can't outlive the WS.
    heartbeat_handle: Rc<RefCell<Option<gloo_timers::callback::Interval>>>,
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
                // Phase 1 obs — paired with EDITOR_TRANSACTIONS.
                // Divergence between the two surfaces "model
                // changed but yrs saw no diff" cases.
                crate::observability::inc(crate::observability::COLLAB_OBSERVE_FIRED);
                pending_ref.borrow_mut().push(event.update.clone());
            }
        }).expect("observe_update_v1 should not fail on a fresh Doc");

        // #92/#121: the diff-sync baseline starts as the content the editor
        // mounted with — read back from the just-initialized ydoc — rather
        // than None. With a None baseline the first fold/send does a
        // full-doc sync, which (per sync_model_to_ydoc_diffed's contract)
        // can overwrite content a concurrent remote update contributed.
        let last_synced_doc = Rc::new(RefCell::new(
            yrs_bridge::read_doc_from_ydoc(&ydoc).ok(),
        ));

        Self {
            ws: Rc::new(RefCell::new(None)),
            state: Rc::new(RefCell::new(ConnectionState::Disconnected)),
            ydoc: Rc::new(RefCell::new(ydoc)),
            doc_id,
            on_remote_update: Rc::new(RefCell::new(None)),
            on_awareness_update: Rc::new(RefCell::new(None)),
            on_comment_event: Rc::new(RefCell::new(None)),
            on_foreign_doc_update: Rc::new(RefCell::new(None)),
            on_liveapp_error: Rc::new(RefCell::new(None)),
            remote_cursors: Rc::new(RefCell::new(std::collections::HashMap::new())),
            _closures: Rc::new(RefCell::new(Vec::new())),
            pending_updates,
            last_synced_doc,
            local_doc_provider: Rc::new(RefCell::new(None)),
            is_applying_remote,
            _update_sub: update_sub,
            last_activity_at: Rc::new(Cell::new(0.0)),
            heartbeat_handle: Rc::new(RefCell::new(None)),
        }
    }

    /// Record that the user is "present" right now — keystroke, mouse
    /// movement over the doc, the tab becoming visible, window regaining
    /// focus. Resets the idle clock so the next heartbeat tick will keep
    /// the connection warm. Does NOT initiate reconnection — the page-
    /// level activity tracker decides when to call `connect` again on a
    /// closed session.
    pub fn record_activity(&self) {
        self.last_activity_at.set(now_ms());
    }

    /// Returns true if the WebSocket is open (Connecting / Connected /
    /// Synced). Used by the page activity tracker to decide whether an
    /// activity event should reconnect.
    pub fn is_connected(&self) -> bool {
        *self.state.borrow() != ConnectionState::Disconnected
    }

    /// Set the callback for remote document updates.
    /// #92: register the editor-model provider used to fold unsent local
    /// keystrokes into the ydoc before each remote update is applied.
    pub fn set_local_doc_provider(&self, provider: LocalDocProvider) {
        *self.local_doc_provider.borrow_mut() = Some(provider);
    }

    pub fn set_on_remote_update(&self, callback: OnRemoteUpdate) {
        *self.on_remote_update.borrow_mut() = Some(callback);
    }

    /// Set the callback for remote awareness updates (cursor presence).
    pub fn set_on_awareness_update(&self, callback: OnAwarenessUpdate) {
        *self.on_awareness_update.borrow_mut() = Some(callback);
    }

    /// Set the callback that fires when a peer creates, updates, or replies
    /// to a comment thread on this document. Pass `None` to unset.
    pub fn set_on_comment_event(&self, callback: OnCommentEvent) {
        *self.on_comment_event.borrow_mut() = Some(callback);
    }

    /// Set the callback that fires when a foreign doc subscribed via
    /// `subscribe_foreign_doc` pushes an update. The argument is the
    /// foreign doc id whose CRDT advanced; the callback is responsible
    /// for invalidating any cached projection of that doc and
    /// refetching.
    pub fn set_on_foreign_doc_update(&self, callback: OnForeignDocUpdate) {
        *self.on_foreign_doc_update.borrow_mut() = Some(callback);
    }

    /// Set the callback that fires on `liveapp-rejected:` MSG_ERROR
    /// frames. The document page hooks a toast setter here — the
    /// user's local yrs still holds the rejected write, so this
    /// callback is the only in-session signal that the change
    /// didn't reach the server.
    pub fn set_on_liveapp_error(&self, callback: OnLiveAppError) {
        *self.on_liveapp_error.borrow_mut() = Some(callback);
    }

    /// Subscribe this connection to live updates for `foreign_doc_id`.
    /// No-op if the WebSocket isn't open. The server gates each
    /// subscribe behind a per-id `View` access check; on denial the
    /// server replies with `MSG_ERROR("foreign-doc-subscribe-denied:<id>")`.
    pub fn subscribe_foreign_doc(&self, foreign_doc_id: &str) {
        let Some(ws) = self.ws.borrow().clone() else { return };
        if ws.ready_state() != web_sys::WebSocket::OPEN { return }
        let mut frame = vec![MSG_SUBSCRIBE_FOREIGN_DOC];
        frame.extend_from_slice(foreign_doc_id.as_bytes());
        let _ = ws.send_with_u8_array(&frame);
    }

    /// Stop receiving updates for `foreign_doc_id`. No-op if the
    /// connection isn't open or the id was never subscribed.
    pub fn unsubscribe_foreign_doc(&self, foreign_doc_id: &str) {
        let Some(ws) = self.ws.borrow().clone() else { return };
        if ws.ready_state() != web_sys::WebSocket::OPEN { return }
        let mut frame = vec![MSG_UNSUBSCRIBE_FOREIGN_DOC];
        frame.extend_from_slice(foreign_doc_id.as_bytes());
        let _ = ws.send_with_u8_array(&frame);
    }

    /// Get the current connection state.
    pub fn connection_state(&self) -> ConnectionState {
        *self.state.borrow()
    }

    /// Number of local CRDT updates queued for send but not yet
    /// flushed to the server. Drives the SyncIndicator's
    /// "Offline — N pending" copy. A non-zero value with state
    /// `Synced` is normal mid-keystroke — `flush_pending` moves it
    /// to zero within a tick or two.
    pub fn pending_count(&self) -> usize {
        self.pending_updates.borrow().len()
    }

    /// Connect to the WebSocket server.
    /// `connected_flag` is set to true when synced, false on disconnect.
    pub fn connect(&self, ws_url: &str, token: &str, connected_flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        *self.state.borrow_mut() = ConnectionState::Connecting;
        // Reconnect path: a stale interval from a prior session must not
        // try to send pings on the new socket once it's spliced in.
        self.heartbeat_handle.borrow_mut().take();
        // Treat the act of connecting as activity so the very first 25s
        // window starts fresh — otherwise an old, stale last_activity_at
        // could make the first heartbeat tick decide to disconnect.
        self.last_activity_at.set(now_ms());

        // #2: on reconnect, tear down the previous socket before installing
        // new handlers, then drop its Closures — otherwise each reconnect
        // leaked four JS callbacks. Detach the old socket's handlers and
        // close it FIRST so the subsequent `_closures.clear()` can't drop a
        // Closure the socket might still invoke (the "closure invoked after
        // dropped" panic class).
        if let Some(old_ws) = self.ws.borrow_mut().take() {
            old_ws.set_onopen(None);
            old_ws.set_onmessage(None);
            old_ws.set_onclose(None);
            old_ws.set_onerror(None);
            let _ = old_ws.close();
        }
        self._closures.borrow_mut().clear();

        // #46: carry the auth token in the WebSocket subprotocol instead
        // of the URL query string, so it never lands in browser history
        // or HTTP access logs. We offer two subprotocols: the token
        // (prefixed) and a fixed sentinel; the server selects+echoes the
        // sentinel, so the token doesn't appear in the response either.
        // The strings must match `crates/api/src/routes/ws.rs`.
        let protocols = js_sys::Array::new();
        protocols.push(&wasm_bindgen::JsValue::from_str(&format!(
            "ogrenotes-ws-token.{token}"
        )));
        protocols.push(&wasm_bindgen::JsValue::from_str("ogrenotes-ws"));

        let ws = match WebSocket::new_with_str_sequence(ws_url, protocols.as_ref()) {
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
            let on_comment_event = Rc::clone(&self.on_comment_event);
            let on_foreign_doc_update = Rc::clone(&self.on_foreign_doc_update);
            let on_liveapp_error = Rc::clone(&self.on_liveapp_error);
            let remote_cursors = Rc::clone(&self.remote_cursors);
            // Pending-updates buffer is shared with `send_update`: edits
            // made before sync completes accumulate here, and the
            // SyncStep2 handler flushes them as soon as we reach Synced
            // — fixes the data-loss class where typed edits during the
            // initial-sync window were silently dropped.
            let pending_updates = Rc::clone(&self.pending_updates);
            let flag = std::sync::Arc::clone(&connected_flag);
            let pending_recv_timer: Rc<RefCell<Option<gloo_timers::callback::Timeout>>> =
                Rc::new(RefCell::new(None));
            let last_synced_doc = Rc::clone(&self.last_synced_doc);
            let local_doc_provider = Rc::clone(&self.local_doc_provider);

            wrap_event(move |event: web_sys::Event| {
                let Some(data) = extract_binary_payload(&event) else { return };
                if data.is_empty() { return; }

                // Phase 1 obs — pairs with the server's broadcast
                // counter. A non-zero gap between this and the
                // server-side broadcast suggests the WS frame
                // never reached this client.
                crate::observability::inc(
                    crate::observability::WS_REMOTE_FRAMES_RECEIVED,
                );

                let msg_type = data[0];
                let payload = &data[1..];

                match msg_type {
                    MSG_SYNC_STEP1 => handle_sync_step1(payload, &ydoc, &ws_ref),
                    MSG_SYNC_STEP2 => {
                        // #92: merge, don't clobber — fold keystrokes still
                        // inside the send-debounce window into the ydoc
                        // before applying the server's state.
                        fold_local_before_remote(&ydoc, &local_doc_provider, &last_synced_doc);
                        apply_remote_update(payload, &ydoc, &is_remote);
                        *state.borrow_mut() = ConnectionState::Synced;
                        flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        // #2: notify the editor so it re-renders from the
                        // now-synced ydoc. The initial sync can carry state
                        // the REST snapshot didn't (peers edited between load
                        // and sync); without this the view stays stale until
                        // the next MSG_UPDATE. yrs apply already ran above, so
                        // a no-op diff just re-renders the same content.
                        schedule_remote_callback(
                            &ydoc,
                            &on_remote,
                            &pending_recv_timer,
                            &last_synced_doc,
                            &local_doc_provider,
                            &pending_updates,
                            &ws_ref,
                            &state,
                        );
                        // Flush anything the user typed during the
                        // initial-sync handshake. yrs apply is
                        // idempotent, so even if part of this buffer
                        // was already covered by our outgoing
                        // SyncStep2 the server's reapply is a no-op.
                        flush_pending_updates_to_socket(&pending_updates, &ws_ref);
                    }
                    MSG_UPDATE => {
                        // #92: same fold-before-apply as SyncStep2 — a peer
                        // update landing mid-debounce must merge with, not
                        // clobber, the keystrokes being typed.
                        fold_local_before_remote(&ydoc, &local_doc_provider, &last_synced_doc);
                        apply_remote_update(payload, &ydoc, &is_remote);
                        schedule_remote_callback(
                            &ydoc,
                            &on_remote,
                            &pending_recv_timer,
                            &last_synced_doc,
                            &local_doc_provider,
                            &pending_updates,
                            &ws_ref,
                            &state,
                        );
                        // The fold's bytes must reach the server too;
                        // post-sync nothing else drains the buffer until
                        // the next local edit.
                        if *state.borrow() == ConnectionState::Synced {
                            flush_pending_updates_to_socket(&pending_updates, &ws_ref);
                        }
                    }
                    MSG_AWARENESS => {
                        // Only process remote cursors after our own document is synced.
                        // Before sync, model positions from other clients don't map
                        // correctly to our (potentially stale) DOM.
                        if *state.borrow() == ConnectionState::Synced {
                            handle_awareness(payload, &remote_cursors, &on_awareness);
                        }
                    }
                    MSG_AWARENESS_LEAVE => {
                        // A peer left — drop their cursor immediately rather
                        // than leaving it frozen until refresh (#9). Safe to
                        // process regardless of sync state: removal can only
                        // shrink the overlay, never misplace it.
                        handle_awareness_leave(payload, &remote_cursors, &on_awareness);
                    }
                    MSG_COMMENT_EVENT => {
                        // Comments aren't carried in the CRDT, so the server
                        // sends a side-channel notification when a peer's
                        // REST write changes a thread. Hand the raw JSON to
                        // the page-level callback; it decides what to refetch.
                        if let Ok(json) = std::str::from_utf8(payload) {
                            if let Some(cb) = on_comment_event.borrow().as_ref() {
                                cb(json.to_string());
                            }
                        }
                    }
                    MSG_FOREIGN_DOC_UPDATE => {
                        // Payload: [1-byte id_len, id bytes, update bytes].
                        // v1 ignores the update bytes and treats the message
                        // as a "stale, refetch" signal — the page-level
                        // callback invalidates the spreadsheet engine's
                        // foreign-doc cache for the id and the next
                        // recompute re-fetches via HTTP.
                        if let Some(&id_len) = payload.first() {
                            let id_len = id_len as usize;
                            if payload.len() >= 1 + id_len {
                                if let Ok(id) = std::str::from_utf8(&payload[1..1 + id_len]) {
                                    if let Some(cb) = on_foreign_doc_update.borrow().as_ref() {
                                        cb(id.to_string());
                                    }
                                }
                            }
                        }
                    }
                    MSG_ERROR => {
                        if let Ok(error) = std::str::from_utf8(payload) {
                            web_sys::console::error_1(&format!("WebSocket error: {error}").into());
                            // Phase 2a Option A: liveapp-rejected frames
                            // route to a page-level toast setter so the
                            // user sees a "not saved" signal instead of
                            // only seeing the divergence on refresh.
                            // Prefix keeps the MSG_ERROR channel usable
                            // by future opaque codes (persist-failed
                            // etc.) without them being mistaken for
                            // liveapp signals.
                            if let Some(rest) = error.strip_prefix("liveapp-rejected:") {
                                if let Some(cb) = on_liveapp_error.borrow().as_ref() {
                                    cb(rest.to_string());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            })
        };

        // ── onclose ──
        let state_for_close = Rc::clone(&state);
        let flag_for_close = std::sync::Arc::clone(&connected_flag);
        let heartbeat_for_close = Rc::clone(&self.heartbeat_handle);
        let on_close = wrap_event(move |_| {
            *state_for_close.borrow_mut() = ConnectionState::Disconnected;
            flag_for_close.store(false, std::sync::atomic::Ordering::Relaxed);
            // Drop the heartbeat interval so it can't fire on a dead socket.
            // Reconnect is the page-level activity tracker's job.
            heartbeat_for_close.borrow_mut().take();
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
        drop(cls);

        // ── heartbeat ──
        // Periodic timer that either sends an application-level Ping (the
        // ALB sees traffic, the connection stays warm) or — once the user
        // has been idle past IDLE_DISCONNECT_MS — closes the WebSocket
        // deliberately. The `onclose` handler above clears the handle when
        // that happens, so the interval can't outlive the socket.
        let ws_for_hb = Rc::clone(&self.ws);
        let last_activity_for_hb = Rc::clone(&self.last_activity_at);
        let heartbeat_self = Rc::clone(&self.heartbeat_handle);
        let interval = gloo_timers::callback::Interval::new(PING_INTERVAL_MS, move || {
            let active = should_heartbeat(
                now_ms(),
                last_activity_for_hb.get(),
                IDLE_DISCONNECT_MS,
            );
            let Some(ws) = ws_for_hb.borrow().clone() else { return };
            if active {
                let _ = ws.send_with_u8_array(&[MSG_PING]);
            } else {
                // Idle past the window — let the connection drop. onclose
                // will clear the heartbeat handle so this closure doesn't
                // re-fire on a dead socket.
                let _ = ws.close();
                heartbeat_self.borrow_mut().take();
            }
        });
        *self.heartbeat_handle.borrow_mut() = Some(interval);
    }

    /// Apply a local editor change as an incremental yrs update.
    ///
    /// Always applies the model diff to the persistent Doc via
    /// `sync_model_to_ydoc` — the `observe_update_v1` callback then
    /// captures the incremental bytes into `pending_updates`. The
    /// wire send only happens when we're `Synced`; otherwise the
    /// updates stay buffered and the SyncStep2 handler flushes them
    /// as soon as sync completes. This is the fix for the data-loss
    /// class where edits typed during the initial-sync handshake
    /// (between `Connected` and `Synced`) were silently dropped:
    /// the old behavior early-returned before applying the diff to
    /// ydoc at all, so neither the WS nor the REST autosave saw
    /// those edits.
    pub fn send_update(&self, new_doc: &Node) {
        // Apply model diff to ydoc — the observer captures
        // incremental bytes into `pending_updates`. Doing this
        // unconditionally is the key change: even pre-sync edits now
        // land in the buffer instead of evaporating.
        {
            let ydoc = self.ydoc.borrow();
            // #121: diff against the doc state from the previous sync so
            // a one-cell commit only touches that cell's yrs subtree
            // instead of walking the whole doc. The returned normalized
            // doc becomes the next sync's baseline.
            let mut cache = self.last_synced_doc.borrow_mut();
            let normalized =
                yrs_bridge::sync_model_to_ydoc_diffed(&ydoc, new_doc, cache.as_ref());
            *cache = Some(normalized);
        }

        if *self.state.borrow() != ConnectionState::Synced {
            crate::editor::debug::log(
                "collab",
                "send_update buffered (not synced)",
                &[("pending_len", &self.pending_updates.borrow().len().to_string())],
            );
            return;
        }

        flush_pending_updates_to_socket(&self.pending_updates, &self.ws);
    }

    /// Send local cursor/selection position as an awareness update.
    /// Send local cursor/selection as a block-relative awareness update.
    /// `cursor`: (block_id, char_offset) for cursor position
    /// `sel_anchor`/`sel_head`: (block_id, char_offset) for selection endpoints
    /// `typing_thread_id`: comment thread the user is currently typing into
    pub fn send_awareness(
        &self,
        user_id: &str,
        name: &str,
        color: u8,
        cursor: Option<(&str, u32)>,
        sel_anchor: Option<(&str, u32)>,
        sel_head: Option<(&str, u32)>,
        typing_thread_id: Option<&str>,
    ) {
        if *self.state.borrow() != ConnectionState::Synced {
            return;
        }
        let payload = AwarenessPayload {
            user_id: user_id.to_string(),
            name: name.to_string(),
            color,
            cursor_block_id: cursor.map(|(b, _)| b.to_string()),
            cursor_offset: cursor.map(|(_, o)| o),
            sel_anchor_block_id: sel_anchor.map(|(b, _)| b.to_string()),
            sel_anchor_offset: sel_anchor.map(|(_, o)| o),
            sel_head_block_id: sel_head.map(|(b, _)| b.to_string()),
            sel_head_offset: sel_head.map(|(_, o)| o),
            // Legacy fields (for backwards compat during rollout)
            cursor_pos: None,
            selection_anchor: None,
            selection_head: None,
            typing_thread_id: typing_thread_id.map(|s| s.to_string()),
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
        // Drop the heartbeat first so it can't fire during the close
        // sequence and try to send on a half-shut socket.
        self.heartbeat_handle.borrow_mut().take();
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
    fn send_update_when_not_synced_buffers_for_flush_on_sync() {
        // Behavior contract (was: silently dropped; now: buffered).
        // Edits made before SyncStep2 arrives must land in
        // `pending_updates` so the SyncStep2 handler can flush them
        // — otherwise the user types during the initial handshake
        // and loses content on refresh. Asserts both that the edit
        // reached ydoc (so the next outgoing SyncStep2 carries it)
        // and that the wire-frame bytes are queued in
        // `pending_updates` (for the post-sync flush).
        let doc = simple_doc();
        let bytes = yrs_bridge::doc_to_ydoc_bytes(&doc);
        let client = CollabClient::new("doc1".into(), Some(&bytes));
        assert!(!client.is_synced());

        let modified = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Modified before sync")]),
            )]),
        );
        client.send_update(&modified);

        // The ydoc now carries the edit — encode_state_as_update will
        // include it in the SyncStep2 the client is about to send.
        let ydoc = client.ydoc.borrow();
        let restored = yrs_bridge::read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(restored.text_content(), "Modified before sync");

        // And the incremental-update bytes are buffered for the
        // post-sync flush, so we recover the same edit even if the
        // server applies our SyncStep2 partially.
        assert!(
            !client.pending_updates.borrow().is_empty(),
            "pre-sync edit must be buffered for flush, not dropped",
        );
    }

    #[test]
    fn pre_sync_edits_drain_when_sync_completes() {
        // Regression for the data-loss class fixed by 07147fa:
        //   1. user navigates to a doc
        //   2. WS is in the multi-RTT sync handshake (Connected, not
        //      yet Synced)
        //   3. user types an edit
        //   4. SyncStep2 arrives → handler sets Synced and flushes
        //   5. buffer must be empty afterwards (the edit went to the
        //      wire, or in this unit harness was dropped — either
        //      way, the contract is "no longer pending")
        // Drives the same sequence the MSG_SYNC_STEP2 handler runs.
        let doc = simple_doc();
        let bytes = yrs_bridge::doc_to_ydoc_bytes(&doc);
        let client = CollabClient::new("doc1".into(), Some(&bytes));

        // Step 3 — typed during the handshake window.
        let modified = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("typed during handshake")]),
            )]),
        );
        client.send_update(&modified);
        assert!(
            !client.pending_updates.borrow().is_empty(),
            "edit must accumulate while not-synced (pre-condition)",
        );

        // Step 4 — sync completes. This is the inline body of the
        // MSG_SYNC_STEP2 handler: state ← Synced, then flush.
        *client.state.borrow_mut() = ConnectionState::Synced;
        flush_pending_updates_to_socket(&client.pending_updates, &client.ws);

        // Step 5 — the post-flush invariant. Without the fix, edits
        // never reached `pending_updates` in the first place; with the
        // fix, they reach it on every edit and are drained here.
        assert!(
            client.pending_updates.borrow().is_empty(),
            "post-sync flush must drain the buffer — otherwise edits \
             are silently held until the next typed character",
        );
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
            cursor_block_id: Some("block1".into()),
            cursor_offset: Some(5),
            sel_anchor_block_id: Some("block1".into()),
            sel_anchor_offset: Some(2),
            sel_head_block_id: Some("block1".into()),
            sel_head_offset: Some(10),
            cursor_pos: None,
            selection_anchor: None,
            selection_head: None,
            typing_thread_id: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"user_id\":\"user1\""));
        assert!(json.contains("\"cursor_block_id\":\"block1\""));
        assert!(json.contains("\"cursor_offset\":5"));
        let back: AwarenessPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.user_id, "user1");
        assert_eq!(back.cursor_block_id.as_deref(), Some("block1"));
        assert_eq!(back.cursor_offset, Some(5));
    }

    #[test]
    fn awareness_payload_optional_fields_omitted() {
        let payload = AwarenessPayload {
            user_id: "user1".into(),
            name: "Alice".into(),
            color: 0,
            cursor_block_id: None,
            cursor_offset: None,
            sel_anchor_block_id: None,
            sel_anchor_offset: None,
            sel_head_block_id: None,
            sel_head_offset: None,
            cursor_pos: None,
            selection_anchor: None,
            selection_head: None,
            typing_thread_id: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("cursor_block_id"), "None fields should be omitted: {json}");
        assert!(!json.contains("sel_anchor"), "None fields should be omitted: {json}");
    }

    // ── Awareness departure (#9) ──

    fn test_cursor(user_id: &str) -> RemoteCursor {
        RemoteCursor {
            user_id: user_id.into(),
            name: user_id.into(),
            color: CURSOR_COLORS[0].to_string(),
            cursor_block: Some(("block1".into(), 0)),
            selection_anchor_block: None,
            selection_head_block: None,
            typing_thread_id: None,
        }
    }

    #[test]
    fn awareness_leave_removes_cursor_and_notifies() {
        let cursors = Rc::new(RefCell::new(std::collections::HashMap::new()));
        cursors.borrow_mut().insert("alice".to_string(), test_cursor("alice"));
        cursors.borrow_mut().insert("bob".to_string(), test_cursor("bob"));

        let last_seen: Rc<RefCell<Option<Vec<RemoteCursor>>>> = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&last_seen);
        let on_awareness: Rc<RefCell<Option<OnAwarenessUpdate>>> = Rc::new(RefCell::new(Some(
            Box::new(move |c: Vec<RemoteCursor>| *sink.borrow_mut() = Some(c)),
        )));

        handle_awareness_leave(b"bob", &cursors, &on_awareness);

        assert!(!cursors.borrow().contains_key("bob"), "departed user dropped");
        assert!(cursors.borrow().contains_key("alice"), "other user retained");
        let notified = last_seen.borrow().clone().expect("callback fired");
        assert_eq!(notified.len(), 1);
        assert_eq!(notified[0].user_id, "alice");
    }

    #[test]
    fn awareness_leave_unknown_user_is_noop() {
        let cursors = Rc::new(RefCell::new(std::collections::HashMap::new()));
        cursors.borrow_mut().insert("alice".to_string(), test_cursor("alice"));

        let fired = Rc::new(RefCell::new(false));
        let sink = Rc::clone(&fired);
        let on_awareness: Rc<RefCell<Option<OnAwarenessUpdate>>> = Rc::new(RefCell::new(Some(
            Box::new(move |_: Vec<RemoteCursor>| *sink.borrow_mut() = true),
        )));

        handle_awareness_leave(b"nobody", &cursors, &on_awareness);

        assert_eq!(cursors.borrow().len(), 1, "no cursor removed");
        assert!(!*fired.borrow(), "callback not fired when nothing changed");
    }

    // ── Golden wire-format fixtures (mirrors backend) ──
    //
    // Same fixture files, same contract: this side must round-trip them
    // without dropping a populated field. If the frontend struct falls
    // behind the backend (or vice versa), one of these tests fails before
    // the PR lands. See `tests/fixtures/protocol/awareness/README.md`.

    const FIXTURE_CURSOR_ONLY: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/cursor-only.json");
    const FIXTURE_SELECTION: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/selection.json");
    const FIXTURE_TYPING_INDICATOR: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/typing-indicator.json");
    const FIXTURE_LEGACY_ABSOLUTE: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/legacy-absolute.json");
    const FIXTURE_NO_PRESENCE: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/no-presence.json");

    fn assert_awareness_fixture_round_trips(raw: &str, name: &str) {
        use serde_json::Value;

        let source: Value = serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("{name}: fixture is not valid JSON: {e}"));
        let source_obj = source
            .as_object()
            .unwrap_or_else(|| panic!("{name}: fixture root must be an object"));

        let payload: AwarenessPayload = serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("{name}: frontend rejected the fixture: {e}"));
        let rebroadcast = serde_json::to_string(&payload)
            .unwrap_or_else(|e| panic!("{name}: re-encode failed: {e}"));
        let rebroadcast_value: Value = serde_json::from_str(&rebroadcast)
            .unwrap_or_else(|e| panic!("{name}: rebroadcast bytes are not valid JSON: {e}"));
        let rebroadcast_obj = rebroadcast_value
            .as_object()
            .unwrap_or_else(|| panic!("{name}: rebroadcast root must be an object"));

        for (key, value) in source_obj {
            let rebroadcast_value = rebroadcast_obj.get(key).unwrap_or_else(|| {
                panic!(
                    "{name}: field `{key}` dropped by frontend decode/encode — \
                    AwarenessPayload is missing this field"
                )
            });
            assert_eq!(
                rebroadcast_value, value,
                "{name}: field `{key}` changed across decode/encode"
            );
        }
    }

    #[test]
    fn fixture_cursor_only_preserved() {
        assert_awareness_fixture_round_trips(FIXTURE_CURSOR_ONLY, "cursor-only");
    }

    #[test]
    fn fixture_selection_preserved() {
        assert_awareness_fixture_round_trips(FIXTURE_SELECTION, "selection");
    }

    #[test]
    fn fixture_typing_indicator_preserved() {
        assert_awareness_fixture_round_trips(FIXTURE_TYPING_INDICATOR, "typing-indicator");
    }

    #[test]
    fn fixture_legacy_absolute_preserved() {
        assert_awareness_fixture_round_trips(FIXTURE_LEGACY_ABSOLUTE, "legacy-absolute");
    }

    #[test]
    fn fixture_no_presence_preserved() {
        assert_awareness_fixture_round_trips(FIXTURE_NO_PRESENCE, "no-presence");
    }

    // ── Heartbeat gate predicate ──
    //
    // Pure function — no DOM, no timers — so the regression around the
    // ALB-idle-timeout fix can be guarded by a fast unit test.

    #[test]
    fn should_heartbeat_within_window() {
        // Activity 100ms ago, idle window 30 minutes — keep pinging.
        assert!(should_heartbeat(1_000_000.0, 999_900.0, 30.0 * 60.0 * 1000.0));
    }

    #[test]
    fn should_not_heartbeat_past_window() {
        // Activity 31 minutes ago, idle window 30 minutes — let the
        // connection drop.
        let now = 31.0 * 60.0 * 1000.0;
        assert!(!should_heartbeat(now, 0.0, 30.0 * 60.0 * 1000.0));
    }

    #[test]
    fn should_heartbeat_at_exact_boundary_is_false() {
        // Exact equality is not "within" the window — tightening this
        // way makes the disconnect deterministic at the limit instead of
        // flapping by a sub-millisecond.
        let idle = 30.0 * 60.0 * 1000.0;
        assert!(!should_heartbeat(idle, 0.0, idle));
    }

    #[test]
    fn should_heartbeat_zero_window_never_fires() {
        // Defensive: zero-window config means "no heartbeat at all".
        assert!(!should_heartbeat(100.0, 0.0, 0.0));
    }
}
