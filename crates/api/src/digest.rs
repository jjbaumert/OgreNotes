// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Daily digest email scheduler (M4.1).
//!
//! Ticks every hour. On each tick, checks whether the current UTC hour
//! matches `EMAIL_DIGEST_HOUR_UTC` and — to survive process restarts
//! mid-window — whether we've already sent today's digest from this
//! process. If both gates pass, walks every user, filters to those who
//! haven't been active in the last 24 hours, and sends them a digest of
//! unread notifications from the same 24-hour window.
//!
//! Per-user timezone support is explicitly out of scope; everyone gets
//! the digest at the same UTC hour. When `User.timezone` lands in a
//! future milestone the decision function becomes per-user.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Timelike, Utc};
use ogrenotes_common::time::now_usec;

use crate::state::AppState;

const TICK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const INACTIVITY_WINDOW_USEC: i64 = 24 * 60 * 60 * 1_000_000;
const PER_USER_UNREAD_LIMIT: usize = 50;

/// Spawn the hourly digest task. Safe to call unconditionally — if
/// `email_digest_enabled` is false the tick short-circuits and does no
/// work. Returns the join handle so callers can keep it alive (typically
/// via a `let _ = spawn_scheduler(...)`).
pub fn spawn_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    // Encode "date we last ran" as a packed i32 (year*10000 + month*100 + day).
    // AtomicI64 carries it cleanly; 0 means "never ran in this process".
    let last_run = Arc::new(AtomicI64::new(0));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        // Skip the initial immediate tick — we don't want a just-booted
        // process to double-fire alongside an earlier instance that
        // already sent today's digest.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = run_tick(&state, &last_run).await {
                tracing::warn!(error = %e, "digest tick failed");
            }
        }
    })
}

/// One scheduler tick. Exposed for tests; decides whether to run, then
/// runs if allowed.
async fn run_tick(state: &AppState, last_run_date: &AtomicI64) -> Result<(), String> {
    if !state.config.email_digest_enabled {
        return Ok(());
    }
    let now = Utc::now();
    if now.hour() as u8 != state.config.email_digest_hour_utc {
        return Ok(());
    }
    let today = pack_date(&now);
    // Swap-on-equal pattern: if another thread raced and already set
    // today's date, we skip. Single-process, single-task today but the
    // guard is cheap.
    if last_run_date.swap(today, Ordering::SeqCst) == today {
        return Ok(());
    }
    send_digests(state).await
}

/// Walk users and send digests. Assumes the caller has already decided
/// it is digest time.
pub(crate) async fn send_digests(state: &AppState) -> Result<(), String> {
    let since = now_usec() - INACTIVITY_WINDOW_USEC;
    let mut cursor: Option<String> = None;
    let mut sent = 0usize;
    let mut scanned = 0usize;

    loop {
        let (users, next) = state
            .user_repo
            .list_all(100, cursor.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        for user in users {
            scanned += 1;
            // Never-active users (last_active_at == 0) are treated as
            // inactive — the digest is an onboarding nudge for them too.
            let is_inactive = user.last_active_at == 0 || user.last_active_at <= since;
            if !is_inactive {
                continue;
            }
            if user.is_disabled {
                continue;
            }
            let unread = match state
                .notification_repo
                .list_unread_since(&user.user_id, since, PER_USER_UNREAD_LIMIT)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(user_id = %user.user_id, error = %e, "digest unread fetch failed");
                    continue;
                }
            };
            if unread.is_empty() {
                continue;
            }
            match state
                .email_service
                .try_send_digest(&user.user_id, &unread)
                .await
            {
                Ok(ogrenotes_notify::SendOutcome::Sent) => sent += 1,
                Ok(outcome) => {
                    tracing::debug!(user_id = %user.user_id, ?outcome, "digest skipped");
                }
                Err(e) => {
                    tracing::warn!(user_id = %user.user_id, error = %e, "digest send failed");
                }
            }
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    tracing::info!(scanned, sent, "digest pass complete");
    Ok(())
}

/// Pack a UTC date into a single i32 so we can store "last run day" in
/// an AtomicI64. `20260421` for 2026-04-21.
fn pack_date(dt: &chrono::DateTime<Utc>) -> i64 {
    (dt.year() as i64) * 10_000 + (dt.month() as i64) * 100 + dt.day() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Simulate two ticks in the same hour: the first swaps the
        // atomic, the second sees equality and should skip.
        let atom = AtomicI64::new(0);
        let today = 20260421;
        let first = atom.swap(today, Ordering::SeqCst);
        assert_eq!(first, 0, "first call records today");
        let second = atom.swap(today, Ordering::SeqCst);
        assert_eq!(second, today, "second call sees today and short-circuits");
    }
}
