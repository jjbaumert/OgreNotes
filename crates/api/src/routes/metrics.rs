// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P9 piece C — frontend RUM ingest.
//!
//! The frontend sampler in `frontend/src/rum.rs` posts a JSON
//! payload of web-vitals for sampled sessions; this handler
//! validates, classifies, and forwards each provided vital into
//! the in-process metrics recorder. The EMF emitter (running on a
//! timer in `crates/common/src/metrics/emf.rs`) then folds the
//! histograms into the existing `OgreNotes` CloudWatch namespace.
//!
//! Auth: requires `AuthUser`. A future enhancement could accept
//! anonymous beacons on the login page; v1 ships with required
//! auth so the rate-limit gate has a stable per-user key and the
//! recorder isn't exposed to drive-by floods.
//!
//! Dimensions kept tight by design — `page` is a closed enum,
//! `user_agent_class` is a 3-bucket derivation. Per-doc-id /
//! per-route would blow up CloudWatch dimension cardinality.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use ogrenotes_common::metrics::{histogram, MetricKey};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/rum", post(ingest_rum))
}

/// Maximum legitimate value for any single vital (60 s). A page
/// that genuinely took longer than 60 s for an LCP is essentially
/// "never loaded"; a number above this cap is almost certainly a
/// clock-skew artifact and would otherwise yank histogram
/// percentiles toward fiction. Clamp + drop, don't reject — one
/// bad vital in a beacon shouldn't poison the rest.
const VITAL_MAX_MS: f64 = 60_000.0;

/// Maximum acceptable `cls` value. CLS is a unitless score that
/// realistically tops out around 1.0; anything beyond 10 is a
/// pathological page or an instrumentation bug.
const CLS_MAX: f64 = 10.0;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RumPayload {
    /// Closed-set page identifier. Unknown values are rejected.
    page: String,
    /// Web vitals; every field is optional so the sampler can
    /// emit each as it becomes available (LCP is decided on
    /// largest-contentful-paint; nav-timing is on `load`; etc.).
    vitals: RumVitals,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RumVitals {
    /// Largest Contentful Paint (ms).
    lcp: Option<f64>,
    /// First Contentful Paint (ms).
    fcp: Option<f64>,
    /// Interaction to Next Paint (ms). Supersedes FID.
    inp: Option<f64>,
    /// Cumulative Layout Shift (unitless score).
    cls: Option<f64>,
    /// Navigation timing — domContentLoadedEnd relative to start (ms).
    nav_dcl: Option<f64>,
    /// Navigation timing — loadEventEnd relative to start (ms).
    nav_load: Option<f64>,
}

/// Validate `page` against the bounded set. Anything outside the
/// set rejects — keeps the CloudWatch `page` dimension cardinality
/// at four.
fn validate_page(page: &str) -> Result<&'static str, ApiError> {
    match page {
        "home" => Ok("home"),
        "editor" => Ok("editor"),
        "spreadsheet" => Ok("spreadsheet"),
        "other" => Ok("other"),
        _ => Err(ApiError::BadRequest(format!("unknown page: {page}"))),
    }
}

/// Classify the User-Agent into desktop / mobile / tablet. Crude
/// substring match — every modern UA string surfaces at least one
/// of these tokens. The dimension only needs three buckets; a
/// fuller parser would be overkill for the signal we're after.
fn classify_user_agent(headers: &HeaderMap) -> &'static str {
    let ua = headers
        .get("user-agent")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if ua.contains("iPad") || ua.contains("Tablet") {
        "tablet"
    } else if ua.contains("Mobi") || ua.contains("Android") {
        "mobile"
    } else {
        "desktop"
    }
}

/// True when `value` is a sane finite quantity below `max`. NaN,
/// negative, and runaway values all get dropped silently.
fn finite_in_range(value: f64, max: f64) -> bool {
    value.is_finite() && value >= 0.0 && value <= max
}

/// Record one millisecond-valued histogram entry. The metric name
/// matches the `rum.*_ms` convention codified in
/// `design/performance-budgets.md`. CLS uses `record_cls` (unitless).
fn record_ms(name: &'static str, page: &str, ua_class: &'static str, value: f64) {
    histogram::record(
        MetricKey::new(
            name,
            &[("page", page), ("ua_class", ua_class)],
        ),
        value,
    );
}

async fn ingest_rum(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    headers: HeaderMap,
    Json(payload): Json<RumPayload>,
) -> Result<StatusCode, ApiError> {
    // Per-user rate limit — defends the recorder against a
    // compromised-token poisoning flood without blocking the
    // legitimate beacons-per-page cadence.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "rum",
        &user_id,
        state.config.rate_limit_rum_per_min,
        60,
    )
    .await?;

    let page = validate_page(&payload.page)?;
    let ua_class = classify_user_agent(&headers);

    // Every vital is optional and individually validated. One bad
    // field shouldn't drop the rest of the beacon — clamp / skip.
    if let Some(lcp) = payload.vitals.lcp {
        if finite_in_range(lcp, VITAL_MAX_MS) {
            record_ms("rum.lcp_ms", page, ua_class, lcp);
        }
    }
    if let Some(fcp) = payload.vitals.fcp {
        if finite_in_range(fcp, VITAL_MAX_MS) {
            record_ms("rum.fcp_ms", page, ua_class, fcp);
        }
    }
    if let Some(inp) = payload.vitals.inp {
        if finite_in_range(inp, VITAL_MAX_MS) {
            record_ms("rum.inp_ms", page, ua_class, inp);
        }
    }
    if let Some(cls) = payload.vitals.cls {
        if finite_in_range(cls, CLS_MAX) {
            histogram::record(
                MetricKey::new(
                    "rum.cls",
                    &[("page", page), ("ua_class", ua_class)],
                ),
                cls,
            );
        }
    }
    if let Some(dcl) = payload.vitals.nav_dcl {
        if finite_in_range(dcl, VITAL_MAX_MS) {
            record_ms("rum.nav_dcl_ms", page, ua_class, dcl);
        }
    }
    if let Some(load) = payload.vitals.nav_load {
        if finite_in_range(load, VITAL_MAX_MS) {
            record_ms("rum.nav_load_ms", page, ua_class, load);
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_page_accepts_known_set() {
        assert_eq!(validate_page("home").unwrap(), "home");
        assert_eq!(validate_page("editor").unwrap(), "editor");
        assert_eq!(validate_page("spreadsheet").unwrap(), "spreadsheet");
        assert_eq!(validate_page("other").unwrap(), "other");
    }

    #[test]
    fn validate_page_rejects_unknown() {
        assert!(validate_page("admin").is_err());
        assert!(validate_page("").is_err());
        assert!(validate_page("HOME").is_err()); // case-sensitive
    }

    fn ua_header(ua: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("user-agent", ua.parse().unwrap());
        h
    }

    #[test]
    fn classify_user_agent_desktop_default() {
        let ua = ua_header("Mozilla/5.0 (X11; Linux x86_64)");
        assert_eq!(classify_user_agent(&ua), "desktop");
    }

    #[test]
    fn classify_user_agent_mobile_via_mobi() {
        let ua = ua_header(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit/605 Mobile/15E148",
        );
        assert_eq!(classify_user_agent(&ua), "mobile");
    }

    #[test]
    fn classify_user_agent_mobile_via_android() {
        let ua = ua_header("Mozilla/5.0 (Linux; Android 14) Chrome/120");
        assert_eq!(classify_user_agent(&ua), "mobile");
    }

    #[test]
    fn classify_user_agent_tablet_via_ipad() {
        let ua = ua_header(
            "Mozilla/5.0 (iPad; CPU OS 17_0) AppleWebKit/605 Version/17 Safari/605",
        );
        assert_eq!(classify_user_agent(&ua), "tablet");
    }

    #[test]
    fn classify_user_agent_missing_header_is_desktop() {
        let ua = HeaderMap::new();
        assert_eq!(classify_user_agent(&ua), "desktop");
    }

    #[test]
    fn finite_in_range_clamps_nan_and_negative() {
        assert!(!finite_in_range(f64::NAN, 1000.0));
        assert!(!finite_in_range(f64::INFINITY, 1000.0));
        assert!(!finite_in_range(-0.5, 1000.0));
        assert!(!finite_in_range(2000.0, 1000.0));
        assert!(finite_in_range(0.0, 1000.0));
        assert!(finite_in_range(500.0, 1000.0));
        assert!(finite_in_range(1000.0, 1000.0));
    }
}
