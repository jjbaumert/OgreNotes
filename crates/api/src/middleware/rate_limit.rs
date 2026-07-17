// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Reusable Redis-backed fixed-window rate limiter.
//!
//! Closes the per-route limiting half of #36. Used by:
//!   - `routes/auth.rs::{login_github, login_provider, refresh}` (per-IP)
//!   - `routes/search.rs::search_handler`                          (per-user)
//!   - `routes/sharing.rs::{add_member, remove_member, …}`         (per-user)
//!
//! Pattern: increment a bucket key in Redis (`ratelimit:<scope>:<id>:<bucket>`),
//! set EXPIRE on the first hit of a bucket, decide based on the
//! returned count vs. the limit. Deny → 429 + `Retry-After`. Redis
//! errors fail-open with a `tracing::warn!` so a Redis blip can't
//! black-hole every authenticated route — same posture as the
//! `/ask` quota module.
//!
//! Multi-instance (#51): the counter lives in the shared Redis the whole
//! ECS service points at, and `INCR` is atomic, so a per-id limit is
//! enforced *across* all tasks — N instances share one budget, not N
//! budgets. Scaling out does not loosen any limit. The only caveat is
//! the fixed-window edge burst (up to 2× at a window boundary), inherent
//! to fixed-window counting and independent of instance count; the
//! `/ask` quota module shares this Redis-counter design and property.

use std::sync::Arc;

use fred::error::RedisError;
use fred::prelude::KeysInterface;
use fred::prelude::RedisClient;
use ogrenotes_common::metrics::{counter, MetricKey};

use crate::error::ApiError;

/// Outcome of a rate-limit check.
pub enum Decision {
    Allow,
    Deny {
        /// Seconds until the current bucket rolls over and the
        /// caller's count resets to zero.
        retry_after_secs: u64,
    },
}

/// Pure check — increments the counter and returns the decision.
/// Caller decides what to do on `Err` (fail-open in this codebase).
async fn check(
    redis: &Arc<RedisClient>,
    scope: &str,
    identifier: &str,
    limit: u64,
    window_secs: u64,
) -> Result<Decision, RedisError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let bucket = now / window_secs;
    // Clamp so a request at the exact bucket boundary doesn't quote a
    // full-window Retry-After (the next INCR would land in a fresh
    // bucket; the cap immediately resets).
    let secs_until_next = (window_secs - (now % window_secs)).clamp(1, window_secs - 1);
    let key = format!("ratelimit:{scope}:{identifier}:{bucket}");

    // Fixed-window counter: INCR then unconditionally EXPIRE. The
    // EXPIRE is idempotent — setting it on every call always points
    // at the same wall-clock moment (just past the bucket boundary),
    // so re-issuing it doesn't extend the window. We do it on every
    // request rather than only on the first because of a race: two
    // concurrent first-hits both see count > 1 from each other's
    // INCR-before-EXPIRE, and if EXPIRE was conditional on `count ==
    // 1` neither would set it. The orphaned key would live forever
    // and the bucket would never reset for that user/IP — a slow
    // memory leak and a permanent rate-limit on whoever raced.
    //
    // A request that is denied still INCRs (we check after the bump),
    // so a borderline user gets at most 2× the limit in the boundary
    // case — acceptable and matches the `/ask` module's behavior.
    let count: u64 = redis.incr(&key).await?;
    // +5s slack so the key doesn't expire microseconds before the
    // boundary in the rare case of a clock-jumpy host.
    let _: () = redis.expire(&key, (secs_until_next + 5) as i64).await?;
    if count > limit {
        Ok(Decision::Deny {
            retry_after_secs: secs_until_next,
        })
    } else {
        Ok(Decision::Allow)
    }
}

/// Enforce a rate limit. On Redis error, log and allow (fail-open).
/// Returns `Err(ApiError::TooManyRequests)` on cap exceeded so callers
/// just propagate with `?`.
pub async fn enforce(
    redis: &Arc<RedisClient>,
    scope: &str,
    identifier: &str,
    limit: u64,
    window_secs: u64,
) -> Result<(), ApiError> {
    match check(redis, scope, identifier, limit, window_secs).await {
        Ok(Decision::Allow) => {
            counter::inc(MetricKey::new(
                "ratelimit.allow_total",
                &[("scope", scope_label(scope))],
            ));
            Ok(())
        }
        Ok(Decision::Deny { retry_after_secs }) => {
            counter::inc(MetricKey::new(
                "ratelimit.deny_total",
                &[("scope", scope_label(scope))],
            ));
            tracing::warn!(
                scope = %scope,
                identifier = %identifier,
                retry_after_secs,
                "rate limit exceeded"
            );
            Err(ApiError::TooManyRequests {
                message: format!(
                    "Rate limit exceeded for {scope}; retry after {retry_after_secs}s."
                ),
                retry_after_secs,
            })
        }
        Err(e) => {
            // Fail-open. Redis is best-effort for limiting — black-
            // holing every authenticated route during a Redis blip
            // would be worse than a brief gap in cap enforcement.
            tracing::warn!(
                scope = %scope,
                error = %e,
                "rate-limit redis error; failing open"
            );
            counter::inc(MetricKey::new(
                "ratelimit.fail_open_total",
                &[("scope", scope_label(scope))],
            ));
            Ok(())
        }
    }
}

/// Map the dynamic scope string to a stable static label for metrics
/// cardinality. Anything not in the allowlist falls through to
/// `"other"` so metrics don't grow unboundedly if a future caller
/// passes a doc_id-shaped scope by mistake.
fn scope_label(scope: &str) -> &'static str {
    match scope {
        "auth_login" => "auth_login",
        "auth_refresh" => "auth_refresh",
        "search" => "search",
        "sharing" => "sharing",
        "admin_mut" => "admin_mut",
        "scim_request" => "scim_request",
        "mfa_verify" => "mfa_verify",
        "comments" => "comments",
        "content_write" => "content_write",
        "import" => "import",
        "bulk_op" => "bulk_op",
        "bulk_export" => "bulk_export",
        "ws_upgrade" => "ws_upgrade",
        "dev_login" => "dev_login",
        "client_telemetry" => "client_telemetry",
        "rum" => "rum",
        "saml_acs" => "saml_acs",
        _ => "other",
    }
}

/// Extract a stable IP-shaped identifier from request headers for
/// per-IP rate limits. Mirrors the dev-login limiter — takes the
/// first hop of `X-Forwarded-For`, falls back to `"unknown"` when
/// the header is absent (single global bucket).
///
/// Trust caveat (carried over from dev-login, also tracked in #17 via
/// the LOW gap registry): `X-Forwarded-For` is client-spoofable
/// unless the ALB strips/normalizes it. For internet-facing prod the
/// ALB's `X-Forwarded-For` *appends* the client IP; we take the
/// first hop, which is the original client. This is safe behind the
/// stack's ALB but loose without one.
pub fn ip_identifier(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
