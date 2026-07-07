// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Rolling-window active-user tracker.
//!
//! A 5-minute and 60-minute "have we seen this user recently?" set.
//! Fed from the auth middleware on every authenticated request; swept by
//! the metrics sampler into gauges.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;

pub struct RollingUsers {
    /// user_id → last-seen unix-millis.
    seen: DashMap<String, u64>,
}

impl RollingUsers {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { seen: DashMap::new() })
    }

    pub fn mark(&self, user_id: &str) {
        self.seen.insert(user_id.to_string(), now_ms());
    }

    /// Return counts of unique users seen in the last 5m and 60m windows.
    /// Also evicts entries older than 60m so the map stays bounded.
    pub fn sweep(&self) -> (usize, usize) {
        let now = now_ms();
        let cutoff_5m = now.saturating_sub(5 * 60 * 1000);
        let cutoff_60m = now.saturating_sub(60 * 60 * 1000);
        self.seen.retain(|_, ts| *ts >= cutoff_60m);
        // After retain, every surviving entry is by construction within the
        // 60m window — so in_60m is just the map size.
        let in_60m = self.seen.len();
        let in_5m = self
            .seen
            .iter()
            .filter(|e| *e.value() >= cutoff_5m)
            .count();
        (in_5m, in_60m)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_and_sweep_counts_users() {
        let ru = RollingUsers::new();
        ru.mark("alice");
        ru.mark("bob");
        ru.mark("alice"); // same user → dedup
        let (in_5m, in_60m) = ru.sweep();
        assert_eq!(in_5m, 2);
        assert_eq!(in_60m, 2);
    }

    #[test]
    fn sweep_evicts_old_entries() {
        let ru = RollingUsers::new();
        // Insert a stale entry directly
        ru.seen.insert("old".to_string(), 0);
        ru.mark("new");
        let (in_5m, in_60m) = ru.sweep();
        assert_eq!(in_5m, 1);
        assert_eq!(in_60m, 1);
        // "old" was evicted
        assert!(!ru.seen.contains_key("old"));
    }

    #[test]
    fn sweep_distinguishes_5m_from_60m_window() {
        // The whole point of the tracker: a user seen 10 minutes ago counts
        // toward the 60m active set but NOT the 5m one. The other tests keep
        // every entry inside both windows, so this boundary was uncovered.
        let ru = RollingUsers::new();
        ru.mark("recent"); // now → inside both windows
        ru.seen
            .insert("ten_min_ago".to_string(), now_ms() - 10 * 60 * 1000);
        let (in_5m, in_60m) = ru.sweep();
        assert_eq!(in_5m, 1, "only the just-marked user is within 5m");
        assert_eq!(in_60m, 2, "both users are within 60m");
        // The 10-min-old entry is outside 5m but not evicted (eviction is >60m).
        assert!(ru.seen.contains_key("ten_min_ago"));
    }

    #[test]
    fn remark_refreshes_into_5m_window() {
        // Re-marking a user who'd aged out of the 5m window pulls them back in.
        let ru = RollingUsers::new();
        ru.seen.insert("u".to_string(), now_ms() - 30 * 60 * 1000); // 30m ago
        let (in_5m, in_60m) = ru.sweep();
        assert_eq!(in_5m, 0, "30-min-old user is outside the 5m window");
        assert_eq!(in_60m, 1, "but still inside the 60m window");

        ru.mark("u"); // fresh activity
        let (in_5m_after, in_60m_after) = ru.sweep();
        assert_eq!(in_5m_after, 1, "re-marking moves the user back into 5m");
        assert_eq!(in_60m_after, 1);
    }
}
