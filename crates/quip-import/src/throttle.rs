//! Rate throttle for the Quip API client.
//!
//! Quip allows 50 requests/minute; exceeding it returns HTTP 503. This
//! module has two layers:
//!
//! - A pure token-bucket calculator ([`RateState`] + [`plan_delay`]) and a
//!   pure exponential-backoff calculator ([`backoff_ms`]) — both plain
//!   functions over plain data, unit-testable without a clock or an
//!   executor.
//! - [`Throttle`], the async wrapper `QuipClient` actually holds: it owns
//!   mutable [`RateState`] behind a short synchronous lock. Admission
//!   rechecks the bucket and current response headers after every sleep.
use std::sync::Mutex;

use tokio::sync::Notify;
use tokio::time::Instant;

use ogrenotes_common::time::now_usec;

/// The safety-margined rate limit (requests/minute) the default
/// [`Throttle::new`] bakes in — 10% under Quip's documented 50/min hard
/// limit, per the design.
pub const DEFAULT_RATE_PER_MIN: u32 = 45;

/// Wall-clock milliseconds since the Unix epoch, per the task contract
/// (`now_usec() / 1000`).
fn now_ms_wall() -> i64 {
    now_usec() / 1000
}

/// Token-bucket state for one client's requests. Plain data; mutated only
/// by [`plan_delay`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateState {
    /// Tokens currently banked (fractional — refills continuously, not in
    /// whole-token steps). Negative values represent reserved future tokens.
    pub tokens: f64,
    /// Wall-clock ms at which `tokens` was last refilled.
    pub last_refill_ms: i64,
    /// Server-reported reset time (ms since epoch) from the last
    /// `x-ratelimit-reset` header seen, if any.
    pub reset_at_ms: Option<i64>,
    /// Server-reported remaining-request count from the last
    /// `x-ratelimit-remaining` header seen, if any.
    pub remaining_hint: Option<u32>,
}

impl RateState {
    /// A freshly-full bucket for `rate_per_min`, as of `now_ms`.
    pub fn full(rate_per_min: u32, now_ms: i64) -> Self {
        Self {
            tokens: rate_per_min as f64,
            last_refill_ms: now_ms,
            reset_at_ms: None,
            remaining_hint: None,
        }
    }
}

/// Plan the delay (in ms) before sending one request, given bucket `state`
/// at `now_ms` and target `rate_per_min`. Pure: refills `state` for the
/// elapsed time, reserves one token (or computes how long until one is
/// available), and returns how long the caller should sleep before
/// actually sending (`0` = send now).
///
/// If the last-observed server headers said `remaining_hint == 0`, the
/// server's count is treated as authoritative over the local bucket and the
/// wait is floored at `reset_at_ms - now_ms` — the local bucket may think a
/// token is free, but the server has already said no. Once `now_ms` reaches
/// `reset_at_ms`, that hint is considered stale (fresh headers should have
/// superseded it by then) and is cleared so the local bucket resumes normal
/// accounting rather than returning `0` unconditionally forever.
pub fn plan_delay(state: &mut RateState, now_ms: i64, rate_per_min: u32) -> u64 {
    let capacity = rate_per_min.max(1) as f64;
    if state.remaining_hint == Some(0) && state.reset_at_ms.is_some_and(|reset| now_ms >= reset) {
        state.remaining_hint = None;
        state.reset_at_ms = None;
    }

    // Reserve at the server reset when it forbids earlier requests. Refilling
    // at that instant before spending tokens also bounds the post-reset burst.
    let reset_floor = if state.remaining_hint == Some(0) {
        state.reset_at_ms.unwrap_or(now_ms)
    } else {
        now_ms
    };
    // A backward wall-clock jump must not refill the same interval twice.
    let reservation_ms = now_ms.max(state.last_refill_ms).max(reset_floor);
    let elapsed_ms = reservation_ms.saturating_sub(state.last_refill_ms) as f64;
    state.tokens = (state.tokens + elapsed_ms * capacity / 60_000.0).min(capacity);
    state.last_refill_ms = reservation_ms;
    state.tokens -= 1.0;

    // Keep debt rather than clamping to zero: every caller owns a distinct
    // future token even when all reservations happen at the same instant.
    let refill_delay = ((-state.tokens).max(0.0) * 60_000.0 / capacity).ceil() as u64;
    let floor_delay = reservation_ms.saturating_sub(now_ms) as u64;
    floor_delay.saturating_add(refill_delay)
}

/// Exponential backoff with full jitter (base 1000ms, cap 60000ms) for a
/// 503 retry. `attempt` is the 0-indexed retry count; `rng01` must be a
/// caller-supplied value in `[0, 1]`. If the server gave `reset_at_ms`, the
/// result is floored at the time remaining until reset — retrying before
/// the server's own window resets is pointless.
pub fn backoff_ms(attempt: u32, reset_at_ms: Option<i64>, now_ms: i64, rng01: f64) -> u64 {
    const BASE_MS: f64 = 1_000.0;
    const CAP_MS: f64 = 60_000.0;

    let exp = BASE_MS * 2f64.powi(attempt as i32);
    let capped = exp.min(CAP_MS);
    let jittered = (rng01.clamp(0.0, 1.0) * capped) as u64;

    let floor = reset_at_ms
        .map(|reset_at| (reset_at - now_ms).max(0) as u64)
        .unwrap_or(0);

    jittered.max(floor)
}

/// Async gate `QuipClient` acquires before every Quip API call. Bucket
/// decisions and header updates share a short synchronous lock, never held
/// across a sleep. Pending callers recheck admission rather than spending a
/// token until they can send, so cancellation does not strand reservations.
pub struct Throttle {
    state: Mutex<RateState>,
    rate_per_min: u32,
    started_ms: i64,
    started_at: Instant,
    headers_changed: Notify,
}

impl Throttle {
    /// A throttle at Quip's documented rate ([`DEFAULT_RATE_PER_MIN`]),
    /// clocked off the wall clock.
    pub fn new() -> Self {
        Self::with_rate(DEFAULT_RATE_PER_MIN)
    }

    /// A throttle at an explicit rate. Mainly for tests, so they don't have
    /// to wait out a real 50/min window.
    pub fn with_rate(rate_per_min: u32) -> Self {
        let now = now_ms_wall();
        Self {
            state: Mutex::new(RateState::full(rate_per_min, now)),
            rate_per_min,
            started_ms: now,
            started_at: Instant::now(),
            headers_changed: Notify::new(),
        }
    }

    // Anchor the epoch used by reset headers to monotonic elapsed time. Wall
    // clock corrections cannot grant extra tokens or make a sleep repeat.
    fn now_ms(&self) -> i64 {
        let elapsed = self.started_at.elapsed().as_millis().min(i64::MAX as u128) as i64;
        self.started_ms.saturating_add(elapsed)
    }

    /// Block until a token is available per the bucket / header hints, then
    /// return. Call this immediately before sending a Quip API request.
    pub async fn acquire(&self) {
        loop {
            // Register before inspecting state so a header update between the
            // decision and the sleep cannot be missed.
            let changed = self.headers_changed.notified();
            let delay_ms = {
                let mut state = self.state.lock().expect("throttle state lock poisoned");
                let mut candidate = *state;
                let delay = plan_delay(&mut candidate, self.now_ms(), self.rate_per_min);
                if delay == 0 {
                    *state = candidate;
                    return;
                }
                delay
            };
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                _ = changed => {}
            }
        }
    }

    /// Feed response rate-limit headers back into the bucket. Header updates
    /// cannot be dropped under contention and wake pending callers to recheck
    /// both extended and shortened reset windows.
    pub fn observe_headers(&self, remaining: Option<u32>, reset_at_ms: Option<i64>) {
        {
            let mut state = self.state.lock().expect("throttle state lock poisoned");
            if remaining.is_some() {
                state.remaining_hint = remaining;
            }
            if reset_at_ms.is_some() {
                state.reset_at_ms = reset_at_ms;
            }
        }
        if remaining.is_some() || reset_at_ms.is_some() {
            self.headers_changed.notify_waiters();
        }
    }

    /// Sleep out an exponential-backoff delay for retry `attempt` after a
    /// 503, honoring the last-observed `reset_at_ms` as a floor. Jitter is
    /// derived from the current wall-clock millisecond, not a real RNG —
    /// see the module's crate-level notes; this avoids pulling in an rng
    /// dependency for one call site.
    pub async fn backoff(&self, attempt: u32) {
        let now = self.now_ms();
        let reset_at_ms = {
            self.state
                .lock()
                .expect("throttle state lock poisoned")
                .reset_at_ms
        };
        let rng01 = (now.rem_euclid(1000)) as f64 / 1000.0;
        let delay_ms = backoff_ms(attempt, reset_at_ms, now, rng01);
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_gates_to_the_rate_over_a_minute() {
        // 45/min: the 46th request within the same minute must be delayed.
        let mut s = RateState::full(45, 0);
        for _ in 0..45 {
            assert_eq!(plan_delay(&mut s, 0, 45), 0);
        }
        let d = plan_delay(&mut s, 0, 45);
        assert!(d > 0 && d <= 60_000, "46th within the minute delays, got {d}");
    }

    #[test]
    fn new_throttle_defaults_to_45_per_min_not_50() {
        // Regression for the 50-vs-45 bug: `Throttle::new()` must bake in
        // the design's 45/min safety margin, not Quip's raw 50/min limit.
        // Drain 45 tokens at t=0 with zero delay, then the 46th must be
        // delayed — if the default were still 50, this would fail (the
        // 46th would sail through with delay 0).
        let mut s = RateState::full(DEFAULT_RATE_PER_MIN, 0);
        for _ in 0..45 {
            assert_eq!(
                plan_delay(&mut s, 0, DEFAULT_RATE_PER_MIN),
                0,
                "first 45 requests at t=0 should never be delayed"
            );
        }
        let d = plan_delay(&mut s, 0, DEFAULT_RATE_PER_MIN);
        assert!(
            d > 0,
            "46th request at t=0 must be delayed under the 45/min default, got {d}"
        );
    }

    #[test]
    fn remaining_hint_zero_waits_for_reset() {
        let mut s = RateState::full(45, 0);
        s.remaining_hint = Some(0);
        s.reset_at_ms = Some(5_000);
        assert_eq!(plan_delay(&mut s, 1_000, 45), 4_000);
    }

    #[test]
    fn backoff_is_bounded_jittered_and_reset_floored() {
        assert!(backoff_ms(0, None, 0, 0.0) <= 1_000);
        assert!(backoff_ms(10, None, 0, 1.0) <= 60_000); // capped
        assert!(backoff_ms(0, Some(30_000), 0, 0.0) >= 30_000); // reset floor
    }

    #[test]
    fn refill_after_elapsed_time_restores_a_token() {
        // 60/min == exactly 1 token/sec — easy to reason about.
        let mut s = RateState::full(60, 0);
        for _ in 0..60 {
            assert_eq!(plan_delay(&mut s, 0, 60), 0);
        }
        // Bucket is now empty. One second later exactly one token should
        // have accrued, so the next request should NOT be delayed.
        assert_eq!(plan_delay(&mut s, 1_000, 60), 0);
        // But the one right after that (same instant, second token not yet
        // due) should be.
        let d = plan_delay(&mut s, 1_000, 60);
        assert!(d > 0, "second request at the same instant should delay, got {d}");
    }

    #[test]
    fn remaining_hint_zero_without_reset_falls_back_to_bucket() {
        // If the server says "zero remaining" but never told us a reset
        // time, we can't compute a floor — fall back to the local bucket
        // instead of hanging forever.
        let mut s = RateState::full(45, 0);
        s.remaining_hint = Some(0);
        assert_eq!(plan_delay(&mut s, 0, 45), 0); // bucket still has tokens
    }

    #[test]
    fn stale_remaining_hint_is_cleared_and_bucket_reenforced_after_reset() {
        // Regression: once remaining_hint == Some(0), plan_delay must not
        // return 0 forever once now_ms passes reset_at_ms — it must clear
        // the stale hint and fall through to real token-bucket accounting.
        let mut s = RateState::full(45, 0);
        s.remaining_hint = Some(0);
        s.reset_at_ms = Some(1_000);

        // Before reset: waits for the remaining reset window.
        assert!(plan_delay(&mut s, 500, 45) > 0);

        // Reset has passed: hint must be cleared and a token spent from the
        // (still-full-ish) bucket rather than bypassing it with delay 0
        // forever.
        let _ = plan_delay(&mut s, 2_000, 45);
        assert_eq!(s.remaining_hint, None, "stale hint must be cleared once reset passes");
        assert_eq!(s.reset_at_ms, None, "stale reset_at_ms must be cleared once reset passes");

        // Drain the rest of the bucket (44 more tokens, 45 already spent
        // above) and confirm the bucket is actually enforced again — not
        // perpetually returning 0.
        for _ in 0..44 {
            plan_delay(&mut s, 2_000, 45);
        }
        assert!(
            plan_delay(&mut s, 2_000, 45) > 0,
            "bucket must be enforced again after the stale hint is cleared"
        );
    }

    #[test]
    fn backoff_grows_with_attempt_before_the_cap() {
        // Same rng01, increasing attempt: pre-cap, later attempts should
        // never plan a *shorter* wait than earlier ones.
        let a1 = backoff_ms(1, None, 0, 0.5);
        let a2 = backoff_ms(2, None, 0, 0.5);
        let a3 = backoff_ms(3, None, 0, 0.5);
        assert!(a1 <= a2 && a2 <= a3, "{a1} {a2} {a3}");
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_waits_when_headers_say_zero_remaining() {
        let t = Throttle::with_rate(45);
        // First acquire: bucket is full, returns immediately.
        t.acquire().await;

        // Server says "nothing left until 5s from now" — this must
        // override the (still mostly full) local bucket.
        let now = now_ms_wall();
        t.observe_headers(Some(0), Some(now + 5_000));

        let start = tokio::time::Instant::now();
        t.acquire().await;
        // Under a paused clock, `sleep` fast-forwards virtual time rather
        // than truly blocking, so this assertion is instant but still
        // proves the ~5s wait was actually scheduled.
        assert!(
            tokio::time::Instant::now().duration_since(start) >= std::time::Duration::from_millis(4_000),
            "acquire should have honored the server's reset hint"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_sleeps_out_the_planned_delay() {
        let t = Throttle::with_rate(45);
        let start = tokio::time::Instant::now();
        t.backoff(10).await; // attempt 10 => capped at 60s, jitter aside
        assert!(tokio::time::Instant::now().duration_since(start) <= std::time::Duration::from_millis(60_000));
    }
}
