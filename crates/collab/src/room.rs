// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Per-document collaboration room and room registry.
//!
//! A Room holds the server-side OgreDoc (CRDT state) for a single document,
//! manages connected clients, and handles update broadcasting.
//! The RoomRegistry is a thread-safe map of active rooms.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{mpsc, RwLock};

use ogrenotes_common::metrics::{counter, MetricKey};

use super::document::OgreDoc;
use super::protocol::{encode_message, MessageType};

// LiveApp validation mode moved to `super::blocks` so the block
// crate owns the enum and the shared emit-and-reject helper.
// Re-exported here so downstream callers that already import via
// `ogrenotes_collab::room::LiveAppValidationMode` don't have to
// churn their imports.
pub use super::blocks::LiveAppValidationMode;

/// Report returned by `apply_update_gated` on a successful apply.
/// Currently just the gap-003 deletion list; other post-apply
/// signals fold in here if we grow more.
#[derive(Debug, Default)]
pub struct GatedApplyReport {
    /// LiveApp sub-nodes (KanbanCard / KanbanColumn / CalendarEvent)
    /// that were removed by this write. Empty when the write only
    /// added or modified attributes. The WS handler emits one
    /// `SecurityAuditAction::LiveAppNodeDeleted` per entry.
    pub deletions: Vec<super::blocks::LiveAppDeletion>,
}

/// A handle to a connected WebSocket client within a room.
pub struct ClientHandle {
    pub user_id: String,
    pub sender: mpsc::UnboundedSender<Vec<u8>>,
}

/// A collaboration room for a single document.
pub struct Room {
    /// The server-side CRDT document.
    doc: RwLock<OgreDoc>,
    /// Connected clients, keyed by client_id.
    clients: RwLock<HashMap<u64, ClientHandle>>,
    /// Most recent awareness JSON payload per client.
    ///
    /// Without this, a client that idle-disconnects and reconnects 30 min
    /// later never sees other collaborators' cursors until *they* move,
    /// because the server previously only forwarded live awareness frames
    /// and had no memory of the last broadcast. Snapshot-on-join primes the
    /// rejoining client with everyone's current cursor state.
    awareness: RwLock<HashMap<u64, Vec<u8>>>,
    /// Document ID.
    doc_id: String,
    /// Timestamp of the last edit (milliseconds since epoch).
    last_edit: AtomicU64,
    /// Next client ID counter.
    next_client_id: AtomicU64,
}

impl Room {
    /// Create a new room with the given document state.
    pub fn new(doc_id: String, doc: OgreDoc) -> Self {
        Self {
            doc: RwLock::new(doc),
            clients: RwLock::new(HashMap::new()),
            awareness: RwLock::new(HashMap::new()),
            doc_id,
            last_edit: AtomicU64::new(0),
            next_client_id: AtomicU64::new(1),
        }
    }

    /// Create a room with an empty document.
    pub fn new_empty(doc_id: String) -> Self {
        Self::new(doc_id, OgreDoc::new())
    }

    /// Get the document ID.
    pub fn doc_id(&self) -> &str {
        &self.doc_id
    }

    /// Allocate a new unique client ID for this room.
    pub fn next_client_id(&self) -> u64 {
        self.next_client_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Add a client to the room.
    pub async fn add_client(&self, client_id: u64, user_id: String, sender: mpsc::UnboundedSender<Vec<u8>>) {
        self.clients.write().await.insert(client_id, ClientHandle { user_id, sender });
    }

    /// Remove a client from the room. Returns true if the room is now empty.
    pub async fn remove_client(&self, client_id: u64) -> bool {
        let mut clients = self.clients.write().await;
        clients.remove(&client_id);
        clients.is_empty()
    }

    /// Get the number of connected clients.
    pub async fn client_count(&self) -> usize {
        self.clients.read().await.len()
    }

    /// Number of currently-connected clients belonging to a specific
    /// user. Used by the WebSocket upgrade handler to enforce the
    /// per-user-per-document concurrency cap (#34) — defends against
    /// one compromised user opening unlimited tabs / connections that
    /// each hold awareness state and a broadcast subscription.
    pub async fn client_count_for_user(&self, user_id: &str) -> usize {
        self.clients
            .read()
            .await
            .values()
            .filter(|c| c.user_id == user_id)
            .count()
    }

    /// Get the server's state vector for sync protocol step 1.
    pub async fn state_vector(&self) -> Vec<u8> {
        self.doc.read().await.state_vector()
    }

    /// Compute a diff from a remote state vector (sync step 2).
    pub async fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, super::document::DocError> {
        self.doc.read().await.encode_diff(remote_sv)
    }

    /// Apply an incremental update from a client.
    /// Returns the raw update bytes for broadcasting to other clients.
    pub async fn apply_update(&self, update_bytes: &[u8]) -> Result<(), super::document::DocError> {
        self.doc.write().await.apply_update(update_bytes)?;
        self.last_edit.store(current_time_ms(), Ordering::Relaxed);
        Ok(())
    }

    /// Walk the room's doc and canonicalize any LiveApp attribute
    /// that fails `validate_attrs` or diverges from the block's
    /// canonical form. Returns the collab-level `RepairReport`.
    ///
    /// The write is a single yrs transaction so any observers
    /// (WS peers watching this room's update stream) see it as
    /// one delta and their local docs converge to the repaired
    /// state via normal CRDT sync.
    pub async fn repair_liveapp_attrs(
        &self,
    ) -> super::blocks::RepairReport {
        let doc = self.doc.write().await;
        let report = super::blocks::repair_liveapp_attrs(doc.inner());
        if report.nodes_touched > 0 {
            self.last_edit.store(current_time_ms(), Ordering::Relaxed);
        }
        report
    }

    /// Apply an interactive update, running the LiveApp attribute
    /// gate first. Called by WS `Update` / `SyncStep2` handlers —
    /// NOT by compaction / snapshot restore, which use the raw
    /// `apply_update` because they replay historically-stored
    /// bytes that may pre-date the gate.
    ///
    /// The gate speculatively applies the update to a scratch
    /// clone, walks the resulting tree, and returns violations
    /// if any LiveApp node's attrs fail validation. Behavior on
    /// violation is set by `mode`:
    /// - `Off` — skip the gate entirely.
    /// - `Log` — count violations via a metric, apply anyway.
    /// - `Reject` — count violations and refuse to apply. Returns
    ///   `Err(DocError::LiveAppRejected(...))` — a distinct variant
    ///   from decode/apply failures so the WS `Update` / `SyncStep2`
    ///   handlers can send a `liveapp-rejected:` error frame back to
    ///   the offending client (see `crates/api/src/routes/ws.rs`).
    pub async fn apply_update_gated(
        &self,
        update_bytes: &[u8],
        mode: LiveAppValidationMode,
        scope: super::blocks::WalkScope,
    ) -> Result<GatedApplyReport, super::document::DocError> {
        use super::blocks::{
            collect_liveapp_index, diff_liveapp_deletions,
            emit_violations_and_should_reject, validate_liveapp_writes_scoped,
        };
        // Hold the write lock for the whole check-then-apply so
        // no concurrent update can slip in between validation and
        // the real apply. Doc lock is per-doc, so latency budget
        // is a single-doc concern, not a global one. See the
        // module doc on `blocks::validate_writes` for the cost
        // breakdown — under `Full` scope the walk is O(doc), under
        // `Changed` (gap-001 fix) it is O(touched-elements).
        //
        // Emit the walk-scope metric on every gated write so an
        // operator can verify the config knob is landing.
        counter::inc(MetricKey::new(
            "liveapp.gate_walk_scope",
            &[("scope", match scope {
                super::blocks::WalkScope::Full => "full",
                super::blocks::WalkScope::Changed => "changed",
                super::blocks::WalkScope::Canary => "canary",
            })],
        ));
        let mut doc = self.doc.write().await;
        if mode != LiveAppValidationMode::Off {
            let violations = validate_liveapp_writes_scoped(&doc, update_bytes, scope)
                .err()
                .unwrap_or_default();
            if let Some(first) = emit_violations_and_should_reject(
                &violations,
                mode,
                &[("path", "ws")],
            ) {
                // Distinct DocError variant so the WS handler can
                // send an `MessageType::Error` frame with a
                // `liveapp-rejected:` payload back to the offending
                // client. The frontend keys off the prefix to show
                // a toast — otherwise the client's local yrs has
                // the (invalid) write but the server's authoritative
                // state doesn't, and only a refresh would surface
                // the divergence to the user.
                return Err(super::document::DocError::LiveAppRejected(format!(
                    "{}: {}: {}",
                    first.node_type.tag_name(),
                    first.field,
                    first.reason,
                )));
            }
        }
        // gap-003: snapshot LiveApp indices before + after apply
        // so the WS handler can emit `LiveAppNodeDeleted` audit
        // rows per removed sub-node. Under the write lock so no
        // concurrent apply can race the diff.
        let pre_index = collect_liveapp_index(doc.inner());
        doc.apply_update(update_bytes)?;
        let post_index = collect_liveapp_index(doc.inner());
        let deletions = diff_liveapp_deletions(&pre_index, &post_index);
        self.last_edit.store(current_time_ms(), Ordering::Relaxed);
        Ok(GatedApplyReport { deletions })
    }

    /// Replace the document state entirely (used when clients send full state).
    #[deprecated(note = "Use apply_update with incremental updates instead")]
    #[allow(deprecated)]
    pub async fn replace_state(&self, state_bytes: &[u8]) -> Result<(), super::document::DocError> {
        self.doc.write().await.replace_state(state_bytes)?;
        self.last_edit.store(current_time_ms(), Ordering::Relaxed);
        Ok(())
    }

    /// Get the full document state as bytes (for snapshots).
    pub async fn to_state_bytes(&self) -> Vec<u8> {
        self.doc.read().await.to_state_bytes()
    }

    /// Broadcast a binary message to all clients except the sender.
    ///
    /// A failed `sender.send(...)` here means the recipient's WS task
    /// has dropped its receiver — typically the peer disconnected
    /// while we were holding the room read-lock. Historically we
    /// `let _ = ...` swallowed this, which made
    /// "did the broadcast actually reach the client" undiagnosable.
    /// Phase 1 of the observability design (see design/observability.md
    /// §Version-skew tolerance) replaces the swallow with a counter
    /// so the operator can see when peers vanish mid-broadcast.
    pub async fn broadcast(&self, exclude_client: u64, data: Vec<u8>) {
        let clients = self.clients.read().await;
        for (id, handle) in clients.iter() {
            if *id != exclude_client {
                if handle.sender.send(data.clone()).is_err() {
                    counter::inc(MetricKey::new(
                        "ws.send_errors_total",
                        &[("side", "primary")],
                    ));
                }
            }
        }
    }

    /// Send a binary message to a specific client. Used primarily for
    /// `MessageType::Error` frames — silently swallowing the delivery
    /// failure of an error frame is the worst possible silent failure,
    /// so this path always emits when the recipient channel is closed
    /// OR when the client has already disconnected (dropped from the
    /// clients map).
    ///
    /// The `reason` tag distinguishes:
    /// - `send_failed` — client is still in the map but the channel
    ///   send returned Err (recv side of the mpsc dropped between
    ///   the map read and the send).
    /// - `client_gone` — the client already disconnected before the
    ///   error frame was queued. Critical for #163's escalation
    ///   criteria: a rising `client_gone` rate on the LiveApp
    ///   error-frame path means the reject-mode toast is landing on
    ///   a socket that's already gone, and the user sees the
    ///   divergence-on-refresh symptom without any warning toast.
    pub async fn send_to_client(&self, client_id: u64, data: Vec<u8>) {
        let clients = self.clients.read().await;
        match clients.get(&client_id) {
            Some(handle) => {
                if handle.sender.send(data).is_err() {
                    counter::inc(MetricKey::new(
                        "ws.send_errors_total",
                        &[("side", "error_frame"), ("reason", "send_failed")],
                    ));
                }
            }
            None => {
                counter::inc(MetricKey::new(
                    "ws.send_errors_total",
                    &[("side", "error_frame"), ("reason", "client_gone")],
                ));
            }
        }
    }

    /// Send the current document state to a newly connected client (sync step 1).
    /// A failure here means the new client never receives the initial
    /// sync; the downstream recv loop then silently produces an
    /// inconsistent CRDT view of the doc.
    pub async fn sync_client(&self, client_id: u64) {
        let sv = self.state_vector().await;
        let msg = encode_message(MessageType::SyncStep1, &sv);
        let clients = self.clients.read().await;
        if let Some(handle) = clients.get(&client_id) {
            if handle.sender.send(msg).is_err() {
                counter::inc(MetricKey::new(
                    "ws.send_errors_total",
                    &[("side", "sync")],
                ));
            }
        }
    }

    /// Push a fresh SyncStep1 (our current state vector) to *every*
    /// connected client, prompting each to reply with its own state
    /// vector so the server can backfill anything the client missed.
    ///
    /// Used to self-heal after a dropped-update event — e.g. a lagged
    /// Redis subscriber (#10) — where connected clients may have
    /// silently diverged from the authoritative CRDT state. yrs sync is
    /// idempotent, so a client that missed nothing simply no-ops.
    /// Returns the number of clients the handshake was sent to.
    pub async fn resync_all_clients(&self) -> usize {
        let sv = self.state_vector().await;
        let msg = encode_message(MessageType::SyncStep1, &sv);
        let clients = self.clients.read().await;
        for handle in clients.values() {
            if handle.sender.send(msg.clone()).is_err() {
                counter::inc(MetricKey::new(
                    "ws.send_errors_total",
                    &[("side", "resync")],
                ));
            }
        }
        clients.len()
    }

    /// Remember a client's latest awareness JSON payload. Called after each
    /// awareness frame is broadcast so future joiners can be primed with the
    /// current cursor state.
    pub async fn store_awareness(&self, client_id: u64, payload: Vec<u8>) {
        self.awareness.write().await.insert(client_id, payload);
    }

    /// Forget a client's awareness when they disconnect.
    pub async fn forget_awareness(&self, client_id: u64) {
        self.awareness.write().await.remove(&client_id);
    }

    /// Snapshot the awareness payloads of every *other* client in the room.
    /// Used after a new client joins: the server replays each entry to them
    /// so their cursor overlay renders existing collaborators immediately
    /// instead of waiting for movement.
    pub async fn awareness_snapshot(&self, exclude_client: u64) -> Vec<Vec<u8>> {
        self.awareness
            .read()
            .await
            .iter()
            .filter_map(|(id, v)| (*id != exclude_client).then(|| v.clone()))
            .collect()
    }

    /// Get milliseconds since last edit (for idle detection).
    pub fn ms_since_last_edit(&self) -> u64 {
        let last = self.last_edit.load(Ordering::Relaxed);
        if last == 0 {
            return u64::MAX; // never edited
        }
        current_time_ms().saturating_sub(last)
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ─── Room Registry ──────────────────────────────────────────────

/// #114: minimum gap between subscriber-lag re-sync bursts. Under
/// sustained lag, `Lagged` can fire rapidly; re-syncing every client of
/// every room on each event would amplify the very load causing the lag
/// (the clients' state-vector replies pile back on). 5s is far above any
/// realistic single-stall cadence, so a normal one-off lag still
/// re-syncs immediately.
const RESYNC_DEBOUNCE_MS: u64 = 5_000;

/// Thread-safe registry of active collaboration rooms.
pub struct RoomRegistry {
    rooms: DashMap<String, Arc<Room>>,
    /// Wall-clock ms of the last `resync_all_rooms` burst (0 = never), for
    /// the #114 debounce.
    last_resync_ms: AtomicU64,
}

impl RoomRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            rooms: DashMap::new(),
            last_resync_ms: AtomicU64::new(0),
        }
    }

    /// Get an existing room or create a new one with the given document.
    pub fn get_or_insert(&self, doc_id: &str, doc: OgreDoc) -> Arc<Room> {
        self.rooms
            .entry(doc_id.to_string())
            .or_insert_with(|| Arc::new(Room::new(doc_id.to_string(), doc)))
            .value()
            .clone()
    }

    /// Get an existing room.
    pub fn get(&self, doc_id: &str) -> Option<Arc<Room>> {
        self.rooms.get(doc_id).map(|r| r.value().clone())
    }

    /// Remove a room if it exists. Returns the removed room.
    pub fn remove(&self, doc_id: &str) -> Option<Arc<Room>> {
        self.rooms.remove(doc_id).map(|(_, room)| room)
    }

    /// Remove a room only if it has no connected clients.
    /// Uses DashMap::remove_if to atomically check and remove.
    pub async fn remove_if_empty(&self, doc_id: &str) -> bool {
        // First check client count while holding the room reference
        let should_remove = if let Some(room) = self.rooms.get(doc_id) {
            room.client_count().await == 0
        } else {
            return false;
        };

        if should_remove {
            // Use remove_if to atomically re-check before removing.
            // The room's client_count could have changed, but DashMap ensures
            // we don't remove a different room that was inserted concurrently.
            let removed = self.rooms.remove_if(doc_id, |_, room| {
                // Synchronous check: use try_read to avoid blocking.
                // If we can't get the lock, skip removal (client is active).
                room.clients
                    .try_read()
                    .map(|c| c.is_empty())
                    .unwrap_or(false)
            });
            return removed.is_some();
        }
        false
    }

    /// Get the number of active rooms.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Re-sync every connected client of every locally-hosted room.
    ///
    /// Called when the Redis subscriber lagged and dropped updates
    /// (#10): the dropped messages are unrecoverable on that channel, so
    /// rather than leave already-connected clients silently divergent we
    /// ask them all to re-handshake (SyncStep1). Bounds divergence to a
    /// single lag event instead of "until the client reconnects".
    /// Returns the number of rooms re-synced.
    pub async fn resync_all_rooms(&self) -> usize {
        // #114: debounce — skip if we re-synced within the last window, so
        // a burst of `Lagged` events can't trigger a resync storm. Single
        // caller (the subscriber task) so a plain load/store is race-free;
        // the store also serves as the new window anchor.
        let now = current_time_ms();
        if now.saturating_sub(self.last_resync_ms.load(Ordering::Relaxed)) < RESYNC_DEBOUNCE_MS {
            return 0;
        }
        self.last_resync_ms.store(now, Ordering::Relaxed);

        // Snapshot the room handles first so we never hold a DashMap
        // shard guard across the awaits below (which take each room's
        // async client lock).
        let rooms: Vec<Arc<Room>> = self.rooms.iter().map(|r| r.value().clone()).collect();
        let count = rooms.len();
        for room in rooms {
            room.resync_all_clients().await;
        }
        count
    }

    /// Apply an update that arrived from another server instance via the
    /// Redis pub/sub fanout. If the room isn't active on this instance, the
    /// update is dropped — it will be replayed from DocRepo when a client
    /// next connects here.
    ///
    /// Expects `wire_bytes` to be a protocol-framed message (1-byte type +
    /// payload). `Update` frames are applied to the local CRDT before
    /// being relayed; `CommentEvent` frames carry no CRDT state so they're
    /// just relayed to every connected client. Other types (SyncStep1/2,
    /// awareness, etc.) are ignored.
    pub async fn apply_remote_update(&self, doc_id: &str, wire_bytes: &[u8]) {
        use super::protocol::{decode_message, MessageType};
        let Some(room) = self.get(doc_id) else { return };
        let Some((msg_type, payload)) = decode_message(wire_bytes) else {
            tracing::warn!(doc_id, "remote update: malformed wire frame");
            return;
        };
        match msg_type {
            MessageType::Update => {
                if let Err(e) = room.apply_update(payload).await {
                    tracing::warn!(doc_id, error = ?e, "remote update: apply failed");
                    return;
                }
            }
            MessageType::CommentEvent => {
                // No CRDT state to update — comments live in the thread DB.
                // Just relay so clients on this instance refresh.
            }
            _ => return,
        }
        // Broadcast the original wire-framed message to every local client.
        // Client IDs start at 1, so passing 0 excludes nobody.
        room.broadcast(0, wire_bytes.to_vec()).await;
    }

    /// List all room doc_ids that are idle (no clients, last edit > threshold).
    pub async fn idle_rooms(&self, idle_threshold_ms: u64) -> Vec<String> {
        // Snapshot (doc_id, room) first so we never hold a DashMap shard
        // read guard across the `client_count().await` below — parking a
        // Tokio worker while holding a parking_lot guard can stall the pool
        // under contention (#8). Mirrors `resync_all_rooms`.
        let snapshot: Vec<(String, Arc<Room>)> = self
            .rooms
            .iter()
            .map(|e| (e.key().clone(), Arc::clone(e.value())))
            .collect();
        let mut idle = Vec::new();
        for (doc_id, room) in snapshot {
            if room.client_count().await == 0 && room.ms_since_last_edit() >= idle_threshold_ms {
                idle.push(doc_id);
            }
        }
        idle
    }
}

impl Default for RoomRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::blocks::WalkScope;
    use super::super::protocol::decode_message;
    use yrs::{ReadTxn, Transact, WriteTxn};

    #[tokio::test]
    async fn room_add_remove_client() {
        let room = Room::new_empty("doc-1".to_string());
        let (tx, _rx) = mpsc::unbounded_channel();
        let client_id = room.next_client_id();

        room.add_client(client_id, "user-1".to_string(), tx).await;
        assert_eq!(room.client_count().await, 1);

        let empty = room.remove_client(client_id).await;
        assert!(empty);
        assert_eq!(room.client_count().await, 0);
    }

    #[tokio::test]
    async fn client_count_for_user_isolates_users() {
        // Per-user-per-doc connection cap (#34) hangs off this method.
        // Verify it counts only matching user_ids.
        let room = Room::new_empty("doc-1".to_string());
        for (uid, n) in [("alice", 3usize), ("bob", 2)] {
            for _ in 0..n {
                let (tx, _rx) = mpsc::unbounded_channel();
                let cid = room.next_client_id();
                room.add_client(cid, uid.to_string(), tx).await;
            }
        }
        assert_eq!(room.client_count().await, 5);
        assert_eq!(room.client_count_for_user("alice").await, 3);
        assert_eq!(room.client_count_for_user("bob").await, 2);
        assert_eq!(room.client_count_for_user("nobody").await, 0);
    }

    #[tokio::test]
    async fn resync_all_clients_sends_sync_step1_to_everyone() {
        // #10: after a lagged subscriber, every connected client must
        // receive a fresh SyncStep1 so it can re-handshake and backfill
        // anything it missed.
        let room = Room::new_empty("doc-1".to_string());
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();
        room.add_client(room.next_client_id(), "u1".to_string(), tx1).await;
        room.add_client(room.next_client_id(), "u2".to_string(), tx2).await;

        let sent = room.resync_all_clients().await;
        assert_eq!(sent, 2, "both clients should be re-synced");

        for rx in [&mut rx1, &mut rx2] {
            let frame = rx.try_recv().expect("client must receive a resync frame");
            let (msg_type, _) = decode_message(&frame).expect("decodable frame");
            assert_eq!(
                msg_type,
                MessageType::SyncStep1,
                "resync must be a SyncStep1 handshake"
            );
        }
    }

    #[tokio::test]
    async fn resync_all_rooms_covers_every_room() {
        // #10: the registry-level recovery must reach clients across all
        // locally-hosted rooms, not just one.
        let registry = RoomRegistry::new();
        let room_a = registry.get_or_insert("doc-a", OgreDoc::new());
        let room_b = registry.get_or_insert("doc-b", OgreDoc::new());
        let (tx_a, mut rx_a) = mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = mpsc::unbounded_channel();
        room_a.add_client(room_a.next_client_id(), "ua".to_string(), tx_a).await;
        room_b.add_client(room_b.next_client_id(), "ub".to_string(), tx_b).await;

        let rooms_resynced = registry.resync_all_rooms().await;
        assert_eq!(rooms_resynced, 2);
        assert!(rx_a.try_recv().is_ok(), "room A client must be re-synced");
        assert!(rx_b.try_recv().is_ok(), "room B client must be re-synced");
    }

    #[tokio::test]
    async fn resync_all_rooms_debounces_rapid_calls() {
        // #114: a second resync within the debounce window must be skipped
        // (returns 0, sends nothing) so a lag burst can't become a resync
        // storm. The two calls here are microseconds apart — well inside
        // RESYNC_DEBOUNCE_MS.
        let registry = RoomRegistry::new();
        let room = registry.get_or_insert("doc-a", OgreDoc::new());
        let (tx, mut rx) = mpsc::unbounded_channel();
        room.add_client(room.next_client_id(), "u".to_string(), tx).await;

        // First call re-syncs the one client.
        assert_eq!(registry.resync_all_rooms().await, 1);
        assert!(rx.try_recv().is_ok(), "first resync must send a frame");

        // Second call is debounced: no rooms reported, no frame sent.
        assert_eq!(registry.resync_all_rooms().await, 0);
        assert!(rx.try_recv().is_err(), "debounced resync must not send a frame");
    }

    #[tokio::test]
    async fn room_broadcast_excludes_sender() {
        let room = Room::new_empty("doc-1".to_string());
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        let id1 = room.next_client_id();
        let id2 = room.next_client_id();
        room.add_client(id1, "user-1".to_string(), tx1).await;
        room.add_client(id2, "user-2".to_string(), tx2).await;

        room.broadcast(id1, b"hello".to_vec()).await;

        // Client 2 should receive, client 1 should not
        assert!(rx2.try_recv().is_ok());
        assert!(rx1.try_recv().is_err());
    }

    #[tokio::test]
    async fn room_apply_update_and_state() {
        let room = Room::new_empty("doc-1".to_string());
        let sv = room.state_vector().await;
        assert!(!sv.is_empty());

        let state_bytes = room.to_state_bytes().await;
        assert!(!state_bytes.is_empty());
    }

    #[tokio::test]
    async fn registry_get_or_insert() {
        let registry = RoomRegistry::new();
        let room = registry.get_or_insert("doc-1", OgreDoc::new());
        assert_eq!(room.doc_id(), "doc-1");
        assert_eq!(registry.room_count(), 1);

        // Second call returns the same room
        let room2 = registry.get_or_insert("doc-1", OgreDoc::new());
        assert_eq!(Arc::as_ptr(&room), Arc::as_ptr(&room2));
    }

    #[tokio::test]
    async fn registry_remove_if_empty() {
        let registry = RoomRegistry::new();
        let _room = registry.get_or_insert("doc-1", OgreDoc::new());

        // No clients — should remove
        assert!(registry.remove_if_empty("doc-1").await);
        assert_eq!(registry.room_count(), 0);

        // Re-create and add a client
        let room = registry.get_or_insert("doc-1", OgreDoc::new());
        let (tx, _rx) = mpsc::unbounded_channel();
        room.add_client(1, "user-1".to_string(), tx).await;

        // Has clients — should not remove
        assert!(!registry.remove_if_empty("doc-1").await);
        assert_eq!(registry.room_count(), 1);
    }

    // ── New tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn next_client_id_is_unique() {
        let room = Room::new_empty("doc-1".to_string());
        let id1 = room.next_client_id();
        let id2 = room.next_client_id();
        let id3 = room.next_client_id();
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        // IDs should be monotonically increasing from 1
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[tokio::test]
    async fn send_to_client() {
        let room = Room::new_empty("doc-1".to_string());
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        let id1 = room.next_client_id();
        let id2 = room.next_client_id();
        room.add_client(id1, "user-1".to_string(), tx1).await;
        room.add_client(id2, "user-2".to_string(), tx2).await;

        room.send_to_client(id1, b"direct".to_vec()).await;

        // Only client 1 should receive
        let msg = rx1.try_recv().unwrap();
        assert_eq!(msg, b"direct");
        assert!(rx2.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_to_nonexistent_client() {
        let room = Room::new_empty("doc-1".to_string());
        // Should not panic
        room.send_to_client(999, b"nope".to_vec()).await;
    }

    #[tokio::test]
    async fn sync_client_sends_sync_step1() {
        let room = Room::new_empty("doc-1".to_string());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let id = room.next_client_id();
        room.add_client(id, "user-1".to_string(), tx).await;

        room.sync_client(id).await;

        let msg = rx.try_recv().unwrap();
        // Verify it's a SyncStep1 message
        let (msg_type, payload) = decode_message(&msg).unwrap();
        assert_eq!(msg_type, MessageType::SyncStep1);
        assert!(!payload.is_empty(), "state vector payload should not be empty");
    }

    #[tokio::test]
    async fn ms_since_last_edit_never_edited() {
        let room = Room::new_empty("doc-1".to_string());
        // Room has never been edited — should return u64::MAX
        assert_eq!(room.ms_since_last_edit(), u64::MAX);
    }

    #[tokio::test]
    async fn ms_since_last_edit_after_update() {
        let room = Room::new_empty("doc-1".to_string());

        // Create a valid update by making a change in a separate doc
        let doc1 = OgreDoc::new();
        {
            use yrs::{Transact, WriteTxn, types::xml::{XmlFragment, XmlTextPrelim, XmlOut}};
            let mut txn = doc1.inner().transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            if let Some(XmlOut::Element(para)) = frag.get(&txn, 0) {
                para.insert(&mut txn, 0, XmlTextPrelim::new("edit"));
            }
        }
        let sv = room.state_vector().await;
        let diff = doc1.encode_diff(&sv).unwrap();

        room.apply_update(&diff).await.unwrap();

        // Should be very recent (within 1 second)
        assert!(room.ms_since_last_edit() < 1000);
    }

    #[tokio::test]
    async fn apply_update_invalid_bytes() {
        let room = Room::new_empty("doc-1".to_string());
        let result = room.apply_update(&[0xFF, 0xFE]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn remove_client_returns_false_when_others_remain() {
        let room = Room::new_empty("doc-1".to_string());
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();

        let id1 = room.next_client_id();
        let id2 = room.next_client_id();
        room.add_client(id1, "user-1".to_string(), tx1).await;
        room.add_client(id2, "user-2".to_string(), tx2).await;

        // Remove one — room not empty
        let empty = room.remove_client(id1).await;
        assert!(!empty);
        assert_eq!(room.client_count().await, 1);
    }

    #[tokio::test]
    async fn registry_get_returns_none_for_missing() {
        let registry = RoomRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn registry_get_returns_room() {
        let registry = RoomRegistry::new();
        registry.get_or_insert("doc-1", OgreDoc::new());
        let room = registry.get("doc-1");
        assert!(room.is_some());
        assert_eq!(room.unwrap().doc_id(), "doc-1");
    }

    #[tokio::test]
    async fn registry_remove() {
        let registry = RoomRegistry::new();
        registry.get_or_insert("doc-1", OgreDoc::new());
        assert_eq!(registry.room_count(), 1);

        let removed = registry.remove("doc-1");
        assert!(removed.is_some());
        assert_eq!(registry.room_count(), 0);

        // Remove again returns None
        assert!(registry.remove("doc-1").is_none());
    }

    #[tokio::test]
    async fn registry_multiple_rooms() {
        let registry = RoomRegistry::new();
        registry.get_or_insert("doc-1", OgreDoc::new());
        registry.get_or_insert("doc-2", OgreDoc::new());
        registry.get_or_insert("doc-3", OgreDoc::new());
        assert_eq!(registry.room_count(), 3);

        registry.remove("doc-2");
        assert_eq!(registry.room_count(), 2);
        assert!(registry.get("doc-1").is_some());
        assert!(registry.get("doc-2").is_none());
        assert!(registry.get("doc-3").is_some());
    }

    #[tokio::test]
    async fn registry_remove_if_empty_nonexistent() {
        let registry = RoomRegistry::new();
        // Nonexistent room returns false
        assert!(!registry.remove_if_empty("nope").await);
    }

    #[tokio::test]
    async fn idle_rooms_empty_registry() {
        let registry = RoomRegistry::new();
        let idle = registry.idle_rooms(0).await;
        assert!(idle.is_empty());
    }

    #[tokio::test]
    async fn idle_rooms_filters_active() {
        let registry = RoomRegistry::new();
        let _room1 = registry.get_or_insert("idle-doc", OgreDoc::new());
        let room2 = registry.get_or_insert("active-doc", OgreDoc::new());

        // Add a client to room2 (makes it non-idle)
        let (tx, _rx) = mpsc::unbounded_channel();
        room2.add_client(1, "user-1".to_string(), tx).await;

        // Both rooms have never been edited (last_edit=0 → ms_since_last_edit=MAX)
        // idle-doc has 0 clients, active-doc has 1 client
        let idle = registry.idle_rooms(0).await;
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0], "idle-doc");
    }

    #[tokio::test]
    async fn registry_default_trait() {
        let registry = RoomRegistry::default();
        assert_eq!(registry.room_count(), 0);
    }

    // ── awareness cache ────────────────────────────────────────────

    #[tokio::test]
    async fn awareness_store_and_snapshot() {
        let room = Room::new_empty("doc-1".to_string());
        room.store_awareness(1, b"alice-state".to_vec()).await;
        room.store_awareness(2, b"bob-state".to_vec()).await;

        // Snapshot excluding client 1 returns bob only.
        let snap = room.awareness_snapshot(1).await;
        assert_eq!(snap, vec![b"bob-state".to_vec()]);

        // Snapshot excluding client 2 returns alice only.
        let snap = room.awareness_snapshot(2).await;
        assert_eq!(snap, vec![b"alice-state".to_vec()]);

        // Snapshot excluding a third party returns both (order-independent).
        let mut snap = room.awareness_snapshot(99).await;
        snap.sort();
        let mut expected: Vec<Vec<u8>> = vec![b"alice-state".to_vec(), b"bob-state".to_vec()];
        expected.sort();
        assert_eq!(snap, expected);
    }

    #[tokio::test]
    async fn awareness_store_overwrites() {
        // Each client has at most one cached awareness payload — the latest.
        let room = Room::new_empty("doc-1".to_string());
        room.store_awareness(1, b"v1".to_vec()).await;
        room.store_awareness(1, b"v2".to_vec()).await;
        let snap = room.awareness_snapshot(999).await;
        assert_eq!(snap, vec![b"v2".to_vec()]);
    }

    #[tokio::test]
    async fn awareness_forget_removes() {
        let room = Room::new_empty("doc-1".to_string());
        room.store_awareness(1, b"alice".to_vec()).await;
        room.store_awareness(2, b"bob".to_vec()).await;
        room.forget_awareness(1).await;
        let snap = room.awareness_snapshot(999).await;
        assert_eq!(snap, vec![b"bob".to_vec()]);
    }

    #[tokio::test]
    async fn awareness_snapshot_empty_when_nobody_has_stored() {
        let room = Room::new_empty("doc-1".to_string());
        assert!(room.awareness_snapshot(42).await.is_empty());
    }

    // ── apply_remote_update CommentEvent relay ─────────────────────

    #[tokio::test]
    async fn apply_remote_update_relays_comment_event_to_local_clients() {
        // A peer instance published a CommentEvent for a doc that has
        // active subscribers on this instance. apply_remote_update should
        // forward the wire-framed message to each connected client without
        // touching the CRDT (comments aren't carried in the doc).
        use super::super::protocol::encode_message;

        let registry = RoomRegistry::new();
        let room = registry.get_or_insert("doc-1", OgreDoc::new());
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();
        room.add_client(1, "alice".into(), tx1).await;
        room.add_client(2, "bob".into(), tx2).await;

        let payload = br#"{"kind":"thread_created","threadId":"abc"}"#;
        let frame = encode_message(MessageType::CommentEvent, payload);

        registry.apply_remote_update("doc-1", &frame).await;

        let recv1 = rx1.try_recv().expect("alice should get the CommentEvent");
        let recv2 = rx2.try_recv().expect("bob should get the CommentEvent");
        assert_eq!(recv1, frame);
        assert_eq!(recv2, frame);
    }

    #[tokio::test]
    async fn apply_remote_update_ignores_unknown_message_types() {
        // SyncStep1/2, Awareness, Ping, Auth, Error: not relayed via the
        // pub/sub fanout. Only Update and CommentEvent should reach
        // connected clients.
        use super::super::protocol::encode_message;

        let registry = RoomRegistry::new();
        let room = registry.get_or_insert("doc-1", OgreDoc::new());
        let (tx, mut rx) = mpsc::unbounded_channel();
        room.add_client(1, "alice".into(), tx).await;

        let frame = encode_message(MessageType::Awareness, b"junk");
        registry.apply_remote_update("doc-1", &frame).await;
        assert!(rx.try_recv().is_err(), "Awareness frames must not be relayed");
    }

    // ── Phase 2a — LiveApp pre-apply gate ────────────────────────

    #[test]
    fn validation_mode_env_parse() {
        assert_eq!(
            LiveAppValidationMode::from_env_value(Some("off")),
            LiveAppValidationMode::Off
        );
        assert_eq!(
            LiveAppValidationMode::from_env_value(Some("OFF")),
            LiveAppValidationMode::Off
        );
        assert_eq!(
            LiveAppValidationMode::from_env_value(Some("log")),
            LiveAppValidationMode::Log
        );
        assert_eq!(
            LiveAppValidationMode::from_env_value(Some("reject")),
            LiveAppValidationMode::Reject
        );
        // Unrecognized → Log (fail-safe toward observability).
        assert_eq!(
            LiveAppValidationMode::from_env_value(Some("garbage")),
            LiveAppValidationMode::Log
        );
        assert_eq!(
            LiveAppValidationMode::from_env_value(None),
            LiveAppValidationMode::Log
        );
    }

    /// Build a room seeded with a Kanban board holding one column
    /// and one red-color card. Returns the room and a peer doc
    /// initialized from the same baseline so callers can craft
    /// updates that reference the same block IDs.
    async fn kanban_room_and_peer(
    ) -> (Arc<Room>, OgreDoc) {
        use yrs::types::xml::{Xml, XmlElementPrelim, XmlFragment};
        use crate::schema::NodeType;

        let doc = OgreDoc::new();
        {
            let mut txn = doc.inner().transact_mut();
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
            card.insert_attribute(&mut txn, "title", "Fix login");
            card.insert_attribute(&mut txn, "color", "red");
        }
        let state = doc.to_state_bytes();
        let peer = OgreDoc::from_state_bytes(&state).unwrap();

        let room = Room::new("doc-1".to_string(), doc);
        (Arc::new(room), peer)
    }

    #[tokio::test]
    async fn gated_apply_off_mode_accepts_invalid_write() {
        use yrs::types::xml::{Xml, XmlFragment, XmlOut};

        let (room, peer) = kanban_room_and_peer().await;
        // Mutate peer's card color to something the validator would reject.
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            card.insert_attribute(&mut txn, "color", "javascript:");
        }
        let sv = room.state_vector().await;
        let mutation = peer.encode_diff(&sv).unwrap();

        // Off mode: apply anyway.
        let result = room
            .apply_update_gated(&mutation, LiveAppValidationMode::Off, WalkScope::Full)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn gated_apply_log_mode_accepts_invalid_write() {
        use yrs::types::xml::{Xml, XmlFragment, XmlOut};

        let (room, peer) = kanban_room_and_peer().await;
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            card.insert_attribute(&mut txn, "color", "javascript:");
        }
        let sv = room.state_vector().await;
        let mutation = peer.encode_diff(&sv).unwrap();

        // Log mode: metric fires (untested here), apply succeeds.
        let result = room
            .apply_update_gated(&mutation, LiveAppValidationMode::Log, WalkScope::Full)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn gated_apply_reject_mode_refuses_invalid_write() {
        use yrs::types::xml::{Xml, XmlFragment, XmlOut};

        let (room, peer) = kanban_room_and_peer().await;
        // Snapshot the room's state before the (rejected) write so
        // we can prove nothing landed.
        let state_before = room.to_state_bytes().await;
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            card.insert_attribute(&mut txn, "color", "javascript:");
        }
        let sv = room.state_vector().await;
        let mutation = peer.encode_diff(&sv).unwrap();

        // Reject mode: refuse the apply.
        let err = room
            .apply_update_gated(&mutation, LiveAppValidationMode::Reject, WalkScope::Full)
            .await
            .expect_err("reject mode must refuse invalid attr writes");
        assert!(
            err.to_string().contains("liveapp validation rejected"),
            "unexpected error message: {err}"
        );
        // And the doc state hasn't advanced.
        let state_after = room.to_state_bytes().await;
        assert_eq!(state_before, state_after);
    }

    /// Regression for Phase-2a review finding #1 — the gate must
    /// reject oversized (silently-clamped) values, not just
    /// structurally-wrong (Err-returning) values. Pre-fix, a
    /// 200-char card title would sail through Reject mode because
    /// `validate_card_attrs` returned `Ok(clamped)` and the gate
    /// only checked Ok/Err.
    #[tokio::test]
    async fn gated_apply_reject_mode_refuses_silently_clamped_write() {
        use crate::blocks::kanban::MAX_CARD_TITLE_LEN;
        use yrs::types::xml::{Xml, XmlFragment, XmlOut};

        let (room, peer) = kanban_room_and_peer().await;
        let state_before = room.to_state_bytes().await;
        let too_long = "x".repeat(MAX_CARD_TITLE_LEN + 20);
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            card.insert_attribute(&mut txn, "title", too_long.as_str());
        }
        let sv = room.state_vector().await;
        let mutation = peer.encode_diff(&sv).unwrap();

        let err = room
            .apply_update_gated(&mutation, LiveAppValidationMode::Reject, WalkScope::Full)
            .await
            .expect_err("clamped title must be rejected under strict compare");
        assert!(
            err.to_string().contains("liveapp validation rejected"),
            "unexpected error message: {err}"
        );
        let state_after = room.to_state_bytes().await;
        assert_eq!(state_before, state_after, "state must not advance on reject");
    }

    #[tokio::test]
    async fn gated_apply_reject_mode_allows_valid_write() {
        use yrs::types::xml::{Xml, XmlFragment, XmlOut};

        let (room, peer) = kanban_room_and_peer().await;
        // Rename column — a change that doesn't violate any schema.
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            col.insert_attribute(&mut txn, "title", "Doing");
        }
        let sv = room.state_vector().await;
        let mutation = peer.encode_diff(&sv).unwrap();

        room.apply_update_gated(&mutation, LiveAppValidationMode::Reject, WalkScope::Full)
            .await
            .expect("valid rename must pass reject mode");
    }

    /// gap-003: deleting a KanbanCard via an interactive update
    /// surfaces in the returned `GatedApplyReport.deletions`. The
    /// WS handler consumes this to emit a `LiveAppNodeDeleted`
    /// audit row.
    #[tokio::test]
    async fn gated_apply_reports_deleted_kanban_card() {
        use yrs::types::xml::{Xml, XmlFragment, XmlOut};

        let (room, peer) = kanban_room_and_peer().await;

        // The peer's baseline has one KanbanCard. Read its
        // blockId (server assigns it via structural_hash) so the
        // deletion assertion can key on the real value if
        // present. Then delete it.
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
        let sv = room.state_vector().await;
        let mutation = peer.encode_diff(&sv).unwrap();

        let report = room
            .apply_update_gated(&mutation, LiveAppValidationMode::Reject, WalkScope::Full)
            .await
            .expect("card deletion is a valid write");
        assert_eq!(report.deletions.len(), 1);
        let del = &report.deletions[0];
        assert_eq!(del.node_type, crate::schema::NodeType::KanbanCard);
        // block_id echoes what was stored on the deleted card
        // (may be empty if the fixture didn't set one).
        assert_eq!(del.block_id, card_block_id);
    }
}
