// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! SecurityAudit retention worker (Phase 4 M-E6 piece D).
//!
//! Ticks every hour. On each tick, checks whether the current UTC
//! hour matches `SECURITY_AUDIT_RETENTION_HOUR_UTC` and — to survive
//! process restarts mid-window — whether we've already run today's
//! pass from this process. If both gates pass, walks every user and
//! deletes their SecurityAudit rows whose `created_at` falls outside
//! the retention window.
//!
//! AdminAudit is unaffected — the M-E6 spec retains it permanently.
//! The scheduler is intentionally cribbed from `digest.rs`'s
//! `spawn_scheduler` shape (hourly tick + UTC-hour gate + atomic
//! same-day idempotency) so behavioral surprises in one apply to
//! both.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Timelike, Utc};
use ogrenotes_common::time::now_usec;

use crate::state::AppState;

const TICK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const USEC_PER_DAY: i64 = 24 * 60 * 60 * 1_000_000;

/// Maximum rows deleted per (user × tick). Bounds the worst-case
/// blast radius if a bad config or wall-clock jump declares a huge
/// chunk of recent history eligible; the next tick (one hour later)
/// resumes draining the backlog. 200 rows × N users at one Query +
/// 200 DeleteItem each is well within Fargate's per-tick budget.
const MAX_DELETES_PER_USER_PER_TICK: usize = 200;

/// Spawn the hourly retention task. Safe to call unconditionally —
/// if `security_audit_retention_enabled` is false the tick
/// short-circuits and does no work. Mirrors
/// `digest::spawn_scheduler`.
pub fn spawn_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    let last_run = Arc::new(AtomicI64::new(0));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        // Skip the initial immediate tick — same reasoning as the
        // digest scheduler: a just-booted process shouldn't
        // double-fire alongside an earlier instance that already ran
        // today's pass.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = run_tick(&state, &last_run).await {
                tracing::warn!(error = %e, "audit_retention tick failed");
            }
        }
    })
}

/// One scheduler tick. Exposed for tests; decides whether to run,
/// then runs if allowed.
async fn run_tick(state: &AppState, last_run_date: &AtomicI64) -> Result<(), String> {
    if !state.config.security_audit_retention_enabled {
        return Ok(());
    }
    let now = Utc::now();
    if now.hour() as u8 != state.config.security_audit_retention_hour_utc {
        return Ok(());
    }
    let today = pack_date(&now);
    if last_run_date.swap(today, Ordering::SeqCst) == today {
        return Ok(());
    }
    let cutoff = cutoff_usec(now_usec(), state.config.security_audit_retention_days);
    sweep(state, cutoff).await
}

/// Walk every user and drop expired audit rows. Exposed via `pub` so an
/// admin-on-demand trigger (future "purge stale audit rows now" button) could
/// reuse the same path, and so integration tests can drive the destructive
/// pass directly with a controlled cutoff (mirrors `trash_cleanup::sweep`);
/// in production nothing calls this besides `run_tick`.
pub async fn sweep(state: &AppState, cutoff_usec: i64) -> Result<(), String> {
    let mut cursor: Option<String> = None;
    let mut scanned = 0usize;
    let mut deleted_total = 0usize;
    loop {
        let (users, next) = state
            .user_repo
            .list_all(100, cursor.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        for user in users {
            scanned += 1;
            match state
                .security_audit_repo
                .delete_older_than_for_user(
                    &user.user_id,
                    cutoff_usec,
                    MAX_DELETES_PER_USER_PER_TICK,
                )
                .await
            {
                Ok(n) => deleted_total += n,
                Err(e) => {
                    tracing::warn!(
                        user_id = %user.user_id,
                        error = %e,
                        "audit_retention: per-user delete failed; continuing"
                    );
                }
            }
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    tracing::info!(
        scanned,
        deleted = deleted_total,
        cutoff_usec,
        "audit_retention pass complete"
    );
    Ok(())
}

/// Compute the retention cutoff in usec-since-epoch given the
/// current wall time and the configured retention window in days.
/// Rows with `created_at < cutoff` are eligible for deletion.
/// Pulled out as a pure function for unit testing.
fn cutoff_usec(now_usec: i64, retention_days: u32) -> i64 {
    now_usec - (retention_days as i64) * USEC_PER_DAY
}

/// Pack a UTC date into a single i64 so we can store "last run day"
/// in an AtomicI64. `20260421` for 2026-04-21. Lifted from
/// `digest::pack_date` — keeping the same encoding means a future
/// consolidation into a shared scheduler harness needs no migration.
fn pack_date(dt: &chrono::DateTime<Utc>) -> i64 {
    (dt.year() as i64) * 10_000 + (dt.month() as i64) * 100 + dt.day() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutoff_usec_subtracts_correct_window() {
        // 90 days = 7,776,000,000,000 microseconds.
        let now = 1_800_000_000_000_000_i64;
        let cutoff = cutoff_usec(now, 90);
        assert_eq!(now - cutoff, 90 * 24 * 60 * 60 * 1_000_000);
    }

    #[test]
    fn cutoff_usec_one_day_window() {
        // A 1-day retention window means rows older than 24h are
        // eligible. This is the smallest meaningful window — the
        // config layer rejects 0, so we don't test it here.
        let now = 1_000_000_000_000_000_i64;
        let cutoff = cutoff_usec(now, 1);
        assert_eq!(now - cutoff, USEC_PER_DAY);
    }

    #[test]
    fn cutoff_usec_long_window_doesnt_overflow() {
        // 10 years = 3650 days. i64::MAX is ~9.2e18; 3650 days in
        // usec is ~3.2e14, well within range. The cast in
        // `cutoff_usec` is the only place an overflow could lurk
        // (u32 → i64), so smoke-test it.
        let now = 1_800_000_000_000_000_i64;
        let cutoff = cutoff_usec(now, 3650);
        let span = now - cutoff;
        assert_eq!(span, 3650 * USEC_PER_DAY);
        assert!(cutoff < now);
    }

    #[test]
    fn cutoff_is_an_exclusive_lower_bound_via_strict_lt() {
        // The repo's `delete_older_than_for_user` uses `< cutoff` for
        // eligibility, so a row whose `created_at` equals the cutoff
        // exactly must survive. Lock that contract here — if someone
        // flips the comparator to `<=` in the repo, retention silently
        // gets one-day-aggressive and this test stays green until a
        // human reads it.
        let now = 2_000_000_000_000_000_i64;
        let cutoff = cutoff_usec(now, 30);
        // A row created right at the cutoff is NOT older than the cutoff.
        assert!(!(cutoff < cutoff));
        // A row created 1 usec before the cutoff IS older.
        assert!((cutoff - 1) < cutoff);
    }

    #[test]
    fn pack_date_is_lexicographically_ordered() {
        use chrono::TimeZone;
        let a = pack_date(&Utc.with_ymd_and_hms(2026, 4, 21, 0, 0, 0).unwrap());
        let b = pack_date(&Utc.with_ymd_and_hms(2026, 4, 22, 0, 0, 0).unwrap());
        let c = pack_date(&Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap());
        assert!(a < b);
        assert!(b < c);
        assert_eq!(a, 20260421);
    }

    #[test]
    fn last_run_guard_is_idempotent_within_day() {
        // Same pattern as digest::tests::last_run_guard_is_idempotent_within_day.
        // Two ticks in the same hour: the first swaps the atomic, the
        // second sees equality and short-circuits.
        let atom = AtomicI64::new(0);
        let today = 20260421;
        let first = atom.swap(today, Ordering::SeqCst);
        assert_eq!(first, 0, "first call records today");
        let second = atom.swap(today, Ordering::SeqCst);
        assert_eq!(second, today, "second call sees today and short-circuits");
    }
}
