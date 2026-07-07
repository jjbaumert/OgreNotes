// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! The rate limiter is deliberately FAIL-OPEN: if Redis is unreachable it
//! allows the request (and bumps `ratelimit.fail_open_total`) rather than
//! 429-ing every authenticated route during a Redis blip. A regression to
//! fail-*closed* would be a silent availability outage, so this pins the
//! posture directly against `enforce` with an unreachable Redis.
//!
//! No infra needed — the whole point is that Redis is down.

use std::sync::Arc;
use std::time::Duration;

use fred::prelude::*;

#[tokio::test]
async fn enforce_fails_open_when_redis_is_unreachable() {
    // A client pointed at a dead port. A short `default_command_timeout`
    // bounds the never-completing command so the test cannot hang; the
    // command then errors, which is exactly the failure `enforce` must
    // tolerate by allowing the request.
    let config = RedisConfig::from_url("redis://127.0.0.1:1").expect("config");
    let perf = fred::types::PerformanceConfig {
        default_command_timeout: Duration::from_millis(250),
        ..Default::default()
    };
    let client = RedisClient::new(config, Some(perf), None, None);
    let _ = client.connect();
    // Deliberately do NOT wait_for_connect — the server is unreachable.
    let redis = Arc::new(client);

    let result = ogrenotes_api::middleware::rate_limit::enforce(
        &redis,
        "test_fail_open",
        "ident-1",
        5,
        60,
    )
    .await;

    assert!(
        result.is_ok(),
        "the rate limiter must FAIL OPEN (allow) when Redis is unreachable, \
         not fail closed: {result:?}"
    );
}
