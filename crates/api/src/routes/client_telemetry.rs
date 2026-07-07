// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! `POST /api/v1/client-telemetry` — frontend metric ingest.
//!
//! Accepts a small JSON batch of counter deltas from the WASM
//! frontend's `client.*` metric surface and projects them into the
//! same in-process recorder the server uses for its own counters.
//! Enables the discrepancy_map agent (Phase 3) to compare paired
//! `client.*` and server-side counters on one axis — see
//! `design/observability.md` for the full design.
//!
//! Trust posture (the L4 edge contract for this endpoint):
//!   - Authentication required (no anonymous metric writes).
//!   - Per-user rate limit (`rate_limit_client_telemetry_per_min`,
//!     default 12/min — a healthy client batches every ~10 s).
//!   - Body capped at 16 KiB — the batch is just counter names +
//!     small ints.
//!   - Metric names matched against `CLIENT_METRIC_ALLOWLIST`
//!     (matched by literal equality against `&'static str` entries
//!     so the `MetricKey` carries a static name as the recorder
//!     requires). Unknown names emit
//!     `client_telemetry.unknown_metric_total{name}` and the
//!     request is rejected as `400 Bad Request` — keeps a
//!     compromised token from inflating CloudWatch cost with
//!     arbitrary metric names.
//!   - Phase 1 accepts no dimensions on client metrics. The six
//!     baseline counters are dimensionless in
//!     `design/observability.md`. Dimensioned client metrics
//!     (e.g. `client.ws.frames_sent_total{type}`) land in Phase 2
//!     with a vetted label allowlist alongside envelope versioning.
//!   - Logs MUST NOT contain document content. The batch carries
//!     counts only — never payload bytes.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use ogrenotes_common::metrics::{counter, MetricKey};
use serde::Deserialize;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Cap on the request body. The batch is just JSON metric names +
/// small ints; 16 KiB covers ~200 entries which is well above a
/// healthy 10-second flush.
const MAX_BODY_BYTES: usize = 16 * 1024;

/// Maximum entries per batch. Prevents one batch from amortizing
/// past the rate limit by stuffing thousands of names into a single
/// request.
const MAX_ENTRIES_PER_BATCH: usize = 200;

/// Cap on the rejected-name label so a compromised client can't
/// blow the CloudWatch dimension-value size limit. Operators care
/// that *something* unknown arrived more than they care about the
/// exact 9-KiB string.
const MAX_DIMENSION_VALUE_LEN: usize = 64;

/// Allowlist of metric names the frontend may post. Phase 1's six
/// counters; Phase 2 grows this list alongside the envelope work.
/// Anything not in the list is rejected — see module docs.
///
/// Stored as `&'static str` so `MetricKey::new` (which requires a
/// `'static` name) can use the matched literal directly.
const CLIENT_METRIC_ALLOWLIST: &[&str] = &[
    "client.editor.transactions_total",
    "client.collab.observe_fired_total",
    "client.collab.pending_updates_drained_total",
    "client.ws.frames_sent_total",
    "client.ws.send_errors_total",
    "client.ws.remote_frames_received_total",
    // Bridge diagnostic counters — see frontend
    // observability::SYNC_* docs.
    "client.collab.sync_children_calls_total",
    "client.collab.sync_model_blocks_total",
    "client.collab.sync_matched_blocks_total",
    "client.collab.sync_slow_path_total",
    // #96: legacy blockId-fallback drain gauge — see frontend
    // observability::BLOCKID_CONTAINER_FALLBACK.
    "client.collab.blockid_container_fallback_total",
];

pub fn router() -> Router<AppState> {
    Router::new().route("/", post(ingest))
}

#[derive(Debug, Deserialize)]
struct TelemetryBatch {
    #[serde(default)]
    counters: Vec<CounterDelta>,
}

#[derive(Debug, Deserialize)]
struct CounterDelta {
    name: String,
    delta: u64,
}

async fn ingest(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    body: axum::body::Bytes,
) -> Result<StatusCode, ApiError> {
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "client_telemetry",
        &user_id,
        state.config.rate_limit_client_telemetry_per_min,
        60,
    )
    .await?;

    if body.len() > MAX_BODY_BYTES {
        return Err(ApiError::BadRequest(format!(
            "client-telemetry batch too large: {} bytes (max {MAX_BODY_BYTES})",
            body.len(),
        )));
    }

    let batch: TelemetryBatch = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("malformed client-telemetry batch: {e}")))?;

    if batch.counters.len() > MAX_ENTRIES_PER_BATCH {
        return Err(ApiError::BadRequest(format!(
            "client-telemetry batch has too many entries: {} (max {MAX_ENTRIES_PER_BATCH})",
            batch.counters.len(),
        )));
    }

    project_counters(&batch.counters)
}

/// Project an accepted batch into the in-process recorder. Pure
/// side-effect helper extracted for unit testability — the request
/// handler owns the auth / rate-limit / size checks; this owns the
/// allowlist + emission.
///
/// Returns 204 on success, 400 if any entry's name is not in the
/// allowlist. We reject the whole batch on a single bad entry —
/// accepting partial batches would let a compromised client smuggle
/// arbitrary names through by interleaving them with valid ones,
/// and the rejected-name counter still fires so we see the skew.
fn project_counters(counters: &[CounterDelta]) -> Result<StatusCode, ApiError> {
    // First pass: validate every entry. Build a list of
    // `(allowlist_static_name, delta)` so the second pass can emit
    // without re-matching.
    let mut accepted: Vec<(&'static str, u64)> = Vec::with_capacity(counters.len());
    for entry in counters {
        match resolve_allowlisted(&entry.name) {
            Some(static_name) => accepted.push((static_name, entry.delta)),
            None => {
                let truncated = truncate_for_label(&entry.name).to_string();
                counter::inc(MetricKey::new(
                    "client_telemetry.unknown_metric_total",
                    &[("name_prefix", truncated.as_str())],
                ));
                return Err(ApiError::BadRequest(format!(
                    "client-telemetry: unknown metric name {:?}",
                    truncated,
                )));
            }
        }
    }

    for (name, delta) in accepted {
        counter::add(MetricKey::new(name, &[]), delta);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Returns the matching `&'static str` from `CLIENT_METRIC_ALLOWLIST`
/// if `name` equals an entry, else `None`. The static reference is
/// what `MetricKey::new` needs.
fn resolve_allowlisted(name: &str) -> Option<&'static str> {
    CLIENT_METRIC_ALLOWLIST
        .iter()
        .copied()
        .find(|&allowed| allowed == name)
}

/// Cap the rejected-name label so a compromised client can't blow
/// the CloudWatch dimension-value size limit by sending megabytes of
/// "name". Walks back to the nearest char boundary so the slice is
/// still valid UTF-8.
fn truncate_for_label(name: &str) -> &str {
    if name.len() <= MAX_DIMENSION_VALUE_LEN {
        return name;
    }
    let mut end = MAX_DIMENSION_VALUE_LEN;
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> CounterDelta {
        CounterDelta {
            name: name.to_string(),
            delta: 1,
        }
    }

    #[test]
    fn accepts_every_allowlisted_metric() {
        let counters: Vec<_> = CLIENT_METRIC_ALLOWLIST
            .iter()
            .map(|n| entry(n))
            .collect();
        let resp = project_counters(&counters).expect("allowlisted batch must accept");
        assert_eq!(resp, StatusCode::NO_CONTENT);
    }

    #[test]
    fn rejects_unknown_metric_name() {
        let bad = vec![entry("client.evil.injected")];
        let err = project_counters(&bad)
            .expect_err("unknown metric name must be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn rejects_whole_batch_on_one_bad_entry() {
        let mixed = vec![
            entry(CLIENT_METRIC_ALLOWLIST[0]),
            entry("client.evil.injected"),
            entry(CLIENT_METRIC_ALLOWLIST[1]),
        ];
        let err = project_counters(&mixed)
            .expect_err("mixed batch must reject as a unit");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn truncate_preserves_utf8_boundaries() {
        // Each "é" is 2 bytes. The byte cap lands mid-codepoint;
        // the trimmer must step back to a boundary, not panic and
        // not produce invalid UTF-8.
        let long: String = "é".repeat(100);
        let trimmed = truncate_for_label(&long);
        assert!(trimmed.len() <= MAX_DIMENSION_VALUE_LEN);
        assert!(std::str::from_utf8(trimmed.as_bytes()).is_ok());
    }

    #[test]
    fn resolve_returns_static_reference_for_match() {
        let s = resolve_allowlisted("client.editor.transactions_total")
            .expect("first allowlist entry must resolve");
        // The returned &str must be the same memory as the constant
        // — that's what gives us 'static-ness.
        assert!(std::ptr::eq(
            s.as_ptr(),
            CLIENT_METRIC_ALLOWLIST[0].as_ptr(),
        ));
    }
}
