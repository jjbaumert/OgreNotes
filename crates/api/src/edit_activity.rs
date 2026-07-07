// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Cross-path Edit-activity debouncer.
//!
//! Both `routes/ws.rs` (CRDT updates) and `routes/documents.rs::put_content`
//! (REST saves) emit `ActivityEventType::Edit` rows. Without a shared
//! cooldown, a user with a live WS session who also autosaves via REST
//! would see two Edit entries per window in the activity feed — one from
//! each path. This module owns a process-global cache keyed by
//! `(doc_id, user_id)` so both call sites consult the same cooldown.
//!
//! The stored value is the last-write timestamp (microseconds). A new
//! event is permitted when `now - stored >= DEBOUNCE_USEC`, which also
//! updates the entry. The check and update are atomic at the DashMap
//! entry level — concurrent Update frames from two WS connections of the
//! same user on the same doc serialize through `entry().and_modify(...)`.

use dashmap::DashMap;
use ogrenotes_common::time::now_usec;

/// Window per (doc_id, user_id) during which exactly one Edit activity
/// row is recorded. Longer than the 5-minute `last_active_at` debounce
/// on purpose: Edit rows show up in the activity feed and should not
/// dominate the history stream during a sustained editing session.
pub const DEBOUNCE_USEC: i64 = 60 * 1_000_000;

#[derive(Default)]
pub struct EditActivityDebouncer {
    last_write: DashMap<(String, String), i64>,
}

impl EditActivityDebouncer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve an Edit-activity slot for `(doc_id, user_id)` at `now`. If
    /// the last reservation for this pair is older than `DEBOUNCE_USEC`,
    /// updates the stored timestamp and returns `true`. Otherwise leaves
    /// the cache untouched and returns `false`.
    pub fn try_record(&self, doc_id: &str, user_id: &str, now: i64) -> bool {
        let key = (doc_id.to_string(), user_id.to_string());
        let mut allowed = false;
        self.last_write
            .entry(key)
            .and_modify(|ts| {
                if now - *ts >= DEBOUNCE_USEC {
                    *ts = now;
                    allowed = true;
                }
            })
            .or_insert_with(|| {
                allowed = true;
                now
            });
        allowed
    }

    /// Shorthand that calls `try_record` with `now = now_usec()`.
    pub fn try_record_now(&self, doc_id: &str, user_id: &str) -> bool {
        self.try_record(doc_id, user_id, now_usec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_is_always_allowed() {
        let d = EditActivityDebouncer::new();
        assert!(d.try_record("doc", "user", 1_000_000));
    }

    #[test]
    fn second_call_within_window_is_rejected() {
        let d = EditActivityDebouncer::new();
        assert!(d.try_record("doc", "user", 1_000_000));
        assert!(!d.try_record("doc", "user", 1_000_000 + DEBOUNCE_USEC - 1));
    }

    #[test]
    fn call_past_window_is_allowed() {
        let d = EditActivityDebouncer::new();
        assert!(d.try_record("doc", "user", 1_000_000));
        assert!(d.try_record("doc", "user", 1_000_000 + DEBOUNCE_USEC));
    }

    #[test]
    fn separate_docs_do_not_share_cooldown() {
        let d = EditActivityDebouncer::new();
        assert!(d.try_record("doc-a", "user", 1_000_000));
        // Same user, different doc → independent cooldown.
        assert!(d.try_record("doc-b", "user", 1_000_000));
    }

    #[test]
    fn separate_users_do_not_share_cooldown() {
        let d = EditActivityDebouncer::new();
        assert!(d.try_record("doc", "alice", 1_000_000));
        assert!(d.try_record("doc", "bob", 1_000_000));
    }
}
