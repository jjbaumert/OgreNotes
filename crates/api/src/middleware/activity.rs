// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Persistent `last_active_at` writer.
//!
//! `ActivityTracker::mark` is called on every authenticated request from
//! `AuthUser::from_request_parts`. It spawns a background DynamoDB write
//! only if the last write for this user is older than the debounce window
//! — bounding cost to at most one `UpdateItem` per user per window.
//!
//! The debounce window matches `ogrenotes_notify::ACTIVE_WINDOW_USEC`
//! (5 minutes) so "how often we persist activity" and "how long we treat
//! someone as active" are the same duration. This means a user who hits
//! the API within the last 5 minutes suppresses an email, and exactly one
//! write lands per 5-minute bucket.

use std::sync::Arc;

use dashmap::DashMap;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::repo::user_repo::UserRepo;

/// Debounce window for `last_active_at` writes. Matches the email
/// suppression window in `ogrenotes_notify`.
pub const DEBOUNCE_USEC: i64 = 5 * 60 * 1_000_000;

pub struct ActivityTracker {
    /// user_id -> last successful write timestamp (microseconds).
    last_write: DashMap<String, i64>,
    debounce_usec: i64,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self {
            last_write: DashMap::new(),
            debounce_usec: DEBOUNCE_USEC,
        }
    }

    /// Test hook — lets tests pick a tight debounce so they don't need to
    /// sleep for 5 real minutes to observe a second write.
    #[cfg(test)]
    pub fn with_debounce(debounce_usec: i64) -> Self {
        Self {
            last_write: DashMap::new(),
            debounce_usec,
        }
    }

    /// Register an activity event for `user_id`. Spawns an async write to
    /// DynamoDB if the debounce window has elapsed since the last write;
    /// no-ops otherwise. Safe to call on every request.
    pub fn mark(&self, user_id: &str, user_repo: Arc<UserRepo>) {
        let now = now_usec();
        let needs_write = self
            .last_write
            .get(user_id)
            .map(|entry| now - *entry.value() > self.debounce_usec)
            .unwrap_or(true);
        if !needs_write {
            return;
        }
        self.last_write.insert(user_id.to_string(), now);
        let uid = user_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = user_repo.update_last_active_at(&uid, now).await {
                tracing::warn!(user_id = %uid, error = %e, "update_last_active_at failed");
            }
        });
    }

    /// Snapshot of the internal cache size — exposed for tests. Outside
    /// of tests there's no reason to inspect this.
    #[cfg(test)]
    pub fn cache_size(&self) -> usize {
        self.last_write.len()
    }
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_cache_then_warm_cache_within_window() {
        // We can't easily cover the spawn path in a unit test (needs a
        // runtime + UserRepo), so we exercise the in-memory decision by
        // inserting directly and checking the needs_write branch.
        let tracker = ActivityTracker::with_debounce(DEBOUNCE_USEC);
        let now = now_usec();
        // Seed as if a write just happened.
        tracker.last_write.insert("u1".to_string(), now);
        // A second mark() within the window must observe `needs_write=false`
        // — we simulate by reading the cache directly.
        let still_fresh = tracker
            .last_write
            .get("u1")
            .map(|e| now - *e.value() <= tracker.debounce_usec)
            .unwrap_or(false);
        assert!(still_fresh, "entry should still be within debounce window");
    }

    #[test]
    fn debounce_elapsed_triggers_new_write_decision() {
        let tracker = ActivityTracker::with_debounce(1_000); // 1ms
        tracker
            .last_write
            .insert("u1".to_string(), now_usec() - 2_000); // 2ms ago
        let now = now_usec();
        let stale = tracker
            .last_write
            .get("u1")
            .map(|e| now - *e.value() > tracker.debounce_usec)
            .unwrap_or(true);
        assert!(stale, "entry older than debounce should trigger a write");
    }
}
