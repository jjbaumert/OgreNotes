// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Point-in-time copies of an on-disk index, for backup and restore.
//!
//! The index is a local directory, so losing the disk (or moving to a new
//! server) loses search until every document is re-indexed. A snapshot is
//! the set of files that make up one committed state of the index; the
//! caller stores it wherever it likes (the API keeps it in S3) and writes
//! it back into an empty directory with [`restore_snapshot`].
//!
//! Consistency: a committed index is `meta.json` plus the files of the
//! segments it lists. The snapshot holds the writer lock, so no commit
//! lands while it copies, and copies `meta.json` first and then exactly
//! the segment files that meta names. Tantivy merges segments on its own
//! threads, which the lock does not pause; a merge can garbage-collect a
//! segment file between reading meta and copying it. That shows up as a
//! missing file, and the snapshot starts over (a few tries, then an
//! error) — it never returns a set of files that doesn't open.

use std::collections::HashSet;
use std::path::Path;

use crate::{SearchError, SearchIndex};

/// How many times to start over when a merge removes a file mid-copy.
const ATTEMPTS: usize = 5;

/// The files of one committed index state: `(file name, contents)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSnapshot {
    pub files: Vec<(String, Vec<u8>)>,
}

impl SearchIndex {
    /// Copy the current committed state of an on-disk index. `Ok(None)`
    /// for an in-memory index, which has nothing to copy.
    pub fn snapshot(&self) -> Result<Option<IndexSnapshot>, SearchError> {
        let Some(dir) = self.dir.as_deref() else { return Ok(None) };
        let _writer = self.writer.lock().map_err(|_| SearchError::WriterUnavailable)?;
        let mut last_err = String::new();
        for _ in 0..ATTEMPTS {
            match copy_committed(dir) {
                Ok(snapshot) => return Ok(Some(snapshot)),
                Err(e) => last_err = e,
            }
        }
        Err(SearchError::Snapshot(format!(
            "gave up after {ATTEMPTS} attempts: {last_err}"
        )))
    }
}

/// `meta.json` plus every file of every segment it lists.
fn copy_committed(dir: &Path) -> Result<IndexSnapshot, String> {
    let meta = std::fs::read(dir.join("meta.json")).map_err(|e| format!("read meta.json: {e}"))?;
    let segments = segment_ids(&meta)?;
    let mut files = vec![("meta.json".to_string(), meta)];
    let entries = std::fs::read_dir(dir).map_err(|e| format!("list index dir: {e}"))?;
    let mut seen = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("list index dir: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.split('.').next() else { continue };
        if !segments.contains(stem) {
            continue;
        }
        let bytes = std::fs::read(entry.path()).map_err(|e| format!("read {name}: {e}"))?;
        seen.insert(stem.to_string());
        files.push((name, bytes));
    }
    if let Some(missing) = segments.iter().find(|s| !seen.contains(*s)) {
        return Err(format!("segment {missing} has no files (merged away mid-copy?)"));
    }
    Ok(IndexSnapshot { files })
}

/// The segment ids `meta.json` lists, normalized to the form used in
/// file names (32 lowercase hex digits, no dashes).
fn segment_ids(meta: &[u8]) -> Result<HashSet<String>, String> {
    let meta: serde_json::Value =
        serde_json::from_slice(meta).map_err(|e| format!("parse meta.json: {e}"))?;
    let segments = meta["segments"]
        .as_array()
        .ok_or_else(|| "meta.json has no segments list".to_string())?;
    segments
        .iter()
        .map(|s| {
            s["segment_id"]
                .as_str()
                .map(|id| id.replace('-', "").to_ascii_lowercase())
                .ok_or_else(|| "segment without an id".to_string())
        })
        .collect()
}

/// Restore into a missing or empty directory, without replacing an index
/// or unrelated files. Files are written and validated in a private sibling
/// directory, then published with one atomic rename. A failed or interrupted
/// restore never exposes partial metadata at the destination.
///
/// A process killed before publication can leave an unused staging directory.
/// Future restores use a fresh directory and never delete unowned siblings.
pub fn restore_snapshot(dir: &Path, snapshot: &IndexSnapshot) -> Result<(), SearchError> {
    if dir.join("meta.json").exists() {
        return Err(SearchError::Snapshot(format!(
            "{} already holds an index",
            dir.display()
        )));
    }
    let mut names = HashSet::new();
    for (name, _) in &snapshot.files {
        let plain = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']);
        if !plain || !names.insert(name.as_str()) {
            return Err(SearchError::Snapshot(format!(
                "invalid or duplicate file name {name:?}"
            )));
        }
    }
    if !names.contains("meta.json") {
        return Err(SearchError::Snapshot("snapshot has no meta.json".into()));
    }
    let parent = dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| SearchError::Snapshot(e.to_string()))?;
    let staging = tempfile::Builder::new()
        .prefix(".ogrenotes-restore-")
        .tempdir_in(parent)
        .map_err(|e| SearchError::Snapshot(format!("create staging directory: {e}")))?;
    for (name, bytes) in &snapshot.files {
        use std::io::Write;
        let mut file = std::fs::File::create(staging.path().join(name))
            .map_err(|e| SearchError::Snapshot(format!("create {name}: {e}")))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| SearchError::Snapshot(format!("write {name}: {e}")))?;
    }
    // Opening the reader checks the referenced segment files, not just JSON
    // syntax. A corrupt or incompatible backup must allow startup to rebuild.
    {
        let index = tantivy::Index::open_in_dir(staging.path())?;
        if index.schema() != SearchIndex::build_schema().0 {
            return Err(SearchError::SchemaMismatch);
        }
        let _reader = index.reader()?;
    }
    sync_directory(staging.path())?;
    // rename cannot replace a nonempty directory, including an index created
    // by another process while this restore was being prepared. Do not remove
    // the destination first: that would both risk data loss and break atomicity.
    std::fs::rename(staging.path(), dir)
        .map_err(|e| SearchError::Snapshot(format!("publish restored index: {e}")))?;
    sync_directory(parent)?;
    Ok(())
}

fn sync_directory(dir: &Path) -> Result<(), SearchError> {
    // Directory fsync is available on Unix, where production is deployed.
    #[cfg(unix)]
    std::fs::File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(|e| SearchError::Snapshot(format!("sync directory {}: {e}", dir.display())))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SearchDocument, SearchQuery};

    fn q(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            doc_type: None,
            owner_id: None,
            folder_id: None,
            limit: 50,
            offset: 0,
        }
    }

    fn doc(id: &str, body: &str) -> SearchDocument {
        SearchDocument {
            doc_id: id.into(),
            title: format!("Title {id}"),
            body: body.into(),
            owner_id: "u1".into(),
            doc_type: "document".into(),
            folder_id: None,
            workspace_id: None,
            updated_at: 1,
            created_at: 1,
        }
    }

    #[test]
    fn a_restored_snapshot_opens_and_finds_what_was_indexed() {
        let src = tempfile::tempdir().unwrap();
        let index = SearchIndex::open_or_create(src.path()).unwrap();
        for i in 0..20 {
            index.index_document(&doc(&format!("d{i}"), &format!("marmot number {i}"))).unwrap();
        }
        index.index_document(&doc("x", "a lone quetzal")).unwrap();
        let snapshot = index.snapshot().unwrap().expect("on-disk index");
        assert!(snapshot.files.iter().any(|(n, _)| n == "meta.json"));
        assert!(
            snapshot.files.iter().all(|(n, _)| !n.contains("lock")),
            "lock files are not part of a snapshot: {:?}",
            snapshot.files.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );

        let dst = tempfile::tempdir().unwrap();
        let dst_dir = dst.path().join("restored");
        restore_snapshot(&dst_dir, &snapshot).unwrap();
        let restored = SearchIndex::open_or_create(&dst_dir).unwrap();
        let hits = restored.search(&q("quetzal")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "x");
        assert_eq!(restored.count(&q("marmot")).unwrap(), 20);
    }

    #[test]
    fn an_in_memory_index_has_no_snapshot() {
        assert!(SearchIndex::open_in_memory().unwrap().snapshot().unwrap().is_none());
    }

    #[test]
    fn restore_refuses_path_escapes_and_existing_indexes() {
        let dst = tempfile::tempdir().unwrap();
        for bad in ["../evil", "a/b", ".."] {
            let snap = IndexSnapshot {
                files: vec![("meta.json".into(), b"{}".to_vec()), (bad.into(), vec![])],
            };
            assert!(restore_snapshot(&dst.path().join(format!("x{}", bad.len())), &snap).is_err(), "{bad}");
        }
        std::fs::write(dst.path().join("meta.json"), b"{}").unwrap();
        let ok = IndexSnapshot { files: vec![("meta.json".into(), b"{}".to_vec())] };
        assert!(restore_snapshot(dst.path(), &ok).is_err(), "never overwrite an index");
    }

    #[test]
    fn failed_restore_does_not_publish_metadata_and_can_be_retried() {
        let src = tempfile::tempdir().unwrap();
        let index = SearchIndex::open_or_create(src.path()).unwrap();
        index.index_document(&doc("recovered", "capybara")).unwrap();
        let snapshot = index.snapshot().unwrap().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let dir = dst.path().join("restored");
        let mut broken = snapshot.clone();
        // A plain name that cannot be written on the filesystem. Earlier
        // snapshot files have already been written when this write fails.
        broken.files.push(("x".repeat(4096), vec![1]));
        assert!(restore_snapshot(&dir, &broken).is_err());
        assert!(
            !dir.join("meta.json").exists(),
            "failed restore was published"
        );
        assert_eq!(
            std::fs::read_dir(dst.path()).unwrap().count(),
            0,
            "staging files leaked"
        );
        restore_snapshot(&dir, &snapshot).unwrap();
        let restored = SearchIndex::open_or_create(&dir).unwrap();
        assert_eq!(
            restored.search(&q("capybara")).unwrap()[0].doc_id,
            "recovered"
        );
    }

    #[test]
    fn an_incomplete_snapshot_is_not_published() {
        let src = tempfile::tempdir().unwrap();
        let index = SearchIndex::open_or_create(src.path()).unwrap();
        index.index_document(&doc("d1", "capybara")).unwrap();
        let mut snapshot = index.snapshot().unwrap().unwrap();
        snapshot.files.retain(|(name, _)| name == "meta.json");
        let dst = tempfile::tempdir().unwrap();
        let dir = dst.path().join("restored");
        assert!(restore_snapshot(&dir, &snapshot).is_err());
        assert!(!dir.exists());
        // This is the same fallback the server uses when restore fails.
        assert!(SearchIndex::open_or_create(&dir).is_ok());
    }

    #[test]
    fn restore_preserves_unrelated_destination_files() {
        let src = tempfile::tempdir().unwrap();
        let index = SearchIndex::open_or_create(src.path()).unwrap();
        let snapshot = index.snapshot().unwrap().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(dst.path().join("operator-notes"), "keep me").unwrap();
        assert!(restore_snapshot(dst.path(), &snapshot).is_err());
        assert_eq!(
            std::fs::read_to_string(dst.path().join("operator-notes")).unwrap(),
            "keep me"
        );
        assert!(!dst.path().join("meta.json").exists());
    }

    #[test]
    fn an_interrupted_staging_directory_does_not_block_a_new_restore() {
        let src = tempfile::tempdir().unwrap();
        let index = SearchIndex::open_or_create(src.path()).unwrap();
        index.index_document(&doc("d1", "capybara")).unwrap();
        let snapshot = index.snapshot().unwrap().unwrap();
        let dst = tempfile::tempdir().unwrap();
        // A killed process cannot run TempDir::drop. Its private staging
        // directory must neither be mistaken for the index nor reused.
        let abandoned = dst.path().join(".ogrenotes-restore-abandoned");
        std::fs::create_dir(&abandoned).unwrap();
        std::fs::write(abandoned.join("meta.json"), &snapshot.files[0].1).unwrap();
        let dir = dst.path().join("restored");
        std::fs::create_dir(&dir).unwrap();
        restore_snapshot(&dir, &snapshot).unwrap();
        let restored = SearchIndex::open_or_create(&dir).unwrap();
        assert_eq!(restored.search(&q("capybara")).unwrap()[0].doc_id, "d1");
        assert!(
            abandoned.exists(),
            "never delete unowned sibling directories"
        );
    }

    #[test]
    fn restore_never_replaces_an_existing_valid_index() {
        let src = tempfile::tempdir().unwrap();
        let source = SearchIndex::open_or_create(src.path()).unwrap();
        source.index_document(&doc("new", "capybara")).unwrap();
        let snapshot = source.snapshot().unwrap().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let existing = SearchIndex::open_or_create(dst.path()).unwrap();
        existing.index_document(&doc("existing", "marmot")).unwrap();
        assert!(restore_snapshot(dst.path(), &snapshot).is_err());
        assert_eq!(existing.search(&q("marmot")).unwrap()[0].doc_id, "existing");
        assert!(existing.search(&q("capybara")).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn restore_does_not_follow_a_destination_symlink() {
        let src = tempfile::tempdir().unwrap();
        let source = SearchIndex::open_or_create(src.path()).unwrap();
        let snapshot = source.snapshot().unwrap().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let unrelated = dst.path().join("unrelated");
        std::fs::create_dir(&unrelated).unwrap();
        let link = dst.path().join("index");
        std::os::unix::fs::symlink(&unrelated, &link).unwrap();
        assert!(restore_snapshot(&link, &snapshot).is_err());
        assert!(link.is_symlink());
        assert_eq!(std::fs::read_dir(unrelated).unwrap().count(), 0);
    }
}
