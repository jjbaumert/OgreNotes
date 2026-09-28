// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Is the storage backend reachable?
//!
//! A self-hosted server keeps its data in DynamoDB and S3 over the
//! internet, so losing the connection is an expected condition, not a
//! crash. Without a signal for it, every request waits out the SDK's
//! connect-and-retry budget and then fails with a generic 500, and a
//! failed edit save is invisible to the user. This module keeps one
//! answer to "can we reach storage right now?", refreshed by a cheap
//! background probe, so that:
//!
//! - API requests fail fast with a clear 503 while storage is down
//!   ([`fail_fast_while_unreachable`]);
//! - the browser can ask (`GET /api/v1/status`) and show a banner;
//! - the collaboration layer knows when to write out edits whose save
//!   failed during the outage ([`spawn_probe`]'s recovery hook).
//!
//! The probe reads one DynamoDB item and heads one S3 object. Both are
//! expected to be absent; "not found" is a *successful* round trip.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::s3::S3Client;

use crate::state::AppState;

/// Probe cadence while storage is reachable.
const PROBE_INTERVAL_UP: Duration = Duration::from_secs(15);
/// Probe cadence while it isn't — recover quickly once it's back.
const PROBE_INTERVAL_DOWN: Duration = Duration::from_secs(5);
/// A probe that takes longer than this counts as a failure.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Default timeouts for every AWS SDK client (DynamoDB overrides them
/// with [`dynamodb_timeouts`]). The SDK's defaults set no overall
/// operation deadline, so with the network gone a request can sit through
/// several slow connection attempts before failing. A short connect
/// timeout makes "unreachable" fail in seconds; the per-attempt and
/// overall deadlines are generous because a snapshot upload over a home
/// uplink can legitimately take a while.
pub fn sdk_timeouts() -> aws_config::timeout::TimeoutConfig {
    aws_config::timeout::TimeoutConfig::builder()
        .connect_timeout(Duration::from_secs(3))
        .operation_attempt_timeout(Duration::from_secs(60))
        .operation_timeout(Duration::from_secs(120))
        .build()
}

/// Tighter timeouts for DynamoDB, whose calls are small: a request stuck
/// on a connection that has gone silent (connect timeout doesn't apply —
/// the connection is already open) gives up in seconds rather than a
/// minute, so a failed edit save is reported while the user is still
/// looking at it.
pub fn dynamodb_timeouts() -> aws_config::timeout::TimeoutConfig {
    aws_config::timeout::TimeoutConfig::builder()
        .connect_timeout(Duration::from_secs(3))
        .operation_attempt_timeout(Duration::from_secs(5))
        .operation_timeout(Duration::from_secs(15))
        .build()
}

/// The key the probe reads. Never written; its absence is the expected
/// answer.
const PROBE_PK: &str = "PROBE#storage-health";
const PROBE_SK: &str = "PROBE";
const PROBE_S3_KEY: &str = "probe/storage-health";

/// Shared reachability state. Starts optimistic (`up`) so a server that
/// boots with working storage never rejects its first requests while the
/// first probe is in flight.
#[derive(Debug)]
pub struct StorageHealth {
    up: AtomicBool,
    /// When `up` last changed, in microseconds since the epoch (0 = never).
    changed_at: AtomicI64,
}

impl Default for StorageHealth {
    fn default() -> Self {
        Self { up: AtomicBool::new(true), changed_at: AtomicI64::new(0) }
    }
}

impl StorageHealth {
    pub fn is_up(&self) -> bool {
        self.up.load(Ordering::Acquire)
    }

    /// When the current state began (µs since epoch), or 0 if it never
    /// changed since boot.
    pub fn since(&self) -> i64 {
        self.changed_at.load(Ordering::Acquire)
    }

    /// Record a probe result. Returns `true` when this call moved the
    /// state from down to up — the moment to write out anything whose
    /// save failed during the outage.
    pub fn record(&self, reachable: bool) -> bool {
        let was = self.up.swap(reachable, Ordering::AcqRel);
        if was != reachable {
            self.changed_at
                .store(ogrenotes_common::time::now_usec(), Ordering::Release);
            if reachable {
                tracing::info!("storage reachable again");
            } else {
                tracing::warn!("storage unreachable — API will answer 503 until it returns");
            }
        }
        !was && reachable
    }
}

/// One round trip to each backend. `Ok(())` when both answered — "not
/// found" included — and `Err` naming the first that didn't.
pub async fn probe_once(db: &DynamoClient, s3: &S3Client) -> Result<(), String> {
    let round_trip = async {
        db.get_item(PROBE_PK, PROBE_SK)
            .await
            .map_err(|e| format!("dynamodb: {e}"))?;
        s3.object_exists(PROBE_S3_KEY)
            .await
            .map_err(|e| format!("s3: {e}"))?;
        Ok::<(), String>(())
    };
    match tokio::time::timeout(PROBE_TIMEOUT, round_trip).await {
        Ok(result) => result,
        Err(_) => Err(format!("no answer within {}s", PROBE_TIMEOUT.as_secs())),
    }
}

/// Probe forever, updating `state.storage_health`, and on recovery write
/// out every room whose edits failed to save during the outage.
pub fn spawn_probe(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let result = probe_once(state.doc_repo.db(), state.doc_repo.s3()).await;
            if let Err(e) = &result {
                tracing::debug!(error = %e, "storage probe failed");
            }
            if state.storage_health.record(result.is_ok()) {
                persist_unsaved_rooms(&state).await;
            }
            let wait = if state.storage_health.is_up() {
                PROBE_INTERVAL_UP
            } else {
                PROBE_INTERVAL_DOWN
            };
            tokio::time::sleep(wait).await;
        }
    })
}

/// Snapshot every room holding edits that failed to save. Rooms keep
/// clients connected (a forced compaction doesn't evict), and a room
/// whose snapshot still fails stays marked for the next recovery.
pub async fn persist_unsaved_rooms(state: &AppState) {
    for doc_id in state.room_registry.unsaved_rooms() {
        match crate::compaction::compact_room_with_outcome(
            &state.room_registry,
            &state.doc_repo,
            &doc_id,
            true,
        )
        .await
        {
            crate::compaction::CompactOutcome::Compacted { .. } => {
                tracing::info!(doc_id, "wrote out edits that failed to save during the outage");
            }
            other => {
                tracing::warn!(doc_id, outcome = ?other, "could not yet write out unsaved edits");
            }
        }
    }
}

/// The fail-fast answer: the same tagged 503 as every other
/// storage-unavailable error ([`crate::error::ApiError::StorageUnavailable`]).
fn unavailable() -> Response {
    crate::error::ApiError::StorageUnavailable.into_response()
}

/// Middleware: while storage is unreachable, answer API requests with a
/// 503 immediately instead of letting each one hang on the SDK's retries.
/// `GET /api/v1/status` and `/health` are exempt — they are how clients
/// and load balancers learn the state.
pub async fn fail_fast_while_unreachable(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let exempt = path == "/health" || path == "/api/v1/status" || !path.starts_with("/api/");
    if !exempt && !state.storage_health.is_up() {
        return unavailable();
    }
    next.run(request).await
}

/// `GET /api/v1/status` — unauthenticated, cheap, and safe to poll: it
/// reads the probe's last answer and touches no backend.
pub async fn status(State(state): State<AppState>) -> impl IntoResponse {
    let health: &Arc<StorageHealth> = &state.storage_health;
    axum::Json(serde_json::json!({
        "storage": if health.is_up() { "ok" } else { "unreachable" },
        "since": health.since(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_up_and_reports_only_the_recovery_edge() {
        let h = StorageHealth::default();
        assert!(h.is_up());
        assert!(!h.record(true), "staying up is not a recovery");
        assert!(!h.record(false), "going down is not a recovery");
        assert!(!h.is_up());
        assert!(h.since() > 0);
        assert!(!h.record(false), "staying down is not a recovery");
        assert!(h.record(true), "down → up is the recovery edge");
        assert!(h.is_up());
        assert!(!h.record(true));
    }
}
