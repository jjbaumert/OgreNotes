// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.4 piece B — worker-mode entrypoint integration tests.
//!
//! Exercises the [`worker_mode::run`]-shape consume/execute/finalize
//! path end-to-end against a real Redis. Uses the same `JobQueue`
//! producer side as a future POST /jobs route (M-6.4 piece C) would.
//! Each test uses a uniquely-named stream so concurrent tests on the
//! same Redis don't compete.
//!
//! Gated on `REDIS_URL` so a developer without docker compose can
//! still `cargo test -p ogrenotes-api` and have these skip.

mod common;

use std::sync::Arc;
use std::time::Duration;

use fred::clients::RedisClient;
use fred::prelude::*;
use ogrenotes_api::worker_mode::{execute_and_finalize, WorkerCtx};
use ogrenotes_worker::{Job, JobQueue, JobStatus};
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

macro_rules! require_redis {
    () => {
        match std::env::var("REDIS_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("REDIS_URL not set; skipping worker-mode integration test");
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
    let stream = format!("worker-mode-test:{}:{}", suffix, nanoid::nanoid!(6));
    let _: Result<(), _> = client.del(stream.as_str()).await;
    let _: Result<(), _> = client.del(format!("{stream}:dlq").as_str()).await;
    JobQueue::new(client, stream).await.expect("queue init")
}

/// Drive the consume/execute/finalize loop in isolation by calling
/// the same primitives `worker_mode::consume_loop` uses, with a
/// shutdown flag that the test toggles after the work completes.
async fn drive_one_consumer(
    queue: JobQueue,
    consumer: String,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        let claim = tokio::select! {
            r = queue.consume_next(&consumer, 500) => r,
            _ = shutdown.changed() => continue,
        };
        match claim {
            Ok(Some(claimed)) => {
                // Mirror execute() for the Noop variant — the worker
                // path tests dispatch isolation via the same call.
                let result = match &claimed.envelope.payload {
                    Job::Noop { label } => Ok(Some(format!(r#"{{"label":"{label}"}}"#))),
                    _ => Err("non-noop not exercised here".to_string()),
                };
                match result {
                    Ok(payload) => {
                        queue.ack(&claimed, payload).await.expect("ack");
                    }
                    Err(e) => {
                        let _ = queue.retry_or_dead_letter(&claimed, 0, &e).await;
                    }
                }
            }
            Ok(None) => continue,
            Err(_) => sleep(Duration::from_millis(50)).await,
        }
    }
}

#[tokio::test]
async fn worker_consumes_and_acks_noop_job() {
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "ack").await;

    // Producer side: same call shape POST /jobs will make in piece C.
    let job_id = queue
        .enqueue(Job::Noop {
            label: "from-test".to_string(),
        })
        .await
        .expect("enqueue");

    // Consumer side: drive the worker loop until the job ack'd.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let q = queue.clone();
    let handle = tokio::spawn(drive_one_consumer(q, "test-consumer".to_string(), shutdown_rx));

    // Poll status; the consumer should ack within a second.
    let final_status = timeout(Duration::from_secs(3), async {
        loop {
            let status = queue.status(&job_id).await.expect("status");
            if matches!(status, JobStatus::Succeeded { .. }) {
                return status;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("job did not reach Succeeded within 3s");

    match final_status {
        JobStatus::Succeeded { result_json, .. } => {
            // Payload from the Noop execute path carries the label
            // back so a poller can correlate the work.
            assert_eq!(
                result_json.as_deref(),
                Some(r#"{"label":"from-test"}"#),
                "Succeeded payload should echo the Noop label",
            );
        }
        other => panic!("expected Succeeded, got {other:?}"),
    }

    let _ = shutdown_tx.send(true);
    let _ = timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn worker_dead_letters_after_retry_budget() {
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "dlq").await;

    // Enqueue an ImportDocx variant — the worker_mode::execute path
    // returns Err for these until M-6.5 lands. With max_retries=0 the
    // first failure dead-letters immediately, so the test doesn't
    // need to wait through retry attempts.
    let job_id = queue
        .enqueue(Job::ImportDocx {
            s3_key: "uploads/never.docx".to_string(),
            title: "x".to_string(),
            folder_id: None,
            owner_id: "u1".to_string(),
        })
        .await
        .expect("enqueue");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let q = queue.clone();
    let handle = tokio::spawn(drive_one_consumer(q, "test-consumer-dlq".to_string(), shutdown_rx));

    let final_status = timeout(Duration::from_secs(3), async {
        loop {
            let status = queue.status(&job_id).await.expect("status");
            if matches!(status, JobStatus::Failed { .. }) {
                return status;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("job did not reach Failed within 3s");

    match final_status {
        JobStatus::Failed { error, .. } => {
            assert!(
                error.contains("non-noop"),
                "Failed.error should carry the dispatch's error string, got: {error}",
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    let _ = shutdown_tx.send(true);
    let _ = timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn worker_drains_cleanly_on_shutdown_signal() {
    let url = require_redis!();
    let client = fresh_client(&url).await;
    let queue = fresh_queue(Arc::clone(&client), "drain").await;

    // No jobs enqueued — the consumer should be blocked on the empty
    // stream when the shutdown signal lands. The loop must wake from
    // its block window and exit; if it doesn't, the timeout below
    // catches it.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let q = queue.clone();
    let handle = tokio::spawn(drive_one_consumer(q, "test-consumer-drain".to_string(), shutdown_rx));

    sleep(Duration::from_millis(100)).await;
    shutdown_tx.send(true).expect("shutdown send");

    timeout(Duration::from_secs(2), handle)
        .await
        .expect("consumer did not drain within 2s — shutdown signal not observed")
        .expect("consumer task panicked");
}

// ─── The REAL finalize + reaper path ────────────────────────────
//
// The tests above drive a local `drive_one_consumer` copy of the loop that
// hard-codes `retry_or_dead_letter(.., 0, ..)`. These drive the production
// `worker_mode::execute_and_finalize` directly so the real MAX_RETRIES budget,
// the dead-letter transition, the ack path, and the reaper's reclaim→finalize
// crash-recovery are actually exercised. They use the TestApp harness for a
// real persistence context (WorkerCtx), so they gate on `require_infra!`.

/// Build a `WorkerCtx` from a `TestApp`'s wired repos.
fn worker_ctx(app: &common::TestApp) -> WorkerCtx {
    WorkerCtx::new(
        app.state.doc_repo.clone(),
        app.state.folder_repo.clone(),
        app.state.doc_repo.s3().clone(),
    )
}

#[tokio::test]
async fn execute_and_finalize_retries_to_budget_then_dead_letters() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ctx = worker_ctx(&app);

    let client = fresh_client("redis://127.0.0.1:6379").await;
    let queue = fresh_queue(Arc::clone(&client), "real-retry").await;

    // A job that deterministically fails in `execute`: ImportDocx with
    // folder_id = None is rejected up front (no S3 access needed).
    let job_id = queue
        .enqueue(Job::ImportDocx {
            s3_key: "missing".to_string(),
            title: "doomed".to_string(),
            folder_id: None,
            owner_id: "owner".to_string(),
        })
        .await
        .expect("enqueue");

    // MAX_RETRIES = 3: attempts 0,1,2 retry; attempt 3 dead-letters. Drive
    // the real finalize once per attempt and confirm the attempt counter
    // climbs (proving the budget, not a hard-coded 0).
    for expected_attempt in 0..=3u32 {
        let claimed = loop {
            if let Some(c) = queue.consume_next("c1", 1_000).await.expect("consume") {
                break c;
            }
        };
        assert_eq!(
            claimed.envelope.attempt, expected_attempt,
            "the retry budget must re-enqueue with an incremented attempt"
        );
        execute_and_finalize(&queue, claimed, &ctx).await;
    }

    // After the budget is spent the job is dead-lettered: gone from the main
    // stream, status Failed (written by retry_or_dead_letter's DLQ path).
    let next = queue.consume_next("c1", 500).await.expect("consume");
    assert!(
        next.is_none(),
        "job must be dead-lettered (not re-queued) once the retry budget is exhausted"
    );
    let status = queue.status(&job_id).await.expect("status");
    assert!(
        matches!(status, JobStatus::Failed { .. }),
        "a dead-lettered job's status must be Failed, got {status:?}"
    );
}

#[tokio::test]
async fn reaper_reclaims_orphaned_job_and_finalizes_it() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ctx = worker_ctx(&app);

    let client = fresh_client("redis://127.0.0.1:6379").await;
    let queue = fresh_queue(Arc::clone(&client), "real-reaper").await;

    let job_id = queue
        .enqueue(Job::Noop { label: "orphan".to_string() })
        .await
        .expect("enqueue");

    // A consumer claims the job but "crashes" — it never acks/retries, so the
    // entry sits orphaned in that consumer's pending list.
    let _claimed = queue
        .consume_next("crashed-consumer", 1_000)
        .await
        .expect("consume")
        .expect("the job should be claimable");
    // Deliberately do NOT finalize it.

    // The crashed consumer flipped it to Running on claim but never finished,
    // so it's left in an in-progress state that only the reaper can recover.
    assert!(
        matches!(queue.status(&job_id).await.expect("status"), JobStatus::Running { .. }),
        "a claimed-but-un-finalized job is Running, orphaned in the consumer's PEL"
    );

    // The reaper reclaims stale entries (min_idle_ms = 0 → eligible now) and
    // finalizes them — the crash-recovery path.
    let mut reclaimed = queue.claim_stale("reaper", 0, 10).await.expect("claim_stale");
    assert_eq!(reclaimed.len(), 1, "the reaper must reclaim the one orphaned entry");
    execute_and_finalize(&queue, reclaimed.pop().unwrap(), &ctx).await;

    // Now completed: the reclaimed Noop was acked.
    let status = queue.status(&job_id).await.expect("status");
    assert!(
        matches!(status, JobStatus::Succeeded { .. }),
        "a reclaimed-and-finalized job must reach Succeeded, got {status:?}"
    );
}
