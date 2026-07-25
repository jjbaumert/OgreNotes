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
//!   the mutable [`RateState`] behind a `tokio::sync::Mutex` and turns the
//!   pure `plan_delay`/`backoff_ms` results into real `tokio::time::sleep`s.
use tokio::sync::Mutex;

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
    /// whole-token steps).
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
    let capacity = (rate_per_min.max(1)) as f64;

    // Refill continuously since the last reservation.
    let elapsed_ms = (now_ms - state.last_refill_ms).max(0) as f64;
    if elapsed_ms > 0.0 {
        let refilled = elapsed_ms * capacity / 60_000.0;
        state.tokens = (state.tokens + refilled).min(capacity);
    }
    state.last_refill_ms = now_ms;

    if state.remaining_hint == Some(0) && let Some(reset_at) = state.reset_at_ms {
        if now_ms < reset_at {
            return (reset_at - now_ms).max(0) as u64;
        }
        // The server's reset window has passed without fresh headers
        // arriving to supersede it. Treating the stale hint as
        // authoritative forever would permanently bypass the local bucket
        // (return 0 without ever spending a token). Clear it and fall
        // through to normal token-bucket accounting.
        state.remaining_hint = None;
        state.reset_at_ms = None;
    }

    if state.tokens >= 1.0 {
        state.tokens -= 1.0;
        return 0;
    }

    // Not enough banked — reserve the token anyway (bucket bottoms out at
    // 0) and report how long until it would naturally have accrued.
    let deficit = 1.0 - state.tokens;
    let ms_per_token = 60_000.0 / capacity;
    state.tokens = 0.0;
    (deficit * ms_per_token).ceil().max(1.0) as u64
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

/// Async gate `QuipClient` acquires before every Quip API call. Wraps a
/// [`RateState`] behind a `tokio::sync::Mutex` so concurrent callers
/// serialize on the same bucket; the lock is held only long enough to run
/// the pure [`plan_delay`]/[`backoff_ms`] calculators, never across an
/// `.await`.
pub struct Throttle {
    state: Mutex<RateState>,
    rate_per_min: u32,
    clock: fn() -> i64,
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
            clock: now_ms_wall,
        }
    }

    /// Block until a token is available per the bucket / header hints, then
    /// return. Call this immediately before sending a Quip API request.
    pub async fn acquire(&self) {
        let now = (self.clock)();
        let delay_ms = {
            let mut state = self.state.lock().await;
            plan_delay(&mut state, now, self.rate_per_min)
        };
        if delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
    }

    /// Feed the last response's `x-ratelimit-remaining` /
    /// `x-ratelimit-reset` headers back into the bucket. Synchronous by
    /// contract, so this uses `try_lock` rather than awaiting: the lock is
    /// only ever held briefly inside `plan_delay`/here, never across an
    /// `.await`, so contention is momentary at worst; on the rare miss we
    /// just drop the hint and let the next response's headers supersede it
    /// (nothing correctness-critical hinges on any single header read).
    pub fn observe_headers(&self, remaining: Option<u32>, reset_at_ms: Option<i64>) {
        match self.state.try_lock() {
            Ok(mut state) => {
                if remaining.is_some() {
                    state.remaining_hint = remaining;
                }
                if reset_at_ms.is_some() {
                    state.reset_at_ms = reset_at_ms;
                }
            }
            Err(_) => {
                tracing::debug!(
                    "throttle: skipped rate-limit header update, bucket was locked"
                );
            }
        }
    }

    /// Sleep out an exponential-backoff delay for retry `attempt` after a
    /// 503, honoring the last-observed `reset_at_ms` as a floor. Jitter is
    /// derived from the current wall-clock millisecond, not a real RNG —
    /// see the module's crate-level notes; this avoids pulling in an rng
    /// dependency for one call site.
    pub async fn backoff(&self, attempt: u32) {
        let now = (self.clock)();
        let reset_at_ms = { self.state.lock().await.reset_at_ms };
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
