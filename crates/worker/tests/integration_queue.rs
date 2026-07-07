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
