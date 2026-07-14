// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Short-TTL, in-memory cache of folder `inherit_mode` (#37).
//!
//! `fetch_folder_grants` runs inside `check_doc_access` on every REST doc
//! operation and reads each containing folder's metadata (`inherit_mode`)
//! to decide inherited access. That folder-global value rarely changes, so
//! this cache lets the hot REST path skip the `folder_repo.get` `GetItem`.
//!
//! **Only `inherit_mode` is cached** — the per-user `get_member` grant is
//! always fetched authoritatively. The cache is consulted **only on the
//! REST path**; the WebSocket-connect access check
//! (`check_doc_access_uncached`) bypasses it entirely, because a stale
//! grant there would bake the wrong write authority into a long-lived live
//! session (#37's CRDT caveat).
//!
//! Two safety nets bound staleness after a folder is restricted / deleted:
//!   1. a short TTL (`TTL_USEC`), and
//!   2. explicit `invalidate` on the folder-metadata mutations
//!      (`update_folder`, `delete_folder`) in `routes/folders.rs`.
//!
//! The cache is in-memory per API instance. On a single instance (the
//! current deploy) `invalidate` is synchronous and immediate; if the
//! service scales out, a peer instance's entry stays stale only until the
//! TTL elapses. The WS path being uncached keeps the dangerous case
//! (live-session write authority) always authoritative regardless.

use dashmap::DashMap;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::InheritMode;

/// How long a cached `inherit_mode` is trusted before a re-read. Short by
/// design — it is the staleness bound for a folder that is restricted on a
/// peer instance whose local `invalidate` never fired.
const TTL_USEC: i64 = 5_000_000; // 5 seconds

/// Short-TTL cache of `folder_id -> inherit_mode` for the REST access path.
#[derive(Default)]
pub struct FolderInheritCache {
    entries: DashMap<String, (InheritMode, i64)>,
}

impl FolderInheritCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the cached `inherit_mode` for `folder_id` if present and still
    /// within the TTL; otherwise `None` (the caller then reads
    /// authoritatively and calls [`insert`](Self::insert)).
    pub fn get(&self, folder_id: &str) -> Option<InheritMode> {
        let entry = self.entries.get(folder_id)?;
        let (mode, inserted) = entry.value();
        if now_usec() - inserted < TTL_USEC {
            Some(mode.clone())
        } else {
            None
        }
    }

    /// Record `folder_id`'s current `inherit_mode`, stamped now.
    pub fn insert(&self, folder_id: &str, mode: InheritMode) {
        self.entries
            .insert(folder_id.to_string(), (mode, now_usec()));
    }

    /// Drop `folder_id`'s entry so the next read is authoritative. Called
    /// on every folder-metadata mutation that could change `inherit_mode`
    /// (folder update / delete).
    pub fn invalidate(&self, folder_id: &str) {
        self.entries.remove(folder_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_returns_inserted_mode() {
        let cache = FolderInheritCache::new();
        cache.insert("f1", InheritMode::Restricted);
        assert_eq!(cache.get("f1"), Some(InheritMode::Restricted));
        // A never-inserted folder is a miss.
        assert_eq!(cache.get("f2"), None);
    }

    #[test]
    fn invalidate_forces_a_miss() {
        // The security-critical property: after invalidation (folder
        // restricted / deleted) the entry is gone, so the next access check
        // re-reads authoritatively rather than serving a stale grant.
        let cache = FolderInheritCache::new();
        cache.insert("f1", InheritMode::Inherit);
        assert_eq!(cache.get("f1"), Some(InheritMode::Inherit));
        cache.invalidate("f1");
        assert_eq!(cache.get("f1"), None);
    }

    #[test]
    fn expired_entry_is_a_miss() {
        // Simulate an entry older than the TTL by writing the pair
        // directly with a stale timestamp.
        let cache = FolderInheritCache::new();
        cache
            .entries
            .insert("f1".to_string(), (InheritMode::Inherit, now_usec() - TTL_USEC - 1));
        assert_eq!(cache.get("f1"), None, "entry past the TTL must miss");
    }
}
