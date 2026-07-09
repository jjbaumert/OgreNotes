// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.4 piece A — async-worker subsystem.
//!
//! Redis-streams-backed job queue. Two API surfaces, one library:
//!
//! - **Producer**: the API server enqueues a [`Job`] via
//!   [`JobQueue::enqueue`]. Returns a stable [`JobId`] the client
//!   can poll on via the future REST endpoint (piece C).
//! - **Consumer**: a worker process calls
//!   [`JobQueue::consume_next`] in a loop, executes the
//!   [`Job::payload`], then calls [`JobQueue::ack`] on success or
//!   [`JobQueue::retry_or_dead_letter`] on failure.
//!
//! Wire shape:
//!
//! - `<stream>`: the main work stream. XADD on enqueue;
//!   XREADGROUP on consume; XACK + XDEL on success.
//! - `<stream>:dlq`: dead-letter stream. XADD when retry budget
//!   exhausts. Operators inspect manually; XCLAIM recoveries
//!   move entries back to `<stream>` after the underlying issue
//!   is fixed.
//! - `job:{id}`: HASH carrying status + result/error + timing.
//!   Side-channel for the GET /jobs/{id} polling API. TTL'd at
//!   24h so old job records drop out without an explicit cleanup
//!   job.
//!
//! The Redis stream entry's intrinsic id (`<ms>-<seq>`) is not
//! exposed; we generate a nanoid on enqueue and carry it in the
//! envelope so consumer-side ack + status updates can route by a
//! stable handle even after a redelivery.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use fred::clients::RedisClient;
use fred::prelude::*;
use fred::types::XReadResponse;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Unique handle for a queued job. Stable across redeliveries —
/// even if the consumer crashes mid-work and the job is XCLAIMed
/// by another worker, the same `JobId` flows through. Clients
/// poll on this via the JobStatus API.
pub type JobId = String;

/// Typed work payloads. Adding a new job kind:
/// 1. Add a variant here with the field shape the worker needs.
/// 2. Update the consumer side's dispatch (typically a `match`
///    on `Job::payload`).
/// 3. Wire the producer-side caller to construct the variant.
///
/// The `#[serde(tag = "type")]` discriminator lands as a top-level
/// `type` field in the JSON wire form so future variants can be
/// detected without parsing the whole payload (handy for the
/// dead-letter operator runbook).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Job {
    /// Placeholder for the M-6.5 DOCX import path. Carries the
    /// raw upload key (S3 object id) and the target folder. The
    /// worker side will fetch from S3, parse the DOCX, and POST
    /// the result into the document repo.
    ImportDocx {
        s3_key: String,
        title: String,
        folder_id: Option<String>,
        owner_id: String,
    },
    /// Placeholder for the M-6.6 PDF import path. Same shape as
    /// DOCX, distinct variant because the parser differs.
    ImportPdf {
        s3_key: String,
        title: String,
        folder_id: Option<String>,
        owner_id: String,
    },
    /// No-op job — used by tests to verify the queue mechanics
    /// without dragging in DOCX/PDF deps. Carries an arbitrary
    /// label so a test can correlate the dequeue.
    Noop { label: String },
}

/// Wrapper around [`Job`] with queue-level metadata. Serialized
/// as the body of one XADD field; consumers parse this back into
/// the typed shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobEnvelope {
    /// Stable identifier — nanoid generated on enqueue.
    pub job_id: JobId,
    /// Wall-clock ms when the job was first enqueued. Persists
    /// across retries so end-to-end latency is observable.
    pub enqueued_at_ms: u64,
    /// 0 on first delivery, incremented on each retry XADD. The
    /// dead-letter cutoff compares this against a max-retries
    /// budget supplied per-call to retry_or_dead_letter.
    pub attempt: u32,
    /// User id permitted to poll this job's status side-channel.
    /// Derived from the payload at enqueue time (Import{Docx,Pdf}
    /// carry `owner_id`; `Noop` has no owner concept and stays
    /// `None` — its status remains a bearer capability). `#[serde
    /// (default)]` so envelopes from before this field was added
    /// deserialize as ownerless. (#85)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The work payload.
    pub payload: Job,
}

/// Derive the polling owner from a payload — only the import jobs
/// have one, so a Noop is ownerless (any authed caller can poll its
/// label echo, which carries no sensitive data).
fn owner_of(payload: &Job) -> Option<&str> {
    match payload {
        Job::ImportDocx { owner_id, .. } | Job::ImportPdf { owner_id, .. } => {
            Some(owner_id.as_str())
        }
        Job::Noop { .. } => None,
    }
}

/// Status snapshot read from the `job:{id}` side-channel hash.
/// Returned by the future GET /jobs/{id} poll endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum JobStatus {
    /// Enqueued but not yet picked up. The producer side writes
    /// this immediately after XADD so a fast poll returns
    /// something meaningful even before the first consumer reads.
    Pending,
    /// Picked up by a consumer; work in progress. The consumer
    /// writes this on its first XREADGROUP claim.
    Running { worker: String, started_at_ms: u64 },
    /// Ack'd; the optional `result_json` carries whatever the
    /// worker wants the client to see (e.g. created document id
    /// for an import job).
    Succeeded {
        finished_at_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_json: Option<String>,
    },
    /// Retry budget exhausted. The error string is the last
    /// observed failure reason; the full entry stays on the
    /// `<stream>:dlq` stream for operator triage.
    Failed { finished_at_ms: u64, error: String },
}

#[derive(Debug, Error)]
pub enum JobError {
    #[error("redis: {0}")]
    Redis(String),
    #[error("serialize: {0}")]
    Serialize(String),
    #[error("not found: {0}")]
    NotFound(String),
}

impl From<RedisError> for JobError {
    fn from(e: RedisError) -> Self {
        JobError::Redis(e.to_string())
    }
}

/// TTL for the `job:{id}` status hash. 24 hours is enough for the
/// REST poll loop in piece C plus a comfortable triage window
/// when the worker crashed and operators need the state to
/// investigate. Long-lived job audit lives in DynamoDB if
/// needed, not Redis.
const JOB_STATUS_TTL_SECS: u64 = 24 * 60 * 60;

/// Default consumer group. One group per deployment; multiple
/// worker instances join as different consumers within the
/// group. XREADGROUP across the same group distributes entries
/// across consumers, so adding a worker just bumps throughput.
pub const DEFAULT_GROUP: &str = "workers";

/// Field name inside the stream entry. Single field carrying the
/// full JSON envelope is simpler than a multi-field record;
/// XADD's pair shape is `<field> <value>` so we use one pair.
const ENVELOPE_FIELD: &str = "envelope";

/// Job queue handle. Cheap to clone (wraps an `Arc<RedisClient>`).
#[derive(Clone)]
pub struct JobQueue {
    client: Arc<RedisClient>,
    stream: String,
    group: String,
}

impl JobQueue {
    /// Construct a queue handle pointing at `stream`. Creates the
    /// consumer group if it doesn't exist; the BUSYGROUP error on
    /// the create path is treated as a no-op (idempotent).
    pub async fn new(
        client: Arc<RedisClient>,
        stream: impl Into<String>,
    ) -> Result<Self, JobError> {
        Self::new_with_group(client, stream, DEFAULT_GROUP).await
    }

    pub async fn new_with_group(
        client: Arc<RedisClient>,
        stream: impl Into<String>,
        group: impl Into<String>,
    ) -> Result<Self, JobError> {
        let stream = stream.into();
        let group = group.into();
        // XGROUP CREATE <stream> <group> $ MKSTREAM — `MKSTREAM`
        // auto-creates the stream key if it doesn't exist;
        // otherwise XREADGROUP on a missing stream errors hard.
        // BUSYGROUP on a re-create is the steady state.
        // "$" = the well-known sentinel id meaning "new entries
        // only" (per the Redis XGROUP CREATE docs); fred's
        // example uses the same literal.
        let res: Result<(), RedisError> = client
            .xgroup_create(stream.as_str(), group.as_str(), "$", true)
            .await;
        match res {
            Ok(()) => {}
            Err(e) if e.to_string().contains("BUSYGROUP") => {}
            Err(e) => return Err(JobError::Redis(e.to_string())),
        }
        Ok(Self { client, stream, group })
    }

    /// Enqueue a job. Returns the stable [`JobId`] (nanoid). Writes
    /// the side-channel status hash to `Pending` before returning
    /// so an immediate poll sees the work.
    pub async fn enqueue(&self, payload: Job) -> Result<JobId, JobError> {
        let owner = owner_of(&payload).map(String::from);
        let envelope = JobEnvelope {
            job_id: nanoid::nanoid!(),
            enqueued_at_ms: now_ms(),
            attempt: 0,
            owner: owner.clone(),
            payload,
        };
        self.xadd(&envelope).await?;
        self.write_status(&envelope.job_id, &JobStatus::Pending, owner.as_deref())
            .await?;
        Ok(envelope.job_id)
    }

    /// XADD the envelope onto the main stream. Used both at first
    /// enqueue and at retry. Internal — callers go through
    /// `enqueue` or `retry_or_dead_letter`.
    async fn xadd(&self, envelope: &JobEnvelope) -> Result<String, JobError> {
        let json = serde_json::to_string(envelope)
            .map_err(|e| JobError::Serialize(e.to_string()))?;
        // fred 9.4 takes a Vec<(&str, &str)> pair directly; no
        // need to wrap in MultipleOrderedPairs (the trait
        // resolution there is brittle).
        let id: String = self
            .client
            .xadd(
                self.stream.as_str(),
                false,
                None,
                "*",
                vec![(ENVELOPE_FIELD, json.as_str())],
            )
            .await?;
        Ok(id)
    }

    /// Block-read the next entry off the stream as `consumer`.
    /// `block_ms` = 0 → block indefinitely until an entry arrives.
    /// Returns `Ok(None)` on the explicit "no message within the
    /// block window" timeout (only possible with block_ms > 0).
    pub async fn consume_next(
        &self,
        consumer: &str,
        block_ms: u64,
    ) -> Result<Option<ClaimedJob>, JobError> {
        // fred 9.4's `xreadgroup_map` expands the response into a
        // typed `XReadResponse` via `into_xread_response()`. When the
        // server returns NIL — i.e. no entries arrived within the
        // block window, the steady-state "empty queue" case — that
        // conversion panics out as `Redis("Parse Error: Cannot
        // convert to map.")`. Treat that specific error as a normal
        // "no entries" return so the consumer loop doesn't need to
        // squint at fred's internals on every empty poll.
        let resp: XReadResponse<String, String, String, String> = match self
            .client
            .xreadgroup_map(
                self.group.as_str(),
                consumer,
                Some(1),
                Some(block_ms),
                false,
                self.stream.as_str(),
                ">",
            )
            .await
        {
            Ok(r) => r,
            Err(e) if e.to_string().contains("Cannot convert to map") => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let Some(entries) = resp.get(&self.stream) else {
            return Ok(None);
        };
        let Some((stream_id, fields)) = entries.iter().next() else {
            return Ok(None);
        };
        let envelope_json = fields
            .get(ENVELOPE_FIELD)
            .ok_or_else(|| JobError::Serialize(
                format!("stream entry {stream_id} missing field {ENVELOPE_FIELD}"),
            ))?;
        let envelope: JobEnvelope = serde_json::from_str(envelope_json)
            .map_err(|e| JobError::Serialize(e.to_string()))?;
        // Flip status to Running before the consumer starts work
        // so a polling client sees the transition without waiting
        // for the worker to update.
        let started = JobStatus::Running {
            worker: consumer.to_string(),
            started_at_ms: now_ms(),
        };
        self.write_status(&envelope.job_id, &started, envelope.owner.as_deref())
            .await?;
        Ok(Some(ClaimedJob {
            stream_id: stream_id.clone(),
            envelope,
        }))
    }

    /// Mark a job complete: XACK + XDEL on the stream, write the
    /// Succeeded status. `result_json` is whatever the worker
    /// wants the client to see on GET /jobs/{id}; pass None when
    /// the work has no result body.
    pub async fn ack(
        &self,
        claimed: &ClaimedJob,
        result_json: Option<String>,
    ) -> Result<(), JobError> {
        let _: u64 = self
            .client
            .xack(
                self.stream.as_str(),
                self.group.as_str(),
                claimed.stream_id.as_str(),
            )
            .await?;
        let _: u64 = self
            .client
            .xdel(self.stream.as_str(), vec![claimed.stream_id.as_str()])
            .await?;
        let status = JobStatus::Succeeded {
            finished_at_ms: now_ms(),
            result_json,
        };
        self.write_status(
            &claimed.envelope.job_id,
            &status,
            claimed.envelope.owner.as_deref(),
        )
        .await?;
        Ok(())
    }

    /// Handle a failed work attempt. If `attempt < max_retries`,
    /// XACK + XDEL the current entry and XADD a new envelope with
    /// `attempt + 1` — keeps the JobId stable. Otherwise, XACK +
    /// XDEL + XADD-to-dlq + Failed status.
    ///
    /// The caller decides `max_retries` per-job-kind: cheap
    /// idempotent work (Noop, status writes) tolerates many
    /// retries; expensive non-idempotent work (PDF rendering)
    /// might do zero retries and dead-letter immediately.
    pub async fn retry_or_dead_letter(
        &self,
        claimed: &ClaimedJob,
        max_retries: u32,
        error: &str,
    ) -> Result<RetryOutcome, JobError> {
        let _: u64 = self
            .client
            .xack(
                self.stream.as_str(),
                self.group.as_str(),
                claimed.stream_id.as_str(),
            )
            .await?;
        let _: u64 = self
            .client
            .xdel(self.stream.as_str(), vec![claimed.stream_id.as_str()])
            .await?;
        if claimed.envelope.attempt < max_retries {
            let next = JobEnvelope {
                job_id: claimed.envelope.job_id.clone(),
                enqueued_at_ms: claimed.envelope.enqueued_at_ms,
                attempt: claimed.envelope.attempt + 1,
                owner: claimed.envelope.owner.clone(),
                payload: claimed.envelope.payload.clone(),
            };
            self.xadd(&next).await?;
            self.write_status(&next.job_id, &JobStatus::Pending, next.owner.as_deref())
                .await?;
            Ok(RetryOutcome::Retried { attempt: next.attempt })
        } else {
            // Dead-letter — same envelope written to <stream>:dlq.
            // Operators inspect manually; XCLAIM from dlq back to
            // the main stream is the recovery path.
            let dlq_stream = format!("{}:dlq", self.stream);
            let json = serde_json::to_string(&claimed.envelope)
                .map_err(|e| JobError::Serialize(e.to_string()))?;
            let _: String = self
                .client
                .xadd(
                    dlq_stream.as_str(),
                    false,
                    None,
                    "*",
                    vec![
                        (ENVELOPE_FIELD, json.as_str()),
                        ("lastError", error),
                    ],
                )
                .await?;
            let status = JobStatus::Failed {
                finished_at_ms: now_ms(),
                error: error.to_string(),
            };
            self.write_status(
                &claimed.envelope.job_id,
                &status,
                claimed.envelope.owner.as_deref(),
            )
            .await?;
            Ok(RetryOutcome::DeadLettered)
        }
    }

    /// Read the side-channel status hash for `job_id`. Returns
    /// `NotFound` when the job is unknown or the 24h TTL has
    /// expired.
    pub async fn status(&self, job_id: &str) -> Result<JobStatus, JobError> {
        let key = status_key(job_id);
        // HGET <key> json — single-field read avoids HGETALL's
        // RedisMap parsing surface (which is awkward in fred 9.4).
        let json: Option<String> = self.client.hget(key.as_str(), "json").await?;
        let json = json.ok_or_else(|| JobError::NotFound(job_id.to_string()))?;
        serde_json::from_str(&json)
            .map_err(|e| JobError::Serialize(e.to_string()))
    }

    /// Like [`status`], but also returns the job's `owner` (the user
    /// id allowed to poll it) when the side-channel hash carries one.
    /// Used by the `GET /jobs/{id}` route to enforce per-job
    /// ownership without leaking job existence to non-owners. (#85)
    pub async fn poll(
        &self,
        job_id: &str,
    ) -> Result<(JobStatus, Option<String>), JobError> {
        let key = status_key(job_id);
        let json: Option<String> = self.client.hget(key.as_str(), "json").await?;
        let json = json.ok_or_else(|| JobError::NotFound(job_id.to_string()))?;
        let status: JobStatus = serde_json::from_str(&json)
            .map_err(|e| JobError::Serialize(e.to_string()))?;
        let owner: Option<String> = self.client.hget(key.as_str(), "owner").await?;
        Ok((status, owner))
    }

    /// XCLAIM ownership of entries that have been pending in the
    /// group for `min_idle_ms` — recovers work from a worker that
    /// crashed mid-task without ack'ing. Returns the claimed
    /// envelopes; the caller should treat each as a fresh
    /// `ClaimedJob` and proceed to execute.
    ///
    /// Runs as part of the worker's main loop on a periodic
    /// timer (every ~30s); not called from the producer side.
    pub async fn claim_stale(
        &self,
        consumer: &str,
        min_idle_ms: u64,
        max_count: usize,
    ) -> Result<Vec<ClaimedJob>, JobError> {
        // XAUTOCLAIM walks the consumer-group's pending list
        // itself and returns up to `count` entries this consumer
        // can take over without an explicit XPENDING walk first.
        // fred 9.4's xautoclaim_values returns
        // (next_cursor, Vec<XReadValue<Ri, Rk, Rv>>) where each
        // XReadValue is (stream_id, HashMap<field, value>).
        let (_next_cursor, entries): (
            String,
            Vec<(String, std::collections::HashMap<String, String>)>,
        ) = self
            .client
            .xautoclaim_values(
                self.stream.as_str(),
                self.group.as_str(),
                consumer,
                min_idle_ms,
                "0-0",
                Some(max_count as u64),
                false,
            )
            .await?;
        let mut out = Vec::with_capacity(entries.len());
        for (stream_id, fields) in entries {
            let envelope_json = fields.get(ENVELOPE_FIELD).ok_or_else(|| {
                JobError::Serialize(format!(
                    "claimed entry {stream_id} missing field {ENVELOPE_FIELD}"
                ))
            })?;
            let envelope: JobEnvelope = serde_json::from_str(envelope_json)
                .map_err(|e| JobError::Serialize(e.to_string()))?;
            out.push(ClaimedJob { stream_id, envelope });
        }
        Ok(out)
    }

    /// Internal: write the status hash with 24h TTL. Sets `json` and,
    /// when `owner` is `Some`, the `owner` field used by the poll-time
    /// ownership check. Re-asserting the same owner on subsequent
    /// status writes is idempotent.
    async fn write_status(
        &self,
        job_id: &str,
        status: &JobStatus,
        owner: Option<&str>,
    ) -> Result<(), JobError> {
        let key = status_key(job_id);
        let json = serde_json::to_string(status)
            .map_err(|e| JobError::Serialize(e.to_string()))?;
        let mut fields: Vec<(&str, &str)> = vec![("json", json.as_str())];
        if let Some(o) = owner {
            fields.push(("owner", o));
        }
        let _: u64 = self.client.hset(key.as_str(), fields).await?;
        let _: bool = self
            .client
            .expire(key.as_str(), JOB_STATUS_TTL_SECS as i64)
            .await?;
        Ok(())
    }
}

fn status_key(job_id: &str) -> String {
    format!("job:{job_id}")
}

/// A consumer's view of a claimed entry. Wraps the parsed
/// envelope with the underlying stream id needed for XACK/XDEL.
#[derive(Debug, Clone)]
pub struct ClaimedJob {
    pub stream_id: String,
    pub envelope: JobEnvelope,
}

/// Outcome of [`JobQueue::retry_or_dead_letter`].
#[derive(Debug, Clone, PartialEq)]
pub enum RetryOutcome {
    Retried { attempt: u32 },
    DeadLettered,
}

/// Trait-shaped abstraction so the producer side (the API task)
/// can take `Arc<dyn JobProducer>` and tests can substitute a
/// no-Redis fake.
#[async_trait]
pub trait JobProducer: Send + Sync {
    async fn enqueue(&self, payload: Job) -> Result<JobId, JobError>;
    async fn status(&self, job_id: &str) -> Result<JobStatus, JobError>;
    /// Status + owner side-channel for the per-job ownership check
    /// on the `GET /jobs/{id}` route. Owner is `None` for jobs whose
    /// payload carries no owner concept (currently just `Noop`).
    ///
    /// # Implementor note — override this for any owned-job impl
    ///
    /// The default returns `(status, None)`, which **silently
    /// bypasses the ownership check in the route handler** for any
    /// job that would otherwise be owned. That's fine for an
    /// in-memory no-infra fake that only enqueues `Noop`, but any
    /// impl that can enqueue `ImportDocx` / `ImportPdf` **must
    /// override** this method and return the stored owner, or the
    /// route's auth gate is silently absent. Production `JobQueue`
    /// overrides with the real two-field Redis read. (#85)
    async fn poll(
        &self,
        job_id: &str,
    ) -> Result<(JobStatus, Option<String>), JobError> {
        Ok((self.status(job_id).await?, None))
    }
}

#[async_trait]
impl JobProducer for JobQueue {
    async fn enqueue(&self, payload: Job) -> Result<JobId, JobError> {
        JobQueue::enqueue(self, payload).await
    }
    async fn status(&self, job_id: &str) -> Result<JobStatus, JobError> {
        JobQueue::status(self, job_id).await
    }
    async fn poll(
        &self,
        job_id: &str,
    ) -> Result<(JobStatus, Option<String>), JobError> {
        JobQueue::poll(self, job_id).await
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_roundtrips_through_serde() {
        let original = JobEnvelope {
            job_id: "abc123".to_string(),
            enqueued_at_ms: 1717000000000,
            attempt: 0,
            owner: Some("user-1".to_string()),
            payload: Job::ImportDocx {
                s3_key: "uploads/abc.docx".to_string(),
                title: "Q4 plan".to_string(),
                folder_id: Some("folder-1".to_string()),
                owner_id: "user-1".to_string(),
            },
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: JobEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
        // Sanity-check the type-tag landed in the JSON for the
        // operator dead-letter triage runbook.
        assert!(json.contains("\"type\":\"importDocx\""));
    }

    #[test]
    fn status_serialization_uses_state_discriminator() {
        let s = JobStatus::Running {
            worker: "worker-1".to_string(),
            started_at_ms: 100,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"state\":\"running\""));
        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s, parsed);
    }

    #[test]
    fn succeeded_status_skips_null_result_json() {
        let s = JobStatus::Succeeded {
            finished_at_ms: 100,
            result_json: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("resultJson"));
    }

    #[test]
    fn noop_job_variant_supports_test_isolation() {
        let job = Job::Noop {
            label: "test-1".to_string(),
        };
        let json = serde_json::to_string(&job).unwrap();
        let parsed: Job = serde_json::from_str(&json).unwrap();
        assert_eq!(job, parsed);
    }

    /// Pre-#85 envelopes were written without the `owner` field.
    /// `#[serde(default)]` must keep them deserializable as
    /// ownerless — a redeploy mid-flight must not strand queued
    /// entries with a parse error.
    #[test]
    fn envelope_without_owner_field_deserializes_as_ownerless() {
        let json = r#"{"jobId":"j-1","enqueuedAtMs":1000,"attempt":2,"payload":{"type":"noop","label":"old"}}"#;
        let env: JobEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(env.owner, None);
        assert_eq!(env.job_id, "j-1");
        assert_eq!(env.enqueued_at_ms, 1000);
        assert_eq!(env.attempt, 2);
        assert_eq!(env.payload, Job::Noop { label: "old".to_string() });
    }

    /// Ownerless envelopes must omit the `owner` key entirely (not
    /// write `"owner":null`) so pre-#85 consumers keep parsing the
    /// wire form, and the envelope struct's keys stay camelCase —
    /// they're a Redis wire contract, not an internal detail.
    #[test]
    fn ownerless_envelope_omits_owner_key_and_uses_camel_case() {
        let env = JobEnvelope {
            job_id: "j-2".to_string(),
            enqueued_at_ms: 7,
            attempt: 0,
            owner: None,
            payload: Job::Noop { label: "x".to_string() },
        };
        let value = serde_json::to_value(&env).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("owner"), "None owner must be omitted");
        for key in ["jobId", "enqueuedAtMs", "attempt", "payload"] {
            assert!(obj.contains_key(key), "missing envelope key {key}");
        }
    }

    /// The `type` tag is the dead-letter triage discriminator; the
    /// existing test pins `importDocx`, this pins the other two so
    /// a variant rename can't slip through unnoticed.
    #[test]
    fn import_pdf_and_noop_type_tags_are_pinned() {
        let pdf = Job::ImportPdf {
            s3_key: "k".to_string(),
            title: "t".to_string(),
            folder_id: None,
            owner_id: "u".to_string(),
        };
        let value = serde_json::to_value(&pdf).unwrap();
        assert_eq!(value.get("type").and_then(|t| t.as_str()), Some("importPdf"));

        let noop = Job::Noop { label: "n".to_string() };
        let value = serde_json::to_value(&noop).unwrap();
        assert_eq!(value.get("type").and_then(|t| t.as_str()), Some("noop"));
    }

    /// The `state` tags are what the frontend poll loop matches on
    /// (`pending` / `running` / `succeeded` / `failed`); `running`
    /// is pinned by an existing test, this pins the rest, and the
    /// field-free Pending form is pinned exactly.
    #[test]
    fn terminal_status_state_tags_and_roundtrips() {
        assert_eq!(
            serde_json::to_string(&JobStatus::Pending).unwrap(),
            r#"{"state":"pending"}"#,
        );

        let succeeded = JobStatus::Succeeded {
            finished_at_ms: 42,
            result_json: Some(r#"{"docId":"d1"}"#.to_string()),
        };
        let json = serde_json::to_string(&succeeded).unwrap();
        assert!(json.contains(r#""state":"succeeded""#));
        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(succeeded, parsed);

        let failed = JobStatus::Failed {
            finished_at_ms: 43,
            error: "boom".to_string(),
        };
        let json = serde_json::to_string(&failed).unwrap();
        assert!(json.contains(r#""state":"failed""#));
        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(failed, parsed);
    }

    /// A Succeeded status written without a result body omits the
    /// key on the wire (pinned by an existing test); reading it back
    /// must land on `result_json: None` via `#[serde(default)]`, not
    /// a missing-field error — that's the shape a poll sees for jobs
    /// whose worker had no result to report.
    #[test]
    fn succeeded_status_roundtrips_when_result_json_omitted() {
        let status = JobStatus::Succeeded {
            finished_at_ms: 9,
            result_json: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, parsed);
    }

    /// Owner derivation is the input to the #85 poll-time ownership
    /// gate: both import variants must yield their `owner_id`, and
    /// Noop must stay ownerless (bearer-capability polling).
    #[test]
    fn owner_derivation_matches_payload_kind() {
        let docx = Job::ImportDocx {
            s3_key: "k".to_string(),
            title: "t".to_string(),
            folder_id: Some("f".to_string()),
            owner_id: "user-docx".to_string(),
        };
        assert_eq!(owner_of(&docx), Some("user-docx"));

        let pdf = Job::ImportPdf {
            s3_key: "k".to_string(),
            title: "t".to_string(),
            folder_id: None,
            owner_id: "user-pdf".to_string(),
        };
        assert_eq!(owner_of(&pdf), Some("user-pdf"));

        let noop = Job::Noop { label: "l".to_string() };
        assert_eq!(owner_of(&noop), None);
    }

    /// The side-channel key encoding is shared with operator
    /// tooling and the 24h-TTL records already in Redis; a silent
    /// prefix change would orphan every live status hash.
    #[test]
    fn status_key_is_job_prefixed() {
        assert_eq!(status_key("abc123"), "job:abc123");
    }

    /// JobError's Display strings surface in HTTP 500 bodies via the
    /// jobs route's `map_job_error`, and the From<RedisError> impl
    /// must preserve the underlying detail for triage.
    #[test]
    fn job_error_display_and_redis_conversion() {
        let redis_err =
            fred::error::RedisError::new(fred::error::RedisErrorKind::Unknown, "boom");
        let err: JobError = redis_err.into();
        assert!(
            matches!(&err, JobError::Redis(msg) if msg.contains("boom")),
            "expected Redis variant carrying the detail, got {err:?}",
        );
        assert!(err.to_string().starts_with("redis: "));
        assert_eq!(
            JobError::NotFound("j1".to_string()).to_string(),
            "not found: j1",
        );
        assert_eq!(
            JobError::Serialize("bad".to_string()).to_string(),
            "serialize: bad",
        );
    }

    /// In-memory no-Redis fake — the substitution the JobProducer
    /// trait exists for. Implements only the required methods so the
    /// tests below exercise the trait's *default* `poll`.
    struct FakeProducer {
        statuses: std::sync::Mutex<std::collections::HashMap<JobId, JobStatus>>,
    }

    impl FakeProducer {
        fn new() -> Self {
            Self {
                statuses: std::sync::Mutex::new(std::collections::HashMap::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl JobProducer for FakeProducer {
        async fn enqueue(&self, _payload: Job) -> Result<JobId, JobError> {
            let id = nanoid::nanoid!();
            self.statuses
                .lock()
                .unwrap()
                .insert(id.clone(), JobStatus::Pending);
            Ok(id)
        }
        async fn status(&self, job_id: &str) -> Result<JobStatus, JobError> {
            self.statuses
                .lock()
                .unwrap()
                .get(job_id)
                .cloned()
                .ok_or_else(|| JobError::NotFound(job_id.to_string()))
        }
    }

    /// Pins the documented #85 footgun: the default `poll` returns
    /// `(status, None)` even for a payload that carries an owner,
    /// meaning the route's ownership gate is bypassed unless an impl
    /// overrides `poll`. If this default ever changes, the trait doc
    /// and every fake need revisiting together.
    #[tokio::test]
    async fn job_producer_default_poll_returns_no_owner_even_for_owned_payloads() {
        let fake = FakeProducer::new();
        let job_id = fake
            .enqueue(Job::ImportDocx {
                s3_key: "k".to_string(),
                title: "t".to_string(),
                folder_id: None,
                owner_id: "user-1".to_string(),
            })
            .await
            .unwrap();
        let (status, owner) = fake.poll(&job_id).await.unwrap();
        assert_eq!(status, JobStatus::Pending);
        assert_eq!(owner, None, "default poll must report no owner");
    }

    /// The default `poll` must propagate `NotFound` from `status`
    /// untouched — the jobs route maps it to a 404.
    #[tokio::test]
    async fn job_producer_default_poll_propagates_not_found() {
        let fake = FakeProducer::new();
        let err = fake.poll("missing-id").await.unwrap_err();
        assert!(
            matches!(&err, JobError::NotFound(id) if id == "missing-id"),
            "expected NotFound, got {err:?}",
        );
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Envelope JSON must round-trip regardless of content —
            /// labels/ids are caller-controlled and may carry quotes,
            /// unicode, or control characters.
            #[test]
            fn prop_envelope_roundtrips_arbitrary_content(
                job_id in any::<String>(),
                enqueued_at_ms in any::<u64>(),
                attempt in any::<u32>(),
                owner in proptest::option::of(any::<String>()),
                label in any::<String>(),
            ) {
                let env = JobEnvelope {
                    job_id,
                    enqueued_at_ms,
                    attempt,
                    owner,
                    payload: Job::Noop { label },
                };
                let json = serde_json::to_string(&env).unwrap();
                let parsed: JobEnvelope = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(env, parsed);
            }

            /// For any import payload: owner derivation returns the
            /// owner_id verbatim, the top-level `type` tag matches the
            /// variant, and the payload round-trips through JSON.
            #[test]
            fn prop_import_jobs_expose_owner_and_type_tag(
                s3_key in any::<String>(),
                title in any::<String>(),
                folder_id in proptest::option::of(any::<String>()),
                owner_id in any::<String>(),
                is_pdf in any::<bool>(),
            ) {
                let payload = if is_pdf {
                    Job::ImportPdf {
                        s3_key,
                        title,
                        folder_id,
                        owner_id: owner_id.clone(),
                    }
                } else {
                    Job::ImportDocx {
                        s3_key,
                        title,
                        folder_id,
                        owner_id: owner_id.clone(),
                    }
                };
                prop_assert_eq!(owner_of(&payload), Some(owner_id.as_str()));
                let value = serde_json::to_value(&payload).unwrap();
                let expected_tag = if is_pdf { "importPdf" } else { "importDocx" };
                prop_assert_eq!(
                    value.get("type").and_then(|t| t.as_str()),
                    Some(expected_tag),
                );
                let parsed: Job = serde_json::from_value(value).unwrap();
                prop_assert_eq!(payload, parsed);
            }
        }
    }
}
