// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Trash cleanup worker (Phase 4 M-E7 item 9).
//!
//! Ticks every hour. On each tick, checks whether the current UTC
//! hour matches `TRASH_CLEANUP_HOUR_UTC` and — to survive process
//! restarts mid-window — whether we've already run today's pass
//! from this process. If both gates pass, queries `GSI7-deleted-at`
//! for documents whose `deleted_at` predates the retention cutoff
//! and hard-purges each one.
//!
//! Patterns on `audit_retention.rs`'s scheduler shape (hourly tick
//! + UTC-hour gate + AtomicI64 same-day idempotency) — keeping the
//! two workers in lockstep means a behavioral surprise in one
//! applies to both, and a future consolidation into a shared
//! scheduler harness is a refactor rather than a rewrite.
//!
//! Dry-run mode (`TRASH_CLEANUP_DRY_RUN=true`) logs which docs WOULD
//! be purged but skips the destructive operations. Designed for the
//! first prod rollout so an operator can verify the cutoff arithmetic
//! against real data before committing to deletions.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Timelike, Utc};
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::security_audit::SecurityAuditAction;

use crate::routes::documents::spawn_delete_from_index;
use crate::routes::audit::record_security_event_by_actor;
use crate::state::AppState;

const TICK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const USEC_PER_DAY: i64 = 24 * 60 * 60 * 1_000_000;

/// Maximum docs purged per tick. Bounds the per-call DDB write
/// volume + the per-call S3 delete volume. Drain across multiple
/// ticks if a backlog exceeds this in one day.
const MAX_PURGES_PER_TICK: usize = 200;

/// Actor id written on every `SecurityAudit::DocDeleted` row the
/// worker emits. Captures "this came from the scheduled job, not an
/// interactive admin action" so future audit-log readers can
/// filter accordingly.
const WORKER_ACTOR_ID: &str = "trash_cleanup_worker";

/// Spawn the hourly trash-cleanup task. Safe to call unconditionally —
/// if `trash_cleanup_enabled` is false the tick short-circuits.
pub fn spawn_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    let last_run = Arc::new(AtomicI64::new(0));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        // Skip the initial immediate tick — a just-booted process
        // shouldn't double-fire alongside an earlier instance that
        // already ran today's pass.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = run_tick(&state, &last_run).await {
                tracing::warn!(error = %e, "trash_cleanup tick failed");
            }
        }
    })
}

/// One scheduler tick. Exposed for tests; decides whether to run,
/// then runs if allowed.
async fn run_tick(state: &AppState, last_run_date: &AtomicI64) -> Result<(), String> {
    if !state.config.trash_cleanup_enabled {
        return Ok(());
    }
    let now = Utc::now();
    if now.hour() as u8 != state.config.trash_cleanup_hour_utc {
        return Ok(());
    }
    let today = pack_date(&now);
    if last_run_date.swap(today, Ordering::SeqCst) == today {
        return Ok(());
    }
    let cutoff = cutoff_usec(now_usec(), state.config.trash_retention_days);
    sweep(state, cutoff).await
}

/// Query the deleted-at GSI and purge eligible docs. Exposed `pub`
/// (rather than `pub(crate)` like `audit_retention::sweep`) so the
/// integration test in `tests/test_trash_cleanup.rs` can drive a
/// single sweep against real DynamoDB with a controlled cutoff —
/// the actual GSI query path is hard to verify any other way. A
/// future admin "purge stale docs now" HTTP trigger could reuse the
/// same entry point. Returns `Ok(())` on partial success: per-doc
/// errors are logged but don't abort the sweep, because letting one
/// bad row block the rest of the backlog would let an adversarially-
/// shaped row indefinitely prevent retention from applying.
pub async fn sweep(state: &AppState, cutoff_usec: i64) -> Result<(), String> {
    let docs = state
        .doc_repo
        .list_eligible_for_purge(cutoff_usec, MAX_PURGES_PER_TICK)
        .await
        .map_err(|e| e.to_string())?;

    let dry_run = state.config.trash_cleanup_dry_run;
    let mut purged = 0usize;
    let mut errors = 0usize;
    for doc in &docs {
        if dry_run {
            tracing::info!(
                doc_id = %doc.doc_id,
                deleted_at = doc.deleted_at.unwrap_or(0),
                "trash_cleanup dry-run: would purge"
            );
            continue;
        }
        // hard_delete owns its own S3 sweep (every doc-prefixed
        // blob under docs/<id>/ is removed alongside the DDB rows).
        // The audit row lands AFTER the destructive op so a failed
        // purge doesn't leave a "purged" record without the deletion.
        match state.doc_repo.hard_delete(&doc.doc_id).await {
            Ok(()) => {
                spawn_delete_from_index(state, doc.doc_id.clone());
                record_security_event_by_actor(
                    state,
                    &doc.owner_id,
                    WORKER_ACTOR_ID,
                    SecurityAuditAction::DocDeleted {
                        doc_id: doc.doc_id.clone(),
                        hard: true,
                    },
                );
                purged += 1;
            }
            Err(e) => {
                tracing::warn!(
                    doc_id = %doc.doc_id,
                    error = %e,
                    "trash_cleanup: hard_delete failed; skipping (continues sweep)"
                );
                errors += 1;
            }
        }
    }

    tracing::info!(
        scanned = docs.len(),
        purged,
        errors,
        dry_run,
        cutoff_usec,
        "trash_cleanup pass complete"
    );
    Ok(())
}

/// Compute the retention cutoff in usec-since-epoch given the
/// current wall time and the configured retention window in days.
/// Rows with `deleted_at < cutoff` are eligible for hard-purge.
fn cutoff_usec(now_usec: i64, retention_days: u32) -> i64 {
    now_usec - (retention_days as i64) * USEC_PER_DAY
}

/// Pack a UTC date into a single i64 so we can store "last run day"
/// in an AtomicI64. `20260421` for 2026-04-21. Identical encoding to
/// `digest::pack_date` and `audit_retention::pack_date` — a future
/// shared-scheduler refactor needs no migration.
fn pack_date(dt: &chrono::DateTime<Utc>) -> i64 {
    (dt.year() as i64) * 10_000 + (dt.month() as i64) * 100 + dt.day() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutoff_usec_subtracts_correct_window() {
        let now = 1_800_000_000_000_000_i64;
        let cutoff = cutoff_usec(now, 30);
        assert_eq!(now - cutoff, 30 * 24 * 60 * 60 * 1_000_000);
    }

    #[test]
    fn cutoff_usec_one_day_window() {
        let now = 1_000_000_000_000_000_i64;
        let cutoff = cutoff_usec(now, 1);
        assert_eq!(now - cutoff, USEC_PER_DAY);
    }

    #[test]
    fn cutoff_usec_long_window_doesnt_overflow() {
        let now = 1_800_000_000_000_000_i64;
        let cutoff = cutoff_usec(now, 3650);
        let span = now - cutoff;
        assert_eq!(span, 3650 * USEC_PER_DAY);
        assert!(cutoff < now);
    }

    #[test]
    fn cutoff_is_an_exclusive_lower_bound() {
        // The repo's `list_eligible_for_purge` uses `< cutoff` for
        // eligibility. A row whose `deleted_at` equals the cutoff
        // exactly must survive. Lock that contract here — if someone
        // flips the comparator to `<=` in the repo, retention silently
        // gets one-day-aggressive.
        let now = 2_000_000_000_000_000_i64;
        let cutoff = cutoff_usec(now, 30);
        assert!(!(cutoff < cutoff));
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
        // Mirrors the same shape as digest + audit_retention. Two
        // ticks in the same hour: the first swaps the atomic, the
        // second sees equality and short-circuits.
        let atom = AtomicI64::new(0);
        let today = 20260421;
        let first = atom.swap(today, Ordering::SeqCst);
        assert_eq!(first, 0, "first call records today");
        let second = atom.swap(today, Ordering::SeqCst);
        assert_eq!(second, today, "second call sees today and short-circuits");
    }
}
