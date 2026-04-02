//! Background task for compacting idle collaboration rooms.
//!
//! When a room has been idle (no edits, no clients) for a configurable threshold,
//! the compaction task snapshots the CRDT state to S3, prunes the UPDATE# rows
//! from DynamoDB, and removes the room from the in-memory registry.

use std::sync::Arc;

use ogrenotes_collab::room::RoomRegistry;
use ogrenotes_storage::models::snapshot::DocSnapshot;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::snapshot_repo::SnapshotRepo;

/// Spawn a background task that periodically compacts idle rooms.
///
/// - Checks every `interval` duration.
/// - Rooms idle for >= `idle_threshold_ms` with 0 clients get compacted.
/// - Compaction: snapshot to S3, prune UPDATE# rows, remove from registry.
pub fn spawn_compaction_task(
    registry: Arc<RoomRegistry>,
    doc_repo: Arc<DocRepo>,
    snapshot_repo: Arc<SnapshotRepo>,
    interval: std::time::Duration,
    idle_threshold_ms: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;

            let idle_doc_ids = registry.idle_rooms(idle_threshold_ms).await;

            for doc_id in idle_doc_ids {
                // Re-check that the room still exists and is still empty.
                let room = match registry.get(&doc_id) {
                    Some(r) => r,
                    None => continue,
                };

                if room.client_count().await > 0 {
                    continue;
                }

                // Record the cutoff time BEFORE reading state, so any updates
                // written after this point are preserved.
                let cutoff = ogrenotes_common::time::now_usec();

                // Snapshot the current CRDT state.
                let state_bytes = room.to_state_bytes().await;

                // Determine the next snapshot version.
                let new_version = match doc_repo.get(&doc_id).await {
                    Ok(Some(meta)) => meta.snapshot_version + 1,
                    _ => continue,
                };

                // Save snapshot to S3 and update DynamoDB metadata.
                if let Err(e) = doc_repo
                    .save_snapshot(&doc_id, &state_bytes, new_version, cutoff)
                    .await
                {
                    tracing::warn!(doc_id, error = %e, "compaction: snapshot save failed");
                    continue;
                }

                // Write a SNAPSHOT# entry for edit history.
                // Re-read metadata to get the canonical s3_key set by save_snapshot.
                let s3_key = match doc_repo.get(&doc_id).await {
                    Ok(Some(meta)) => meta.snapshot_s3_key.unwrap_or_else(|| {
                        format!("docs/{doc_id}/snapshots/{new_version}.bin")
                    }),
                    _ => format!("docs/{doc_id}/snapshots/{new_version}.bin"),
                };
                let snap = DocSnapshot {
                    doc_id: doc_id.clone(),
                    version: new_version,
                    s3_key,
                    size_bytes: state_bytes.len() as u64,
                    user_id: "system".to_string(),
                    created_at: cutoff,
                };
                if let Err(e) = snapshot_repo.create(&snap).await {
                    tracing::warn!(doc_id, error = %e, "compaction: snapshot record write failed");
                    // Non-fatal: the snapshot is in S3 and metadata is updated
                }

                // Prune only UPDATE# rows created before the snapshot cutoff.
                match doc_repo.delete_updates_before(&doc_id, cutoff).await {
                    Ok(count) if count > 0 => {
                        tracing::info!(doc_id, count, "compaction: pruned updates");
                    }
                    Err(e) => {
                        tracing::warn!(doc_id, error = %e, "compaction: update pruning failed");
                    }
                    _ => {}
                }

                // Remove room from registry.
                let _ = registry.remove_if_empty(&doc_id).await;

                tracing::info!(doc_id, version = new_version, "compaction: room compacted");
            }
        }
    })
}
