// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Per-user and global quota coverage for `/api/v1/ask`.
//!
//! Pins:
//! - Per-user hourly cap (USER_HOURLY_CAP=30) trips on the 31st call
//!   and returns 429 with a `Retry-After` header bounded by the hour
//!   window.
//! - Global circuit breaker returns 503 with `Retry-After` once the
//!   counter pre-crosses GLOBAL_DAILY_CAP=5000.
//!
//! Both tests use a `ConstantClaude` stub that returns a single text
//! block immediately — no tool calls — so each `/api/v1/ask` request
//! completes in milliseconds. Per-user hourly is `nanoid`-emailed so
//! the user keys never collide across tests.
//!
//! Redis state caveat: the `ratelimit:ask:global:day:N` counter is
//! shared across all concurrent tests touching this endpoint. Each
//! test below DELs the global key at start so the in-process state
//! is deterministic; concurrent integration tests of unrelated
//! endpoints don't touch this key. If we ever add many concurrent
//! callers of `/api/v1/ask` in other tests, the global-cap test
//! becomes vulnerable to false-positive throttling and would need
//! a per-test prefix knob.

mod common;

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use axum::body::Body;
use fred::prelude::KeysInterface;
use http_body_util::BodyExt;
use hyper::{Method, Request};
use tower::ServiceExt;

use ogrenotes_api::claude::{
    ClaudeError, ClaudeMessages, Message, MessagesResponse, ResponseBlock, Tool,
};

/// A ClaudeMessages stub that returns the same text block on every
/// call — sufficient to drive the agent loop's terminal-text path
/// (no tool_use, exits in round 0).
struct ConstantClaude {
    text: String,
}

#[async_trait]
impl ClaudeMessages for ConstantClaude {
    async fn messages(
        &self,
        _system: &str,
        _messages: &[Message],
        _tools: &[Tool],
        _max_tokens: u32,
    ) -> Result<MessagesResponse, ClaudeError> {
        Ok(MessagesResponse {
            content: vec![ResponseBlock::Text {
                text: self.text.clone(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: None,
        })
    }
}

/// Serializes the `#[tokio::test]`s in this file that touch the shared
/// global daily quota key (`ratelimit:ask:global:day:N`). libtest runs
/// test fns in a binary concurrently; `test_ask_quota_per_user_and_global`
/// Phase B briefly bumps that key past `GLOBAL_DAILY_CAP`, and a
/// concurrent `test_ask_byok_bypasses_per_user_cap` warm-up call landing
/// in that window gets a 503 instead of 200 (flaky — observed red in CI
/// run 28236682889, green the run before). Holding one lock for each
/// test's duration removes the overlap. The combined-test comment below
/// explains why those two checks were already merged; this lock extends
/// the same serialization to the BYOK test, the third `/ask` caller.
fn quota_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Send POST /api/v1/ask and return (status, retry_after_header).
async fn ask_status(
    app: &common::TestApp,
    token: &str,
) -> (u16, Option<String>) {
    let body = serde_json::json!({ "question": "anything" });
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ask")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    // Drain body so the spawned agent task gets a chance to schedule.
    let _ = resp.into_body().collect().await;
    (status, retry_after)
}

/// Like `ask_status` but forwards a browser-supplied (BYOK) Anthropic key
/// in the `x-anthropic-key` header. Returns just the HTTP status — which the
/// handler decides synchronously (the quota gate runs before the agent loop
/// is spawned), so this is deterministic even though the BYOK path would use
/// a real client for the fire-and-forget agent loop.
async fn ask_status_byok(app: &common::TestApp, token: &str, byok_key: &str) -> u16 {
    let body = serde_json::json!({ "question": "anything" });
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ask")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .header("x-anthropic-key", byok_key)
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    resp.status().as_u16()
}

/// #29: a BYOK request bypasses the operator's per-user cap. After a user
/// has exhausted the hourly cap (server-key path → 429), the same user
/// presenting their own key is admitted (200), because the operator's quota
/// doesn't apply when the user is paying.
#[tokio::test]
async fn test_ask_byok_bypasses_per_user_cap() {
    common::require_infra!();
    // Serialize against the global-cap test, which briefly poisons the
    // shared global daily counter (see quota_test_lock).
    let _quota_guard = quota_test_lock().lock().await;
    let stub: Arc<dyn ClaudeMessages> = Arc::new(ConstantClaude {
        text: "ok".to_string(),
    });
    let app = common::TestApp::new_with_claude(Some(stub)).await;

    let email = format!("byokquota-{}@test.com", nanoid::nanoid!(8));
    let token = app.create_user_token(&email).await;

    // Exhaust the per-user hourly cap on the operator-key path.
    for _ in 0..30u64 {
        let (status, _) = ask_status(&app, &token).await;
        assert_eq!(status, 200);
    }
    let (status, _) = ask_status(&app, &token).await;
    assert_eq!(status, 429, "cap must be exhausted on the operator-key path");

    // Same user, now presenting their own key → admitted despite the cap.
    let status = ask_status_byok(&app, &token, "sk-ant-byok-test-key").await;
    assert_eq!(
        status, 200,
        "a BYOK request must bypass the exhausted per-user cap, got {status}",
    );
}

/// Combined per-user + global quota coverage in a single test.
///
/// Why one test instead of two: the global daily counter is, by
/// design, a single Redis key shared across every caller of
/// /api/v1/ask. Splitting the per-user and global checks across two
/// `#[tokio::test]` functions makes them race against each other on
/// that key (cargo test runs `#[test]` functions concurrently by
/// default). Combining them serializes the touches.
///
/// Phase A — per-user hourly cap:
///   30 sequential calls succeed; the 31st returns 429 with a
///   Retry-After header bounded by the hour window.
///
/// Phase B — global daily circuit breaker:
///   Pre-set the global counter to GLOBAL_DAILY_CAP via INCRBY
///   (deterministic regardless of what's already accumulated). The
///   next call returns 503 with Retry-After bounded by the day window.
#[tokio::test]
async fn test_ask_quota_per_user_and_global() {
    common::require_infra!();
    // Held for the whole test: Phase B poisons the shared global counter,
    // so no other /ask test may run concurrently (see quota_test_lock).
    let _quota_guard = quota_test_lock().lock().await;
    let stub: Arc<dyn ClaudeMessages> = Arc::new(ConstantClaude {
        text: "ok".to_string(),
    });
    let app = common::TestApp::new_with_claude(Some(stub)).await;

    // ── Phase A: per-user hourly cap ─────────────────────────────
    // Use a nanoid email so the user keys don't collide with prior
    // cargo test runs (Redis keys persist until the hour rolls).
    let email = format!("quota-{}@test.com", nanoid::nanoid!(8));
    let token = app.create_user_token(&email).await;

    for i in 1..=30u64 {
        let (status, _) = ask_status(&app, &token).await;
        assert_eq!(status, 200, "call #{i} should be 200, got {status}");
    }
    let (status, retry_after) = ask_status(&app, &token).await;
    assert_eq!(status, 429, "31st call should be 429, got {status}");
    let retry_after = retry_after.expect("Retry-After header missing on 429");
    let secs: u64 = retry_after
        .parse()
        .expect("Retry-After must be integer seconds");
    assert!(
        (1..=3660).contains(&secs),
        "Retry-After must be within the hour window (got {secs}s)",
    );

    // ── Phase B: global circuit breaker ──────────────────────────
    // Bump the global counter past the cap deterministically. INCRBY
    // adds to whatever's already there (from this run's 31 calls plus
    // any concurrent traffic), so we land safely above 5000 even if
    // the key was non-zero. Use a fresh user so the per-user cap from
    // Phase A doesn't shadow the global trip.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let global_key = format!("ratelimit:ask:global:day:{}", now / 86400);
    let _: u64 = app
        .state
        .redis
        .incr_by(&global_key, 5000_i64)
        .await
        .expect("incr_by global counter");

    let email_b = format!("globalquota-{}@test.com", nanoid::nanoid!(8));
    let token_b = app.create_user_token(&email_b).await;

    let (status, retry_after) = ask_status(&app, &token_b).await;

    // Cleanup BEFORE asserting so a failing assertion doesn't leave
    // the global counter inflated for the rest of the day TTL — the
    // key is shared across test runs against the same Redis. INCRBY
    // -5000 puts the counter back to whatever it was before Phase B
    // bumped it; if the request itself somehow already drove it
    // higher, that excess remains. (Earlier revisions did this AFTER
    // the asserts, so a panic mid-phase poisoned subsequent runs for
    // 24h. See review note on commit fd93691.)
    let _: u64 = app
        .state
        .redis
        .incr_by(&global_key, -5000_i64)
        .await
        .expect("incr_by reset");

    assert_eq!(
        status, 503,
        "global cap exceeded should be 503, got {status}"
    );
    let retry_after = retry_after.expect("Retry-After header missing on 503");
    let secs: u64 = retry_after
        .parse()
        .expect("Retry-After must be integer seconds");
    assert!(
        (1..=86460).contains(&secs),
        "Retry-After must be within the day window (got {secs}s)",
    );

    app.cleanup().await;
}
