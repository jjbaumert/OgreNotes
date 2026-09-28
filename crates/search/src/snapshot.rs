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

/// Write a snapshot's files into `dir`, which must not already hold an
/// index. File names are checked to be plain names (no path separators),
/// so a tampered snapshot can't write outside `dir`.
pub fn restore_snapshot(dir: &Path, snapshot: &IndexSnapshot) -> Result<(), SearchError> {
    if dir.join("meta.json").exists() {
        return Err(SearchError::Snapshot(format!(
            "{} already holds an index",
            dir.display()
        )));
    }
    if !snapshot.files.iter().any(|(name, _)| name == "meta.json") {
        return Err(SearchError::Snapshot("snapshot has no meta.json".into()));
    }
    std::fs::create_dir_all(dir).map_err(|e| SearchError::Snapshot(e.to_string()))?;
    for (name, bytes) in &snapshot.files {
        let plain = !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains(['/', '\\']);
        if !plain {
            return Err(SearchError::Snapshot(format!("refusing file name {name:?}")));
        }
        std::fs::write(dir.join(name), bytes)
            .map_err(|e| SearchError::Snapshot(format!("write {name}: {e}")))?;
    }
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
}
