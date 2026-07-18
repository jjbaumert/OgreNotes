// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for [`JobQueue`] against a live Redis. Gated
//! on the `REDIS_URL` env var so the build doesn't fail when
//! infra isn't available. Each test uses a uniquely-named stream
//! so concurrent test runs don't interfere.
//!
//! ```bash
//! REDIS_URL=redis://127.0.0.1:6379 cargo test -p ogrenotes-worker
//! ```

use std::sync::Arc;
use std::time::Duration;

use fred::clients::RedisClient;
use fred::prelude::*;
use ogrenotes_worker::{Job, JobProducer, JobQueue, JobStatus, RetryOutcome};
use tokio::time::sleep;

macro_rules! require_redis {
    () => {
        match std::env::var("REDIS_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("REDIS_URL not set; skipping integration test");
                return;
            }
        }
    };
}

async fn fresh_client(url: &str) -> Arc<RedisClient> {
    let config = fred::types::RedisConfig::from_url(url).expect("parse REDIS_URL");
    let client = RedisClient::new(config, None, None, None);
    client.init().await.expect("connect redis");
    Arc::new(client)
}

async fn fresh_queue(client: Arc<RedisClient>, suffix: &str) -> JobQueue {
    // Distinct stream name per test + nanoid run to avoid
    // collisions across parallel invocations on the same Redis.
    let stream = format!("rag-eval-job-test:{}:{}", suffix, nanoid::nanoid!(6));
    // Cleanup any leftover keys from a prior aborted run.
    let _: Result<(), _> = client.del(stream.as_str()).await;
    let _: Result<(), _> = client.del(format!("{}:dlq", stream).as_str()).await;
    JobQueue::new(client, stream).await.expect("queue construct")
}

#[tokio::test]
async fn enqueue_then_consume_then_ack() {
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "ack").await;

    let job_id = queue
        .enqueue(Job::Noop { label: "hello".to_string() })
        .await
        .expect("enqueue");
    assert!(!job_id.is_empty(), "job id must be non-empty");

    // Status visible immediately as Pending.
    let status = queue.status(&job_id).await.expect("status pending");
    assert!(matches!(status, JobStatus::Pending));

    // Consumer reads the entry.
    let claimed = queue
        .consume_next("test-consumer-1", 1000)
        .await
        .expect("consume_next")
        .expect("got an entry");
    assert_eq!(claimed.envelope.job_id, job_id);
    assert_eq!(claimed.envelope.attempt, 0);
    match &claimed.envelope.payload {
        Job::Noop { label } => assert_eq!(label, "hello"),
        _ => panic!("expected Noop payload"),
    }

    // Status flipped to Running.
    let status = queue.status(&job_id).await.expect("status running");
    assert!(
        matches!(status, JobStatus::Running { ref worker, .. } if worker == "test-consumer-1"),
        "expected Running by test-consumer-1, got {status:?}",
    );

    // Ack with a result body.
    queue
        .ack(&claimed, Some("{\"docId\":\"abc\"}".to_string()))
        .await
        .expect("ack");

    // Status flipped to Succeeded with the result_json.
    let status = queue.status(&job_id).await.expect("status succeeded");
    match status {
        JobStatus::Succeeded { result_json, .. } => {
            assert_eq!(result_json.as_deref(), Some("{\"docId\":\"abc\"}"));
        }
        other => panic!("expected Succeeded, got {other:?}"),
    }

    // Consume again — nothing pending, short block returns None.
    let next = queue
        .consume_next("test-consumer-1", 50)
        .await
        .expect("consume_next post-ack");
    assert!(next.is_none(), "queue should be empty after ack");
}

#[tokio::test]
async fn retry_increments_attempt_then_dead_letters() {
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "retry").await;

    let job_id = queue
        .enqueue(Job::Noop { label: "retry-me".to_string() })
        .await
        .expect("enqueue");

    // First attempt — claimed, fails, retries.
    let claimed = queue
        .consume_next("c", 1000)
        .await
        .unwrap()
        .expect("first claim");
    assert_eq!(claimed.envelope.attempt, 0);
    let outcome = queue
        .retry_or_dead_letter(&claimed, 2, "transient blip")
        .await
        .expect("retry");
    assert_eq!(outcome, RetryOutcome::Retried { attempt: 1 });

    // Job is back in the stream, attempt=1, same job_id.
    let again = queue
        .consume_next("c", 1000)
        .await
        .unwrap()
        .expect("second claim");
    assert_eq!(again.envelope.job_id, job_id);
    assert_eq!(again.envelope.attempt, 1);

    // Second retry — still under the cap (max=2).
    let outcome = queue
        .retry_or_dead_letter(&again, 2, "another blip")
        .await
        .expect("retry2");
    assert_eq!(outcome, RetryOutcome::Retried { attempt: 2 });

    // Third attempt — now at the cap, dead-letters.
    let third = queue.consume_next("c", 1000).await.unwrap().expect("third");
    assert_eq!(third.envelope.attempt, 2);
    let outcome = queue
        .retry_or_dead_letter(&third, 2, "final fail")
        .await
        .expect("retry3");
    assert_eq!(outcome, RetryOutcome::DeadLettered);

    // Status reads Failed.
    let status = queue.status(&job_id).await.expect("status");
    match status {
        JobStatus::Failed { error, .. } => {
            assert_eq!(error, "final fail");
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    // Main stream is drained.
    let none = queue.consume_next("c", 50).await.expect("post-dlq");
    assert!(none.is_none());
}

#[tokio::test]
async fn claim_stale_recovers_unacked_job() {
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "stale").await;

    let job_id = queue
        .enqueue(Job::Noop { label: "abandoned".to_string() })
        .await
        .expect("enqueue");

    // Consumer A reads but does NOT ack — simulates a crashed worker.
    let claimed = queue
        .consume_next("worker-a", 1000)
        .await
        .unwrap()
        .expect("claim");
    assert_eq!(claimed.envelope.job_id, job_id);

    // Wait briefly so the entry has measurable idle time.
    sleep(Duration::from_millis(150)).await;

    // Consumer B sweeps stale entries; should pick up A's entry.
    let recovered = queue
        .claim_stale("worker-b", 100, 10)
        .await
        .expect("claim_stale");
    assert_eq!(recovered.len(), 1, "expected one stale entry");
    assert_eq!(recovered[0].envelope.job_id, job_id);

    // B can ack normally.
    queue.ack(&recovered[0], None).await.expect("ack post-claim");

    let status = queue.status(&job_id).await.expect("status");
    assert!(matches!(status, JobStatus::Succeeded { .. }));
}

#[tokio::test]
async fn job_producer_trait_routes_through_queue() {
    // Verifies the dyn JobProducer surface (what the API server
    // will hold) round-trips a job. Same flow as the first test
    // but through the trait, not the concrete type.
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "trait").await;

    let producer: Arc<dyn JobProducer> = Arc::new(queue.clone());
    let job_id = producer
        .enqueue(Job::Noop { label: "via-trait".to_string() })
        .await
        .expect("enqueue");

    let status = producer.status(&job_id).await.expect("status");
    assert!(matches!(status, JobStatus::Pending));

    let claimed = queue.consume_next("c", 1000).await.unwrap().expect("claim");
    queue.ack(&claimed, None).await.expect("ack");
}

// ─── Ownership / #85 poll boundary ─────────────────────────────

/// Helper: an owned import job. Import{Docx,Pdf} carry owner_id;
/// the poll-time ownership check (GET /jobs/{id}) reads it back.
fn owned_import(owner: &str) -> Job {
    Job::ImportDocx {
        s3_key: "uploads/x.docx".to_string(),
        title: "Doc".to_string(),
        folder_id: None,
        owner_id: owner.to_string(),
    }
}

#[tokio::test]
async fn poll_returns_owner_for_owned_job_and_none_for_noop() {
    // The #85 boundary: GET /jobs/{id} calls poll() and enforces that
    // only the owner sees an owned job. The concrete JobQueue::poll must
    // read the `owner` field enqueue wrote to the side-channel hash — an
    // import job yields Some(owner), a Noop (bearer capability) None.
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "poll-owner").await;

    let owned_id = queue.enqueue(owned_import("alice")).await.expect("enqueue owned");
    let (status, owner) = queue.poll(&owned_id).await.expect("poll owned");
    assert!(matches!(status, JobStatus::Pending));
    assert_eq!(owner.as_deref(), Some("alice"), "owned job must expose its owner");

    let noop_id = queue.enqueue(Job::Noop { label: "n".to_string() }).await.expect("enqueue noop");
    let (_status, owner) = queue.poll(&noop_id).await.expect("poll noop");
    assert_eq!(owner, None, "ownerless job must poll as None");
}

#[tokio::test]
async fn owner_survives_running_and_succeeded_transitions() {
    // write_status re-asserts the owner on every transition. A poll at
    // each lifecycle stage must keep returning the same owner, or the
    // ownership check would flip open/closed mid-job.
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "poll-transitions").await;

    let job_id = queue.enqueue(owned_import("bob")).await.expect("enqueue");

    let claimed = queue.consume_next("w", 1000).await.unwrap().expect("claim");
    let (status, owner) = queue.poll(&job_id).await.expect("poll running");
    assert!(matches!(status, JobStatus::Running { .. }));
    assert_eq!(owner.as_deref(), Some("bob"), "owner preserved through Running");

    queue.ack(&claimed, Some("{\"docId\":\"d1\"}".to_string())).await.expect("ack");
    let (status, owner) = queue.poll(&job_id).await.expect("poll succeeded");
    assert!(matches!(status, JobStatus::Succeeded { .. }));
    assert_eq!(owner.as_deref(), Some("bob"), "owner preserved through Succeeded");
}

#[tokio::test]
async fn owner_survives_the_dead_letter_route() {
    // The failure path clones the owner into the retried envelope and
    // the Failed status. An owned import that exhausts its retries must
    // stay pollable BY ITS OWNER with a Failed status — a dropped owner
    // here would make a failed job either invisible or world-readable.
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "poll-dlq").await;

    let job_id = queue.enqueue(owned_import("carol")).await.expect("enqueue");

    // max_retries = 0 → dead-letters on the first failure.
    let claimed = queue.consume_next("w", 1000).await.unwrap().expect("claim");
    let outcome = queue
        .retry_or_dead_letter(&claimed, 0, "boom")
        .await
        .expect("dead-letter");
    assert_eq!(outcome, RetryOutcome::DeadLettered);

    let (status, owner) = queue.poll(&job_id).await.expect("poll failed");
    match status {
        JobStatus::Failed { error, .. } => assert_eq!(error, "boom"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(owner.as_deref(), Some("carol"), "owner preserved through dead-letter");
}

// ─── Dead-letter durability + poison-entry safety ──────────────

#[tokio::test]
async fn dead_lettered_entry_lands_on_the_dlq_stream() {
    // On retry exhaustion the code XDELs the main-stream entry and XADDs
    // the envelope + lastError to <stream>:dlq — the operator triage /
    // XCLAIM-back path. If that XADD regressed, dead-lettered jobs would
    // vanish silently (the main-stream delete still runs). Assert the DLQ
    // entry exists with the job_id and the error string.
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "dlq-durable").await;
    let stream = queue.stream_name().to_string();
    let dlq = format!("{stream}:dlq");

    let job_id = queue.enqueue(owned_import("dave")).await.expect("enqueue");
    let claimed = queue.consume_next("w", 1000).await.unwrap().expect("claim");
    queue.retry_or_dead_letter(&claimed, 0, "fatal").await.expect("dlq");

    // One entry on the DLQ stream, carrying the envelope + lastError.
    let len: u64 = client.xlen(dlq.as_str()).await.expect("xlen dlq");
    assert_eq!(len, 1, "exactly one dead-lettered entry");

    let entries: Vec<(String, std::collections::HashMap<String, String>)> = client
        .xrange(dlq.as_str(), "-", "+", None)
        .await
        .expect("xrange dlq");
    assert_eq!(entries.len(), 1);
    let fields = &entries[0].1;
    let envelope = fields.get("envelope").expect("dlq entry has envelope field");
    assert!(envelope.contains(&job_id), "DLQ envelope carries the job_id");
    assert_eq!(
        fields.get("lastError").map(String::as_str),
        Some("fatal"),
        "DLQ entry records the last error"
    );
}

#[tokio::test]
async fn poison_stream_entry_surfaces_as_error_not_panic() {
    // A stream entry whose `envelope` field is not valid JobEnvelope JSON
    // (wire skew after a deploy, or a hand-XADDed poison message) must
    // surface as a JobError, never panic the consumer loop. Exercises the
    // serde_json::from_str error branch in consume_next.
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "poison").await;
    let stream = queue.stream_name().to_string();

    // Hand-XADD a garbage envelope directly onto the stream.
    let _: String = client
        .xadd(
            stream.as_str(),
            false,
            None,
            "*",
            vec![("envelope", "{ not valid json")],
        )
        .await
        .expect("xadd poison");

    let result = queue.consume_next("w", 1000).await;
    assert!(result.is_err(), "poison entry must error, got {result:?}");

    // A stream entry missing the `envelope` field entirely also errors.
    let queue2 = fresh_queue(Arc::clone(&client), "poison-missing").await;
    let stream2 = queue2.stream_name().to_string();
    let _: String = client
        .xadd(stream2.as_str(), false, None, "*", vec![("wrongfield", "x")])
        .await
        .expect("xadd missing-field");
    assert!(
        queue2.consume_next("w", 1000).await.is_err(),
        "entry missing the envelope field must error"
    );
}
