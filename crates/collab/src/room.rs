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

use super::document::OgreDoc;
use super::protocol::{encode_message, MessageType};

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

    /// Get the full document state as bytes (for snapshots).
    pub async fn to_state_bytes(&self) -> Vec<u8> {
        self.doc.read().await.to_state_bytes()
    }

    /// Broadcast a binary message to all clients except the sender.
    pub async fn broadcast(&self, exclude_client: u64, data: Vec<u8>) {
        let clients = self.clients.read().await;
        for (id, handle) in clients.iter() {
            if *id != exclude_client {
                let _ = handle.sender.send(data.clone());
            }
        }
    }

    /// Send a binary message to a specific client.
    pub async fn send_to_client(&self, client_id: u64, data: Vec<u8>) {
        let clients = self.clients.read().await;
        if let Some(handle) = clients.get(&client_id) {
            let _ = handle.sender.send(data);
        }
    }

    /// Send the current document state to a newly connected client (sync step 1).
    pub async fn sync_client(&self, client_id: u64) {
        let sv = self.state_vector().await;
        let msg = encode_message(MessageType::SyncStep1, &sv);
        let clients = self.clients.read().await;
        if let Some(handle) = clients.get(&client_id) {
            let _ = handle.sender.send(msg);
        }
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

/// Thread-safe registry of active collaboration rooms.
pub struct RoomRegistry {
    rooms: DashMap<String, Arc<Room>>,
}

impl RoomRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            rooms: DashMap::new(),
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
            self.rooms.remove_if(doc_id, |_, room| {
                // Synchronous check: use try_read to avoid blocking.
                // If we can't get the lock, skip removal (client is active).
                room.clients
                    .try_read()
                    .map(|c| c.is_empty())
                    .unwrap_or(false)
            });
            return true;
        }
        false
    }

    /// Get the number of active rooms.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// List all room doc_ids that are idle (no clients, last edit > threshold).
    pub async fn idle_rooms(&self, idle_threshold_ms: u64) -> Vec<String> {
        let mut idle = Vec::new();
        for entry in self.rooms.iter() {
            let room = entry.value();
            if room.client_count().await == 0 && room.ms_since_last_edit() >= idle_threshold_ms {
                idle.push(entry.key().clone());
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
        let room = registry.get_or_insert("doc-1", OgreDoc::new());

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
}
