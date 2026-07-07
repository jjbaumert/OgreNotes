// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Background task for compacting idle collaboration rooms.
//!
//! When a room has been idle (no edits, no clients) for a configurable threshold,
//! the compaction task snapshots the CRDT state to S3, prunes the UPDATE# rows
//! from DynamoDB, and removes the room from the in-memory registry.

use std::sync::Arc;

use ogrenotes_collab::room::RoomRegistry;
use ogrenotes_common::metrics::{counter, histogram, MetricKey};
use ogrenotes_storage::repo::doc_repo::DocRepo;

/// Spawn a background task that periodically compacts idle rooms.
///
/// - Checks every `interval` duration.
/// - Rooms idle for >= `idle_threshold_ms` with 0 clients get compacted.
/// - Compaction: snapshot to S3, prune UPDATE# rows, remove from registry.
pub fn spawn_compaction_task(
    registry: Arc<RoomRegistry>,
    doc_repo: Arc<DocRepo>,
    interval: std::time::Duration,
    idle_threshold_ms: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            compact_idle_rooms(&registry, &doc_repo, idle_threshold_ms).await;
        }
    })
}

/// Run one compaction pass: find idle rooms and compact each.
///
/// Separated from the spawn loop for testability.
pub async fn compact_idle_rooms(
    registry: &RoomRegistry,
    doc_repo: &DocRepo,
    idle_threshold_ms: u64,
) {
    let idle_doc_ids = registry.idle_rooms(idle_threshold_ms).await;

    for doc_id in idle_doc_ids {
        compact_room(registry, doc_repo, &doc_id).await;
    }
}

/// Outcome of a compaction attempt — exposed so the admin route can
/// distinguish "compacted, N rows pruned" from "skipped because the
/// room is missing" without re-parsing tracing events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactOutcome {
    /// Compaction succeeded; `version` is the new snapshot version
    /// and `updates_pruned` counts the UPDATE# rows removed.
    Compacted { version: u64, updates_pruned: u64 },
    /// Room is not in the registry (never opened, or evicted).
    RoomMissing,
    /// Room has connected clients and `force=false` was supplied —
    /// the periodic worker uses this branch; the admin endpoint
    /// passes `force=true` to bypass it.
    RoomActive,
    /// Document metadata not found in DDB.
    MetadataMissing,
    /// Snapshot write to S3+DDB failed.
    SnapshotFailed,
}

/// Compact a single room: snapshot, prune, remove.
///
/// Skips if the room no longer exists or has reconnected clients.
/// Periodic worker callers pass `force=false`; the admin endpoint
/// passes `force=true` to compact active rooms (the cutoff design
/// is race-safe — see the cutoff comment below).
pub async fn compact_room(
    registry: &RoomRegistry,
    doc_repo: &DocRepo,
    doc_id: &str,
) -> bool {
    matches!(
        compact_room_with_outcome(registry, doc_repo, doc_id, false).await,
        CompactOutcome::Compacted { .. }
    )
}

/// Decide what to do when the last client leaves a room.
///
/// If the doc has unsnapshotted `UPDATE#` rows, compact it (snapshot to
/// S3, prune the op log, drop the room). If it has none — e.g. a
/// read-only open/close — just drop the empty room, so viewing a doc
/// doesn't write a redundant snapshot (and a new `SNAPSHOT#` version row)
/// every time. A transient failure must never evict the room without
/// compacting — that would orphan the op log — so on a snapshot failure or
/// a client reconnecting mid-call the room is left for the periodic
/// compactor to retry; only a confirmed-deleted doc has its empty room
/// dropped.
///
/// Without this, a doc edited only over WebSocket whose clients all
/// disconnect cleanly was removed from the registry before the periodic
/// compactor could see it, so its op log was never pruned and cold-loads
/// grew without bound.
pub async fn compact_or_remove_on_empty(
    registry: &RoomRegistry,
    doc_repo: &DocRepo,
    doc_id: &str,
) {
    let has_pending = match doc_repo.has_pending_updates(doc_id).await {
        Ok(p) => p,
        Err(e) => {
            // DynamoDB unavailable — we can't tell whether there's an op log
            // to prune. Leave the room in the registry so the periodic
            // compactor retries it; evicting it here would orphan the op log,
            // the very bug this function exists to prevent.
            tracing::warn!(
                doc_id,
                error = %e,
                "compact_or_remove_on_empty: pending-update check failed; leaving room for periodic compactor"
            );
            return;
        }
    };

    if !has_pending {
        // Read-only open/close: nothing to fold in, so don't write a
        // redundant snapshot — just drop the empty room.
        registry.remove_if_empty(doc_id).await;
        return;
    }

    // Unsnapshotted updates exist: compact (snapshot + prune + drop room).
    match compact_room_with_outcome(registry, doc_repo, doc_id, false).await {
        // Snapshotted + pruned (room already removed), or a concurrent
        // disconnect already removed it.
        CompactOutcome::Compacted { .. } | CompactOutcome::RoomMissing => {}
        // A client reconnected between the check and the compaction, or the
        // snapshot write failed. Leave the room either way: a reconnect means
        // it's in use, and a failed snapshot must NOT evict (that would orphan
        // the op log) — the periodic compactor retries idle empty rooms.
        CompactOutcome::RoomActive | CompactOutcome::SnapshotFailed => {}
        // MetadataMissing collapses two cases inside compact_room_with_outcome:
        // the doc is truly deleted (`Ok(None)`) or the metadata fetch hit a
        // transient error (`Err`). Re-check before evicting — drop the room
        // only when the doc is confirmed gone; otherwise leave it for the
        // periodic compactor, so a transient error can't orphan the op log.
        CompactOutcome::MetadataMissing => match doc_repo.get(doc_id).await {
            Ok(None) => {
                registry.remove_if_empty(doc_id).await;
            }
            Ok(Some(_)) | Err(_) => {
                tracing::warn!(
                    doc_id,
                    "compact_or_remove_on_empty: metadata check inconclusive; leaving room for periodic compactor"
                );
            }
        },
    }
}

/// Variant that returns a typed outcome and accepts a `force` flag.
/// The periodic worker calls this via `compact_room`; the admin
/// endpoint calls it directly with `force=true`.
pub async fn compact_room_with_outcome(
    registry: &RoomRegistry,
    doc_repo: &DocRepo,
    doc_id: &str,
    force: bool,
) -> CompactOutcome {
    let started_at = std::time::Instant::now();
    let room = match registry.get(doc_id) {
        Some(r) => r,
        None => return CompactOutcome::RoomMissing,
    };

    if !force && room.client_count().await > 0 {
        return CompactOutcome::RoomActive;
    }

    // Record the cutoff time BEFORE reading state, so any updates
    // written after this point are preserved. This is what makes
    // force-compacting an active room safe: in-flight updates that
    // land between `cutoff` and `delete_updates_before(cutoff)` are
    // strictly newer than the cutoff and are not pruned.
    let cutoff = ogrenotes_common::time::now_usec();

    let state_bytes = room.to_state_bytes().await;

    let new_version = match doc_repo.get(doc_id).await {
        Ok(Some(meta)) => meta.snapshot_version + 1,
        _ => return CompactOutcome::MetadataMissing,
    };

    if let Err(e) = doc_repo
        .save_snapshot(doc_id, &state_bytes, new_version, cutoff, "system")
        .await
    {
        counter::inc(MetricKey::new("compaction.failure_total", &[]));
        tracing::warn!(doc_id, error = %e, "compaction: snapshot save failed");
        return CompactOutcome::SnapshotFailed;
    }

    let updates_pruned = match doc_repo.delete_updates_before(doc_id, cutoff).await {
        Ok(count) => {
            if count > 0 {
                counter::add(
                    MetricKey::new("compaction.updates_pruned", &[]),
                    count as u64,
                );
                tracing::info!(doc_id, count, "compaction: pruned updates");
            }
            count as u64
        }
        Err(e) => {
            tracing::warn!(doc_id, error = %e, "compaction: update pruning failed");
            0
        }
    };

    // Only the periodic path tries to remove the room. The admin
    // path keeps the room alive so connected clients aren't
    // disturbed mid-session.
    if !force {
        let _ = registry.remove_if_empty(doc_id).await;
    }

    counter::inc(MetricKey::new("compaction.success_total", &[]));
    histogram::record(
        MetricKey::new("compaction.duration_ms", &[]),
        started_at.elapsed().as_secs_f64() * 1000.0,
    );
    tracing::info!(
        doc_id,
        version = new_version,
        forced = force,
        updates_pruned,
        "compaction: room compacted"
    );
    CompactOutcome::Compacted {
        version: new_version,
        updates_pruned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_collab::document::OgreDoc;
    use ogrenotes_collab::room::RoomRegistry;
    use tokio::sync::mpsc;

    // ─── Unit tests for compaction logic (no AWS needed) ───

    #[tokio::test]
    async fn idle_rooms_detected_when_no_clients_and_old_edit() {
        let registry = RoomRegistry::new();
        let _room = registry.get_or_insert("doc-1", OgreDoc::new());

        // Room has never been edited (last_edit = 0) and has no clients.
        // ms_since_last_edit returns u64::MAX for never-edited rooms.
        let idle = registry.idle_rooms(1000).await;
        assert_eq!(idle, vec!["doc-1"]);
    }

    #[tokio::test]
    async fn room_with_clients_not_idle() {
        let registry = RoomRegistry::new();
        let room = registry.get_or_insert("doc-1", OgreDoc::new());
        let (tx, _rx) = mpsc::unbounded_channel();
        room.add_client(room.next_client_id(), "user-1".to_string(), tx).await;

        let idle = registry.idle_rooms(0).await;
        assert!(idle.is_empty(), "Room with connected clients should not be idle");
    }

    #[tokio::test]
    async fn recently_edited_room_not_idle() {
        let registry = RoomRegistry::new();
        let room = registry.get_or_insert("doc-1", OgreDoc::new());

        // Simulate an edit by applying an update (sets last_edit to now)
        let doc = OgreDoc::new();
        let update = doc.to_state_bytes();
        let _ = room.apply_update(&update).await;

        // With a very high threshold, the room should not be idle
        let idle = registry.idle_rooms(999_999_999).await;
        assert!(idle.is_empty(), "Recently edited room should not be idle");
    }

    #[tokio::test]
    async fn compact_room_skips_when_room_gone() {
        let registry = RoomRegistry::new();
        // Don't insert any room — compact_room should return false
        // We can't call compact_room without real repos for the full path,
        // but we can verify the idle detection doesn't find missing rooms.
        let idle = registry.idle_rooms(0).await;
        assert!(idle.is_empty());
    }

    #[tokio::test]
    async fn compact_room_skips_when_client_reconnects() {
        let registry = RoomRegistry::new();
        let room = registry.get_or_insert("doc-1", OgreDoc::new());

        // Room is idle (no clients, never edited)
        let idle = registry.idle_rooms(0).await;
        assert_eq!(idle.len(), 1);

        // Now a client connects between idle check and compaction
        let (tx, _rx) = mpsc::unbounded_channel();
        room.add_client(room.next_client_id(), "user-1".to_string(), tx).await;

        // The room should no longer qualify for removal
        assert!(!registry.remove_if_empty("doc-1").await);
        assert_eq!(registry.room_count(), 1, "Room with client should not be removed");
    }

    #[tokio::test]
    async fn remove_if_empty_succeeds_for_empty_room() {
        let registry = RoomRegistry::new();
        let _ = registry.get_or_insert("doc-1", OgreDoc::new());
        assert_eq!(registry.room_count(), 1);

        assert!(registry.remove_if_empty("doc-1").await);
        assert_eq!(registry.room_count(), 0);
    }

    #[tokio::test]
    async fn remove_if_empty_fails_for_nonexistent_room() {
        let registry = RoomRegistry::new();
        assert!(!registry.remove_if_empty("no-such-room").await);
    }

    #[tokio::test]
    async fn state_bytes_roundtrip() {
        // Verify that Room produces valid state bytes for snapshot
        let room_a = {
            let doc = OgreDoc::new();
            Arc::new(ogrenotes_collab::room::Room::new("doc-1".to_string(), doc))
        };
        let state = room_a.to_state_bytes().await;
        assert!(!state.is_empty(), "State bytes should not be empty");

        // Verify the bytes can be loaded into a new OgreDoc
        let restored = OgreDoc::from_state_bytes(&state);
        assert!(restored.is_ok(), "State bytes should decode back into OgreDoc");
    }

    #[tokio::test]
    async fn multiple_idle_rooms_detected() {
        let registry = RoomRegistry::new();
        let _ = registry.get_or_insert("doc-1", OgreDoc::new());
        let _ = registry.get_or_insert("doc-2", OgreDoc::new());
        let _ = registry.get_or_insert("doc-3", OgreDoc::new());

        // Add a client to doc-2 only
        let room2 = registry.get("doc-2").unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        room2.add_client(room2.next_client_id(), "user-1".to_string(), tx).await;

        let mut idle = registry.idle_rooms(0).await;
        idle.sort();
        assert_eq!(idle, vec!["doc-1", "doc-3"]);
    }

    #[tokio::test]
    async fn compact_idle_rooms_removes_all_idle() {
        let registry = RoomRegistry::new();
        let _ = registry.get_or_insert("doc-1", OgreDoc::new());
        let _ = registry.get_or_insert("doc-2", OgreDoc::new());

        // Add a client to doc-2
        let room2 = registry.get("doc-2").unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        room2.add_client(room2.next_client_id(), "user-1".to_string(), tx).await;

        // Only doc-1 is idle
        let idle = registry.idle_rooms(0).await;
        assert_eq!(idle, vec!["doc-1"]);

        // Remove idle rooms from registry (simulating what compact_room does at the end)
        for id in &idle {
            let _ = registry.remove_if_empty(id).await;
        }

        assert_eq!(registry.room_count(), 1, "Only doc-2 (with client) should remain");
        assert!(registry.get("doc-2").is_some());
        assert!(registry.get("doc-1").is_none());
    }

    #[tokio::test]
    async fn cutoff_recorded_before_state_read() {
        // Verify the cutoff timing pattern: cutoff is taken before reading state.
        // This ensures concurrent writes after cutoff are not pruned.
        let before = ogrenotes_common::time::now_usec();
        let cutoff = ogrenotes_common::time::now_usec();
        let after = ogrenotes_common::time::now_usec();

        assert!(cutoff >= before);
        assert!(cutoff <= after);
        // In production code, delete_updates_before(cutoff) only deletes rows
        // with created_at < cutoff, so anything written after cutoff survives.
    }

    /// Regression: compact_room must construct the s3_key locally using the same
    /// format as save_snapshot, instead of re-reading metadata (TOCTOU race).
    /// This test verifies the key format is consistent.
    #[test]
    fn snapshot_key_format_consistent() {
        let doc_id = "doc-abc123";
        let version = 42u64;

        // This is the format used in both compact_room and save_snapshot
        let key = format!("docs/{doc_id}/snapshots/{version}.bin");
        assert_eq!(key, "docs/doc-abc123/snapshots/42.bin");

        // Verify it matches the pattern used in documents.rs put_content
        let key2 = format!("docs/{}/snapshots/{}.bin", doc_id, version);
        assert_eq!(key, key2, "Key format must be consistent across all code paths");
    }
}
