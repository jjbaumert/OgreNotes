// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Back the local search index up to S3, restore it on an empty disk, and
//! rebuild it from DynamoDB.
//!
//! The search index is a local Tantivy directory — rebuildable, but only
//! by re-reading every document. For a self-hosted server that makes a
//! lost disk or a move to new hardware a long, silent stretch of empty
//! search. So:
//!
//! - every `SEARCH_BACKUP_INTERVAL_MINS` the index is snapshotted
//!   ([`ogrenotes_search::SearchIndex::snapshot`]) and uploaded to
//!   `search-index/snapshot.zip` in the bucket ([`spawn_scheduler`]);
//! - at startup, an empty index directory is restored from that snapshot
//!   ([`restore_if_empty`]) and then brought up to date by re-indexing
//!   the documents changed since it was taken ([`reindex`] with `since`);
//! - with no snapshot to restore, an empty index is rebuilt from every
//!   document ([`reindex`] with no `since`) — also available on demand
//!   to admins as `POST /api/v1/admin/search/reindex`.
//!
//! Re-indexing only updates this process's Tantivy index; embeddings live
//! in the shared vector store and are left alone.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ogrenotes_search::{IndexSnapshot, restore_snapshot};
use ogrenotes_storage::s3::S3Client;

use crate::state::AppState;

/// Where the snapshot lives in the bucket.
pub const SNAPSHOT_KEY: &str = "search-index/snapshot.zip";
/// The entry inside the zip recording when the snapshot was taken.
const MANIFEST: &str = "ogrenotes-snapshot.json";
/// Documents changed this long before a snapshot are re-indexed after
/// restoring it too, to cover a commit racing the snapshot.
const CATCH_UP_MARGIN_USEC: i64 = 10 * 60 * 1_000_000;

/// One re-index at a time.
static REINDEXING: AtomicBool = AtomicBool::new(false);

/// Zip a snapshot, with a manifest recording `taken_at` (µs since epoch).
pub fn encode(snapshot: &IndexSnapshot, taken_at: i64) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file(MANIFEST, options).map_err(|e| e.to_string())?;
    zip.write_all(serde_json::json!({ "taken_at": taken_at }).to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    for (name, bytes) in &snapshot.files {
        zip.start_file(name.as_str(), options).map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

/// Inverse of [`encode`]: the snapshot and when it was taken.
pub fn decode(bytes: &[u8]) -> Result<(IndexSnapshot, i64), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut taken_at = None;
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        if name == MANIFEST {
            let manifest: serde_json::Value =
                serde_json::from_slice(&buf).map_err(|e| format!("manifest: {e}"))?;
            taken_at = manifest["taken_at"].as_i64();
        } else {
            files.push((name, buf));
        }
    }
    let taken_at = taken_at.ok_or_else(|| "snapshot has no manifest".to_string())?;
    Ok((IndexSnapshot { files }, taken_at))
}

/// Snapshot the index and upload it. `Ok(false)` for an in-memory index.
pub async fn back_up(state: &AppState) -> Result<bool, String> {
    let index = state.search_index.clone();
    let taken_at = ogrenotes_common::time::now_usec();
    // Reading the index directory is blocking file I/O under the writer lock.
    let snapshot = tokio::task::spawn_blocking(move || index.snapshot())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let Some(snapshot) = snapshot else { return Ok(false) };
    let bytes = encode(&snapshot, taken_at)?;
    let size = bytes.len();
    state
        .doc_repo
        .s3()
        .put_object(SNAPSHOT_KEY, bytes)
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(files = snapshot.files.len(), bytes = size, "search index backed up");
    Ok(true)
}

/// Back the index up every `search_backup_interval_mins` (0 = never).
/// Skips a round while storage is unreachable.
pub fn spawn_scheduler(state: AppState) -> Option<tokio::task::JoinHandle<()>> {
    let mins = state.config.search_backup_interval_mins;
    if mins == 0 {
        return None;
    }
    Some(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(u64::from(mins) * 60));
        ticker.tick().await; // the first tick is immediate; wait a full interval
        loop {
            ticker.tick().await;
            if !state.storage_health.is_up() {
                continue;
            }
            if let Err(e) = back_up(&state).await {
                tracing::warn!(error = %e, "search index backup failed");
            }
        }
    }))
}

/// Before the index is opened: if `dir` holds no index and the bucket has
/// a snapshot, restore it and return when it was taken. `None` when there
/// was nothing to do or restoring failed (the caller then rebuilds).
pub async fn restore_if_empty(s3: &S3Client, dir: &Path) -> Option<i64> {
    if dir.join("meta.json").exists() {
        return None;
    }
    match s3.object_exists(SNAPSHOT_KEY).await {
        Ok(true) => {}
        Ok(false) => return None,
        Err(e) => {
            tracing::warn!(error = %e, "search index: could not check for a snapshot to restore");
            return None;
        }
    }
    let restored = async {
        let bytes = s3.get_object(SNAPSHOT_KEY).await.map_err(|e| e.to_string())?;
        let (snapshot, taken_at) = decode(&bytes)?;
        restore_snapshot(dir, &snapshot).map_err(|e| e.to_string())?;
        Ok::<_, String>((taken_at, snapshot.files.len()))
    }
    .await;
    match restored {
        Ok((taken_at, files)) => {
            tracing::info!(files, taken_at, "search index restored from snapshot");
            Some(taken_at)
        }
        Err(e) => {
            tracing::warn!(error = %e, "search index: snapshot restore failed; will rebuild");
            // restore_snapshot publishes atomically and owns its staging
            // cleanup. Never delete pre-existing or concurrently created data.
            None
        }
    }
}

/// Whether a re-index is running right now.
pub fn is_reindexing() -> bool {
    REINDEXING.load(Ordering::Acquire)
}

/// What a re-index did.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReindexReport {
    pub scanned: usize,
    pub indexed: usize,
    pub removed: usize,
}

/// Re-index documents from DynamoDB into this process's index: every
/// document when `since` is `None`, otherwise those changed at or after
/// `since` (µs since epoch, less a margin) plus any with edits not yet
/// folded into a snapshot (live edits bump `updated_at` only at
/// compaction). Trashed documents are removed from the index. Returns
/// `Err` if another re-index is already running.
pub async fn reindex(state: &AppState, since: Option<i64>) -> Result<ReindexReport, String> {
    if REINDEXING.swap(true, Ordering::AcqRel) {
        return Err("a re-index is already running".into());
    }
    let result = reindex_inner(state, since).await;
    REINDEXING.store(false, Ordering::Release);
    result
}

async fn reindex_inner(state: &AppState, since: Option<i64>) -> Result<ReindexReport, String> {
    let cutoff = since.map(|t| t - CATCH_UP_MARGIN_USEC);
    let mut report = ReindexReport::default();
    let mut cursor = None;
    loop {
        let (metas, next) = state
            .doc_repo
            .list_all_meta(100, cursor)
            .await
            .map_err(|e| format!("list documents: {e}"))?;
        for meta in metas {
            report.scanned += 1;
            if meta.is_deleted {
                if state.search_index.delete_document(&meta.doc_id).is_ok() {
                    report.removed += 1;
                }
                continue;
            }
            let changed = match cutoff {
                None => true,
                Some(t) if meta.updated_at >= t => true,
                Some(_) => state.doc_repo.has_pending_updates(&meta.doc_id).await.unwrap_or(true),
            };
            if changed {
                crate::routes::documents::index_document_now(state, meta, false).await;
                report.indexed += 1;
            }
        }
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    tracing::info!(?report, full = since.is_none(), "search re-index complete");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trips() {
        let snapshot = IndexSnapshot {
            files: vec![
                ("meta.json".into(), b"{\"segments\":[]}".to_vec()),
                ("abc.idx".into(), vec![0, 1, 2, 255]),
            ],
        };
        let (back, taken_at) = decode(&encode(&snapshot, 1234).unwrap()).unwrap();
        assert_eq!(back, snapshot);
        assert_eq!(taken_at, 1234);
    }

    #[test]
    fn a_zip_without_a_manifest_is_rejected() {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("meta.json", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"{}").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(decode(&bytes).is_err());
    }
}
