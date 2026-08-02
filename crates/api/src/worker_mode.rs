// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.4 piece B — worker-mode entrypoint.
//!
//! Same Docker image as the API server, different argv: when the
//! binary is launched with `--mode=worker`, [`run`] takes over instead
//! of the HTTP server path. Spawns [`AppConfig::worker_concurrency`]
//! consumers against the configured Redis stream and one dedicated
//! reaper task that XAUTOCLAIMs entries left orphaned by a crashed
//! peer.
//!
//! Wire shape mirrors `crates/worker` exactly — this module is the
//! glue that turns those primitives into a long-lived process:
//!
//! ```text
//! ┌──────────────┐ XADD            XREADGROUP ┌──────────────┐
//! │  API server  │ ─────► Redis ◄────────────│  worker mode │
//! │  POST /jobs  │       stream    XACK/XDEL │  this module │
//! └──────────────┘                            └──────────────┘
//! ```
//!
//! Operationally:
//!
//! - Graceful shutdown: SIGTERM (ECS task stop) or SIGINT flips the
//!   shared watch channel; consumers wake from their block window
//!   within [`CONSUME_BLOCK_MS`] and exit. A doubled deadline acts
//!   as a hard cap if a consumer's `execute` is stuck.
//! - Reaper: only one task runs XAUTOCLAIM — running it from every
//!   consumer would compete with itself and thrash the pending list.
//! - Per-job dispatch lives in [`execute`]; DOCX (M-6.5) and PDF
//!   (M-6.6) variants currently dead-letter with a TODO message so
//!   the queue still drains cleanly during M-6.4 rollout.

use std::sync::Arc;
use std::time::Duration;

use fred::clients::RedisClient;
use fred::prelude::*;
use futures_util::future::FutureExt;
use ogrenotes_common::config::AppConfig;
use ogrenotes_quip_import::{QuipClient, QuipError, QuipThread, QuipToken, TokenStore, walk_inventory};
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::models::import::ImportStatus;
use ogrenotes_storage::models::import_inventory::{FolderRow, ReportNote, ThreadRow, ThreadState};
use ogrenotes_storage::models::DocType;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
use ogrenotes_storage::repo::import_repo::ImportRepo;
use ogrenotes_storage::repo::user_repo::UserRepo;
use ogrenotes_storage::s3::S3Client;
use ogrenotes_worker::{ClaimedJob, Job, JobQueue, RetryOutcome};
use tokio::sync::watch;

/// Handles the import jobs need to persist a document: S3 to fetch the
/// uploaded blob, plus the doc/folder/user repos to write it. Built
/// once in [`run`] and shared (read-only) across every consumer +
/// reaper task. The worker deliberately does *not* index the new doc
/// into Tantivy or Qdrant — keyword search runs against the API
/// instance's local index, which is a separate process, so an
/// imported doc becomes searchable only after it's opened/edited (the
/// API reindexes on its own writes). Documented as a v1 limitation.
/// Persistence context shared by the consume + reaper loops. `pub` with a
/// `pub` constructor so integration tests can build one from a `TestApp`'s
/// repos and drive [`execute_and_finalize`] (and the reaper's `claim_stale`
/// path) directly, instead of reimplementing the loop.
pub struct WorkerCtx {
    doc_repo: Arc<DocRepo>,
    folder_repo: Arc<FolderRepo>,
    s3: S3Client,
    /// Quip import manifest repo — the durable, token-free checkpoint the
    /// inventory handler reads scope from and writes FOLDER#/THREAD# rows to.
    import_repo: Arc<ImportRepo>,
    /// Read-only here, and used for exactly one thing: turning a Quip
    /// person's email into an OgreNotes user id so an imported `@mention`
    /// points at a real account ([`PersonDirectory`]). Nothing on this path
    /// writes a user, and no email it reads is ever logged or stored.
    user_repo: Arc<UserRepo>,
    /// Where the per-import Quip token lives (SSM in prod, in-process in
    /// dev). The `StartQuipImport` trigger is token-free; the handler
    /// re-reads the token from here and never logs it.
    quip_token_store: Arc<dyn TokenStore>,
    /// Base URL override for the per-import Quip client. `None` in prod
    /// (real `platform.quip.com`); a wiremock URI in integration tests.
    quip_base: Option<String>,
}

impl WorkerCtx {
    pub fn new(
        doc_repo: Arc<DocRepo>,
        folder_repo: Arc<FolderRepo>,
        s3: S3Client,
        import_repo: Arc<ImportRepo>,
        user_repo: Arc<UserRepo>,
        quip_token_store: Arc<dyn TokenStore>,
        quip_base: Option<String>,
    ) -> Self {
        Self { doc_repo, folder_repo, s3, import_repo, user_repo, quip_token_store, quip_base }
    }
}

/// What [`execute`] decided about a claimed entry, over and above the
/// `Result`'s success/failure axis.
///
/// The queue offers exactly two finalizations — ack (success) and
/// retry-or-dead-letter (failure) — and a *multi-hour* job needs a third.
/// The reaper reclaims any entry idle for [`REAPER_MIN_IDLE_MS`] (60s), so
/// every real Quip content pass gets redelivered while its original consumer
/// is still working. The redelivered run must be able to say "not mine, not
/// finished, not broken" without the entry being acked out from under the
/// worker that *is* doing the work.
#[derive(Debug)]
pub enum JobDisposition {
    /// The work is finished. `result_json` is the ack payload handed to
    /// pollers via `GET /jobs/{id}`.
    Done(Option<String>),
    /// Another **live** runner owns this unit of work and is still on it.
    ///
    /// Neither success nor failure: the entry is left **pending** in the
    /// consumer group (no XACK, no retry XADD), so
    ///
    /// - the retry budget is untouched — "someone else has it" must never
    ///   push a healthy import toward the dead-letter queue; and
    /// - if the original worker then dies (deploy, SIGKILL, OOM) the entry
    ///   is still there to be reclaimed. Its DynamoDB lease goes stale after
    ///   [`CLAIM_STALE_MS`] (30s), which is below the reaper's 60s idle
    ///   threshold, so the *next* redelivery reclaims it and resumes from
    ///   the per-thread `ContentDone` checkpoints.
    ///
    /// The cost is a bounded re-execution: the reaper re-claims the entry
    /// roughly once a minute for as long as the work runs, and each of those
    /// runs is one conditional DynamoDB update (`claim_runner`) before it
    /// returns here. That is not a storm — it is the same order of magnitude
    /// as the lease heartbeat it is checking.
    HeldByLiveRunner,
}

/// Retry budget shared across all job kinds in v1. Per-kind overrides
/// could land in [`execute_and_finalize`] when DOCX / PDF arrive in
/// M-6.5 / M-6.6, but a flat default keeps the v1 loop honest.
const MAX_RETRIES: u32 = 3;

/// Block window for `XREADGROUP` in milliseconds. Each consumer parks
/// up to this long when the stream is empty; on shutdown the loop
/// observes the watch channel on every wake.
const CONSUME_BLOCK_MS: u64 = 5_000;

/// Reaper cadence in seconds. Doesn't need to be aggressive — the
/// only thing it catches is a worker that crashed mid-job. Lower
/// values cost more Redis calls; higher values delay recovery.
const REAPER_INTERVAL_SECS: u64 = 30;

/// Minimum idle time (ms) before the reaper takes over an entry.
/// 60s gives a normal worker plenty of room to finish a job before
/// being treated as crashed; XAUTOCLAIM only moves entries past this
/// threshold.
const REAPER_MIN_IDLE_MS: u64 = 60_000;

/// Inventory-lease staleness cutoff (ms). Deliberately kept BELOW
/// [`REAPER_MIN_IDLE_MS`] (60s) so a crashed worker's DynamoDB lease looks
/// stale by the time the Redis reaper redelivers the orphaned entry (~60s):
/// the redelivered handler then finds the ~60s-old lease past this 30s
/// cutoff → `claim_runner` returns `Ok(true)` → it reclaims and resumes. If
/// this sat above the reaper interval, a redelivered crashed job would see
/// the dead worker's lease as still-live, no-op, get acked, and the import
/// would strand — defeating crash-resumability. A live long-running walk
/// heartbeats (folder→meta boundary and per thread-meta chunk) to keep its
/// lease fresh; a rare stolen-live lease only causes a harmless double-run
/// (`put_thread` is insert-if-absent), so the lease is an optimization, not
/// a correctness gate.
const CLAIM_STALE_MS: i64 = 30_000;

/// Thread-metadata fetch batch size for `/1/threads/`. Quip accepts many
/// ids per call; 100 keeps each request comfortably under URL limits.
const THREAD_META_CHUNK: usize = 100;

/// Entrypoint. Runs until SIGTERM / SIGINT lands, then drains.
pub async fn run(config: AppConfig) {
    tracing::info!(
        stream = %config.job_stream_name,
        concurrency = config.worker_concurrency,
        "worker mode: starting",
    );

    let redis_config = fred::types::RedisConfig::from_url(&config.redis_url)
        .expect("invalid REDIS_URL");
    let client = RedisClient::new(redis_config, None, None, None);
    client.connect();
    client
        .wait_for_connect()
        .await
        .expect("worker requires Redis; connect failed");
    tracing::info!("worker mode: redis connected");

    let queue = JobQueue::new(Arc::new(client), config.job_stream_name.clone())
        .await
        .expect("worker mode: queue init failed");

    // Build the persistence context. Same AWS client construction the
    // server-mode path in `main` uses — the worker process never runs
    // both, so each builds its own clients.
    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(config.aws_region.clone()))
        .load()
        .await;
    let dynamo = DynamoClient::new(aws_sdk_dynamodb::Client::new(&aws_config), config.table_name());
    let s3 = S3Client::new(aws_sdk_s3::Client::new(&aws_config), config.s3_bucket.clone());

    // Quip import deps. Build `import_repo` from a clone BEFORE `FolderRepo`
    // consumes `dynamo` below. The token store selection mirrors
    // `main.rs`/`AppState::new` exactly: in-process in dev (no SSM in the
    // local stack), SSM SecureString in prod. `quip_base = None` → the
    // handler builds a per-import client against real platform.quip.com.
    let import_repo = Arc::new(ImportRepo::new(dynamo.clone()));
    let quip_token_store: Arc<dyn TokenStore> = if config.dev_mode {
        Arc::new(ogrenotes_quip_import::InMemoryTokenStore::new())
    } else {
        Arc::new(ogrenotes_quip_import::SsmTokenStore::new(
            aws_sdk_ssm::Client::new(&aws_config),
            format!("/{}ogrenote/", config.dynamodb_table_prefix),
        ))
    };

    let user_repo = Arc::new(UserRepo::new(dynamo.clone()));
    let ctx = Arc::new(WorkerCtx::new(
        Arc::new(DocRepo::new(dynamo.clone(), s3.clone())),
        Arc::new(FolderRepo::new(dynamo)),
        s3,
        import_repo,
        user_repo,
        quip_token_store,
        None,
    ));
    tracing::info!("worker mode: persistence context ready");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handles = spawn_workers(queue, ctx, config.worker_concurrency, shutdown_rx);

    await_shutdown_signal().await;
    tracing::info!("worker mode: shutdown signal received, draining");
    let _ = shutdown_tx.send(true);

    let drain_deadline = Duration::from_millis(CONSUME_BLOCK_MS * 2);
    if tokio::time::timeout(drain_deadline, futures_util::future::join_all(handles))
        .await
        .is_err()
    {
        tracing::warn!("worker mode: drain timeout exceeded; some tasks still running");
    }
    tracing::info!("worker mode: stopped");
}

/// Spawn `concurrency` consumer tasks plus one reaper against `queue`, all
/// sharing `ctx` and observing `shutdown_rx`. Returns the join handles so the
/// caller can await a graceful drain. Factored out of [`run`] so the API server
/// can host an *embedded* worker in dev mode (see `main.rs`) — sharing the same
/// `AppState` components, crucially the same in-process `TokenStore`, so a
/// single-process `cargo run` fully processes Quip jobs without a separate
/// worker. The deployed `--mode=worker` path calls this unchanged.
pub fn spawn_workers(
    queue: JobQueue,
    ctx: Arc<WorkerCtx>,
    concurrency: u32,
    shutdown_rx: watch::Receiver<bool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let consumer_prefix = consumer_prefix();
    let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    for i in 0..concurrency.max(1) {
        let q = queue.clone();
        let consumer = format!("{consumer_prefix}-{i}");
        let rx = shutdown_rx.clone();
        handles.push(tokio::spawn(consume_loop(q, consumer, rx, Arc::clone(&ctx))));
    }
    {
        let q = queue.clone();
        let consumer = format!("{consumer_prefix}-reaper");
        let rx = shutdown_rx.clone();
        handles.push(tokio::spawn(reaper_loop(q, consumer, rx, Arc::clone(&ctx))));
    }
    handles
}

/// Consume one entry at a time, execute it, finalize. The
/// `tokio::select!` against the shutdown receiver guarantees the
/// loop wakes from its block window on shutdown rather than waiting
/// for the full [`CONSUME_BLOCK_MS`].
async fn consume_loop(
    queue: JobQueue,
    consumer: String,
    mut shutdown: watch::Receiver<bool>,
    ctx: Arc<WorkerCtx>,
) {
    tracing::info!(consumer, "worker mode: consumer started");
    loop {
        if *shutdown.borrow() {
            tracing::info!(consumer, "worker mode: consumer exiting");
            return;
        }
        let claim_result = tokio::select! {
            r = queue.consume_next(&consumer, CONSUME_BLOCK_MS) => r,
            _ = shutdown.changed() => continue,
        };
        match claim_result {
            Ok(Some(claimed)) => execute_and_finalize(&queue, claimed, &ctx).await,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(
                    consumer,
                    error = %e,
                    "consume_next failed; backing off",
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn reaper_loop(
    queue: JobQueue,
    consumer: String,
    mut shutdown: watch::Receiver<bool>,
    ctx: Arc<WorkerCtx>,
) {
    tracing::info!(consumer, "worker mode: reaper started");
    let mut tick = tokio::time::interval(Duration::from_secs(REAPER_INTERVAL_SECS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // First tick fires immediately; skip it so the first reap waits
    // a full interval after startup.
    tick.tick().await;
    loop {
        tokio::select! {
            _ = tick.tick() => {},
            _ = shutdown.changed() => {
                tracing::info!(consumer, "worker mode: reaper exiting");
                return;
            }
        }
        if *shutdown.borrow() {
            return;
        }
        match queue.claim_stale(&consumer, REAPER_MIN_IDLE_MS, 16).await {
            Ok(entries) if entries.is_empty() => {}
            Ok(entries) => {
                tracing::info!(
                    consumer,
                    count = entries.len(),
                    "worker mode: reaper claimed stale entries",
                );
                for claimed in entries {
                    execute_and_finalize(&queue, claimed, &ctx).await;
                }
            }
            Err(e) => {
                tracing::warn!(consumer, error = %e, "claim_stale failed");
            }
        }
    }
}

/// Best-effort human-readable text for a caught panic payload.
///
/// Only ever reaches a **log**, never a durable `reason` / `ReportNote`: a
/// panic message is arbitrary runtime data — a slice of the document being
/// parsed, at worst — and the durable strings are user-visible. See
/// [`ThreadImportError::transient`] for how the per-thread path keeps it out.
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

/// Execute one claimed job and finalize it: ack on success (then drop the
/// staging blob), or retry / dead-letter on failure per [`MAX_RETRIES`].
/// `pub` so integration tests can drive the real retry budget and the
/// reaper's reclaim→finalize path directly.
pub async fn execute_and_finalize(queue: &JobQueue, claimed: ClaimedJob, ctx: &WorkerCtx) {
    let job_id = claimed.envelope.job_id.clone();
    let attempt = claimed.envelope.attempt;
    // Isolate the handler behind catch_unwind: a panic in any job kind
    // (e.g. a malformed-PDF panic in a parser dependency — see the
    // `catch_unwind` in import_pdf.rs for why this class is real) must
    // NOT unwind out of the consumer/reaper loop. Losing the reaper in
    // particular disables stale-job recovery for the entire worker fleet,
    // not just the poison job. A panic is treated as an ordinary job
    // failure so the retry/dead-letter machinery still applies.
    let result = match std::panic::AssertUnwindSafe(execute(ctx, &claimed.envelope.payload))
        .catch_unwind()
        .await
    {
        Ok(r) => r,
        Err(panic) => {
            let msg = panic_message(&panic);
            tracing::error!(job_id, attempt, panic = %msg, "job execution panicked");
            Err(format!("job execution panicked: {msg}"))
        }
    };
    match result {
        // Deliberately NOT an ack: see [`JobDisposition::HeldByLiveRunner`].
        // Acking here would delete the only durable record that this work is
        // outstanding, while the work is still in flight — so the original
        // worker's death would strand the import with nothing to retry it.
        Ok(JobDisposition::HeldByLiveRunner) => {
            tracing::info!(
                job_id,
                attempt,
                "job is held by a live runner; leaving the entry pending for redelivery",
            );
        }
        Ok(JobDisposition::Done(payload)) => match queue.ack(&claimed, payload).await {
            Ok(()) => {
                tracing::info!(job_id, attempt, "job succeeded");
                // Terminal success: the staging upload is no longer
                // needed. Delete after the ack, never before — a
                // pre-ack delete would strand a retry.
                cleanup_staging_blob(ctx, &claimed.envelope.payload).await;
            }
            Err(e) => tracing::warn!(job_id, error = %e, "ack failed; entry orphaned"),
        },
        Err(reason) => {
            tracing::warn!(job_id, attempt, error = %reason, "job failed");
            match queue
                .retry_or_dead_letter(&claimed, MAX_RETRIES, &reason)
                .await
            {
                Ok(RetryOutcome::Retried { attempt }) => {
                    tracing::info!(job_id, attempt, "job retried");
                }
                Ok(RetryOutcome::DeadLettered) => {
                    tracing::warn!(job_id, "job dead-lettered");
                    // Terminal failure: drop the staging upload too —
                    // no future attempt will read it.
                    cleanup_staging_blob(ctx, &claimed.envelope.payload).await;
                    // For a Quip import the queue's dead-letter is invisible to
                    // the frontend, which polls the ImportRecord: without a
                    // terminal status the record stays Running/phase 0 and the
                    // wizard hangs on "Scanning…" forever. Flip it to Failed so
                    // the poll loop stops. Only on DeadLettered, never on Retried.
                    mark_import_dead_lettered(ctx, &claimed.envelope.payload).await;
                }
                Err(e) => {
                    tracing::error!(
                        job_id,
                        error = %e,
                        "retry/dlq write failed; entry orphaned",
                    );
                }
            }
        }
    }
}

/// Map a [`Job`] payload to its handler. New variants land here.
async fn execute(ctx: &WorkerCtx, payload: &Job) -> Result<JobDisposition, String> {
    match payload {
        Job::Noop { label } => {
            tracing::info!(label, "noop executed");
            Ok(JobDisposition::Done(Some(
                serde_json::json!({ "label": label }).to_string(),
            )))
        }
        Job::ImportDocx {
            s3_key,
            title,
            folder_id,
            owner_id,
        } => {
            let doc_id = execute_import_docx(
                &ctx.doc_repo,
                &ctx.folder_repo,
                &ctx.s3,
                s3_key,
                title,
                folder_id.as_deref(),
                owner_id,
            )
            .await?;
            tracing::info!(doc_id, owner_id, "docx imported");
            Ok(JobDisposition::Done(Some(
                serde_json::json!({ "docId": doc_id }).to_string(),
            )))
        }
        #[cfg(feature = "pdf")]
        Job::ImportPdf {
            s3_key,
            title,
            folder_id,
            owner_id,
        } => {
            let doc_id = execute_import_pdf(
                &ctx.doc_repo,
                &ctx.folder_repo,
                &ctx.s3,
                s3_key,
                title,
                folder_id.as_deref(),
                owner_id,
            )
            .await?;
            tracing::info!(doc_id, owner_id, "pdf imported");
            Ok(JobDisposition::Done(Some(
                serde_json::json!({ "docId": doc_id }).to_string(),
            )))
        }
        #[cfg(not(feature = "pdf"))]
        Job::ImportPdf { .. } => Err("PDF import not compiled into this build".into()),
        Job::StartQuipImport { import_id, owner_id } => {
            match execute_start_quip_import(ctx, import_id, owner_id).await? {
                ImportRunOutcome::Ran => Ok(JobDisposition::Done(Some(
                    serde_json::json!({ "importId": import_id }).to_string(),
                ))),
                // Not ours to ack — the lease-holder is still working. See
                // [`JobDisposition::HeldByLiveRunner`].
                ImportRunOutcome::HeldByLiveRunner => Ok(JobDisposition::HeldByLiveRunner),
            }
        }
    }
}

/// Best-effort delete of an import job's S3 staging blob once the job
/// reaches a terminal state (succeeded or dead-lettered). Skipped for
/// non-import jobs. A missing key or SDK error is logged, not
/// propagated — cleanup failure must never re-fail an already-finished
/// job. Covers the future PDF path at no extra cost.
async fn cleanup_staging_blob(ctx: &WorkerCtx, payload: &Job) {
    let s3_key = match payload {
        Job::ImportDocx { s3_key, .. } | Job::ImportPdf { s3_key, .. } => s3_key.as_str(),
        // A Quip import DOES stage in S3 — one raw-HTML object per thread
        // under [`quip_staging_prefix`], written by `import_one_thread`
        // since Phase 2a. It is deliberately not swept from here, because
        // this hook is *job*-terminal and the delete has to be
        // *import*-terminal: an acked `StartQuipImport` is not necessarily a
        // finished import (a `TokenRejected` run acks too, and that import
        // resumes the moment the user reconnects), and a dead-lettered one
        // is finalized by [`mark_import_dead_lettered`]. The sweep therefore
        // hangs off the terminal-status writes themselves — see
        // [`cleanup_quip_staging`] for the full list of hooks.
        Job::StartQuipImport { .. } | Job::Noop { .. } => return,
    };
    if let Err(e) = ctx.s3.delete_object(s3_key).await {
        tracing::warn!(s3_key, error = %e, "failed to delete import staging blob");
    }
}

/// The S3 prefix a Quip import stages its per-thread raw HTML under.
///
/// One object per thread lives directly beneath it
/// (`imports/{import_id}/threads/{quip_thread_id}.html`), written by
/// [`import_one_thread`] and swept by [`cleanup_quip_staging`]. Both spell it
/// through this function so the writer and the deleter cannot drift apart —
/// a deleter aimed at a prefix the writer no longer uses is a silent leak of
/// the user's full document text, which is what issue #196 was.
///
/// **The trailing `/` is load-bearing.** It is what confines a delete to one
/// import: without it, `imports/abc` would also match `imports/abcdef/...`,
/// i.e. a *different* import's staged documents. It also excludes the DOCX /
/// PDF staging that shares the `imports/` root but has a different shape
/// (`imports/{user_id}/{id}.{ext}` — those are single objects owned by their
/// own job, cleaned by [`cleanup_staging_blob`]).
fn quip_staging_prefix(import_id: &str) -> String {
    format!("imports/{import_id}/threads/")
}

/// Drop an import's staged raw Quip HTML. **Call only once the import itself
/// is terminal** — `Succeeded`, or `Failed` (including the dead-letter).
///
/// The staged objects are the user's full document text (issue #196: a real
/// import staged brokerage statements and named correspondence), so retaining
/// them past the run that needed them is a data-retention defect on its own —
/// deleting the imported OgreNotes documents does not reach these, because
/// they are keyed by import id rather than doc id.
///
/// **Import-terminal, not job-terminal.** A job retry, a
/// [`JobDisposition::HeldByLiveRunner`] redelivery, a mid-pass transient
/// failure and a `TokenRejected` import all leave the import re-runnable, and
/// none of them may reach this function: while an import can still run again,
/// its staging is still the in-flight run's diagnostic material.
///
/// **Advisory by construction: this returns nothing, so it cannot enter the
/// import's control flow** — the same discipline [`record_report`] follows. An
/// import that succeeded must never be reported as failed because its cleanup
/// could not reach S3; the failure is logged, and the `imports/` lifecycle
/// rule (see `infra/lib/data.ts`) is the backstop that catches what a lost
/// delete leaves behind.
async fn cleanup_quip_staging(ctx: &WorkerCtx, import_id: &str) {
    let prefix = quip_staging_prefix(import_id);
    match ctx.s3.delete_prefix(&prefix).await {
        Ok(()) => tracing::info!(
            import_id,
            prefix = %prefix,
            "quip import: staged thread HTML deleted (import is terminal)",
        ),
        Err(e) => tracing::warn!(
            import_id,
            prefix = %prefix,
            error = %e,
            "quip import: deleting the staged thread HTML failed; the import's outcome is \
             unchanged and the bucket lifecycle rule remains the backstop",
        ),
    }
}

/// Best-effort terminal-status write for a dead-lettered import job. The
/// job queue's dead-letter is invisible to the frontend, which polls the
/// `ImportRecord`; a sustained-failure Quip inventory job that exhausts its
/// retry budget would otherwise leave the record in `Running`/phase 0 and hang
/// the wizard on "Scanning…" forever. Flipping the status to `Failed` gives the
/// poll loop a terminal state to stop on (the frontend already treats
/// `"failed"` as terminal). Dispatches on the payload exactly like
/// [`cleanup_staging_blob`]; only the Quip variant has an `ImportRecord` to
/// finalize. A write error is logged, not propagated — this runs after the job
/// is already terminal in the queue.
async fn mark_import_dead_lettered(ctx: &WorkerCtx, payload: &Job) {
    match payload {
        Job::StartQuipImport { import_id, .. } => {
            if let Err(e) = ctx.import_repo.set_status(import_id, ImportStatus::Failed).await {
                tracing::warn!(import_id, error = %e, "failed to mark dead-lettered import Failed");
            }
            // Import-terminal: the queue has exhausted the retry budget, so no
            // future run will read this import's staging. Unconditional on the
            // status write above — a dead-lettered job is terminal whether or
            // not DynamoDB accepted the record of it, and the staged content
            // must not outlive the run either way.
            cleanup_quip_staging(ctx, import_id).await;
        }
        Job::ImportDocx { .. } | Job::ImportPdf { .. } | Job::Noop { .. } => {}
    }
}

/// Run a DOCX import to completion: fetch the staged blob from S3,
/// parse it, and persist a new document, returning its id. The
/// worker's `ImportDocx` arm is a thin wrapper over this; it's public
/// so integration tests can drive the import path without standing up
/// a full consumer loop, and so the PDF import (M-6.6) can share the
/// persist tail.
///
/// `folder_id` must be `Some` — the import-job route resolves and
/// authorizes the destination before enqueuing. A `None` means the job
/// bypassed that authorized path, so we reject rather than invent a
/// destination: the worker has no auth context to fall back on, and
/// inventing one would be authorization-after-the-fact.
pub async fn execute_import_docx(
    doc_repo: &DocRepo,
    folder_repo: &FolderRepo,
    s3: &S3Client,
    s3_key: &str,
    title: &str,
    folder_id: Option<&str>,
    owner_id: &str,
) -> Result<String, String> {
    let folder = folder_id.ok_or_else(|| {
        format!(
            "ImportDocx job for owner {owner_id} has no folder_id; \
             its destination was never authorized — rejecting"
        )
    })?;
    let bytes = s3
        .get_object(s3_key)
        .await
        .map_err(|e| format!("fetch {s3_key}: {e}"))?;
    let doc = ogrenotes_collab::import_docx::from_docx(&bytes)
        .map_err(|e| format!("parse docx: {e}"))?;
    let snapshot = ogrenotes_collab::snapshot::doc_to_bytes(&doc);
    let now = ogrenotes_common::time::now_usec();
    persist_imported_document(
        doc_repo,
        folder_repo,
        &snapshot,
        title,
        owner_id,
        folder,
        DocType::Document,
        &[],
        now,
        now,
        &ogrenotes_common::id::new_id(),
        OnExistingDoc::Reject,
    )
    .await
}

/// PDF counterpart of [`execute_import_docx`] (M-6.6). Same fetch →
/// parse → persist shape, with `import_pdf::from_pdf` — which already
/// wraps the panic-prone `pdf-extract` in `catch_unwind`, so a
/// malformed PDF surfaces as a dead-lettered job, not a worker crash.
/// Public so the round-trip integration test can drive it directly.
#[cfg(feature = "pdf")]
pub async fn execute_import_pdf(
    doc_repo: &DocRepo,
    folder_repo: &FolderRepo,
    s3: &S3Client,
    s3_key: &str,
    title: &str,
    folder_id: Option<&str>,
    owner_id: &str,
) -> Result<String, String> {
    let folder = folder_id.ok_or_else(|| {
        format!(
            "ImportPdf job for owner {owner_id} has no folder_id; \
             its destination was never authorized — rejecting"
        )
    })?;
    let bytes = s3
        .get_object(s3_key)
        .await
        .map_err(|e| format!("fetch {s3_key}: {e}"))?;
    let doc = ogrenotes_collab::import_pdf::from_pdf(&bytes)
        .map_err(|e| format!("parse pdf: {e}"))?;
    let snapshot = ogrenotes_collab::snapshot::doc_to_bytes(&doc);
    let now = ogrenotes_common::time::now_usec();
    persist_imported_document(
        doc_repo,
        folder_repo,
        &snapshot,
        title,
        owner_id,
        folder,
        DocType::Document,
        &[],
        now,
        now,
        &ogrenotes_common::id::new_id(),
        OnExistingDoc::Reject,
    )
    .await
}

/// What a document already existing under the caller's `doc_id` means to
/// [`persist_imported_document`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnExistingDoc {
    /// The id was minted immediately before the call and handed to nobody,
    /// so a collision is a nanoid collision or a caller bug — fail loudly.
    /// DOCX and PDF imports.
    Reject,
    /// The id was **reserved** on durable state before the first create
    /// attempt (the Quip content pass writes it to the `THREAD#` row), so a
    /// document already sitting there is this thread's own earlier attempt.
    /// Adopt it and carry on — minting a fresh id instead is precisely how a
    /// single DynamoDB throttle at step 8/9/10 used to hand the user two
    /// copies of one Quip document.
    ReconcileReservedId,
}

/// Persist a freshly-parsed import as a new document: write the v=1
/// snapshot via the doc repo, then link it into its folder(s). Mirrors
/// the synchronous `routes::documents::create_from_text` doc-creation
/// shape; the PDF import (M-6.6) and the Quip content pass share it.
///
/// The caller mints `doc_id` rather than receiving one back, because the
/// Quip content pass has to know the id *before* it persists: the S3 keys
/// of the images it side-loads are `blobs/{doc_id}/...`, and those keys are
/// baked into the snapshot's `Image.src` values. DOCX/PDF just pass a fresh
/// [`new_id`](ogrenotes_common::id::new_id).
///
/// `created_at` / `updated_at` are caller-supplied so an import can preserve
/// the *source* system's timestamps — a Quip doc last edited in 2019 must not
/// come across looking like it was written today. DOCX/PDF pass `now_usec()`
/// twice, which is what they did when the timestamps were hardcoded here.
///
/// `additional_folder_ids` records multi-folder membership on the document
/// metadata; note that field alone does **not** create the folder→child
/// rows, so this links `folder_id` *and* every additional folder via
/// `add_child`.
///
/// `on_existing` decides what a *pre-existing* document under `doc_id`
/// means — see [`OnExistingDoc`].
#[allow(clippy::too_many_arguments)]
async fn persist_imported_document(
    doc_repo: &DocRepo,
    folder_repo: &FolderRepo,
    snapshot: &[u8],
    title: &str,
    owner_id: &str,
    folder_id: &str,
    doc_type: DocType,
    additional_folder_ids: &[String],
    created_at: i64,
    updated_at: i64,
    doc_id: &str,
    on_existing: OnExistingDoc,
) -> Result<String, String> {
    use ogrenotes_storage::models::document::DocumentMeta;
    use ogrenotes_storage::models::folder::FolderChild;
    use ogrenotes_storage::models::ChildType;

    let meta = DocumentMeta {
        doc_id: doc_id.to_string(),
        title: title.to_string(),
        owner_id: owner_id.to_string(),
        folder_id: Some(folder_id.to_string()),
        additional_folder_ids: additional_folder_ids.to_vec(),
        workspace_id: None,
        doc_type,
        snapshot_version: 1,
        snapshot_s3_key: Some(format!("docs/{doc_id}/snapshots/1.bin")),
        is_deleted: false,
        deleted_at: None,
        link_sharing_mode: None,
        link_view_options: ogrenotes_storage::models::ViewOptions::default(),
        locked: false,
        is_template: false,
        created_at,
        updated_at,
    };
    if let Err(create_err) = doc_repo.create(&meta, snapshot).await {
        // `create` is conditional on `attribute_not_exists(PK)`, and it does
        // not distinguish "already there" from "DynamoDB said no" in its
        // error type — so ask the table instead of parsing the message.
        let existing = match on_existing {
            OnExistingDoc::Reject => None,
            OnExistingDoc::ReconcileReservedId => doc_repo
                .get(doc_id)
                .await
                .map_err(|e| format!("read back reserved document: {e}"))?,
        };
        match existing {
            None => return Err(format!("create document: {create_err}")),
            Some(_) => {
                // Our own earlier attempt got this far. Adopt it.
                tracing::info!(
                    doc_id,
                    error = %create_err,
                    "import: document already exists under the reserved id; \
                     reconciling instead of creating a duplicate",
                );
                reassert_imported_snapshot(doc_repo, doc_id, snapshot, &meta, updated_at, owner_id)
                    .await?;
            }
        }
    }

    for folder in std::iter::once(folder_id).chain(additional_folder_ids.iter().map(String::as_str))
    {
        folder_repo
            .add_child(&FolderChild {
                folder_id: folder.to_string(),
                child_id: doc_id.to_string(),
                child_type: ChildType::Doc,
                added_at: updated_at,
            })
            .await
            .map_err(|e| format!("link to folder: {e}"))?;
    }

    Ok(doc_id.to_string())
}

/// Re-write the initial snapshot of a document a previous import attempt
/// already created, without clobbering anything written since.
///
/// Why re-write at all: `DocRepo::create` writes the metadata row BEFORE the
/// S3 object, so a failure between the two leaves a row pointing at a snapshot
/// that isn't there. Before the id was reserved, the retry minted a new id and
/// that document was simply abandoned (a duplicate, but a readable one). Now
/// the retry adopts the same id, so the unreadable state would become
/// permanent unless the snapshot is re-asserted. The bytes are re-derived from
/// the same staged source, so this is a rewrite, not an edit.
///
/// Why conditionally: if the document has been written since, re-asserting
/// version 1 would silently discard that. Guarded by
/// `save_snapshot_conditional`, whose `expected_version` check and version
/// write are ONE conditional `UpdateItem` — a read-compare-write here would
/// leave a window between the compare and the write.
///
/// # Known limitation — live editor edits are NOT detected
///
/// `snapshot_version` only advances when a *snapshot* is written. Edits made
/// in the live editor persist as `UPDATE#` rows (see `routes::ws`) and do not
/// bump it until compaction runs, so this function also refuses to re-assert
/// when any `UPDATE#` row exists. That pair of checks covers both persistence
/// shapes, but neither is atomic with respect to an edit landing *during* the
/// re-assert: an update written between the check and the S3 put would be
/// rebasing onto a Y.Doc whose client ids the re-write replaced. The exposure
/// is one in-flight thread's document, seconds old, not yet surfaced to the
/// user by the wizard — judged acceptable, and recorded here rather than left
/// for the next reader to over-trust the version guard.
async fn reassert_imported_snapshot(
    doc_repo: &DocRepo,
    doc_id: &str,
    snapshot: &[u8],
    meta: &ogrenotes_storage::models::document::DocumentMeta,
    updated_at: i64,
    owner_id: &str,
) -> Result<(), String> {
    // Any pending live-editor update means the document has content this
    // snapshot doesn't know about. `1` is enough to answer "any?".
    let pending = doc_repo
        .get_pending_updates(doc_id, 1)
        .await
        .map_err(|e| format!("check reserved document for pending updates: {e}"))?;
    if !pending.is_empty() {
        tracing::warn!(
            doc_id,
            "import: reserved document has live edits pending; leaving its content alone",
        );
        return Ok(());
    }

    match doc_repo
        .save_snapshot_conditional(
            doc_id,
            snapshot,
            meta.snapshot_version,
            meta.snapshot_version,
            updated_at,
            owner_id,
        )
        .await
        .map_err(|e| format!("re-assert reserved document snapshot: {e}"))?
    {
        ogrenotes_storage::repo::doc_repo::SnapshotWrite::Committed => Ok(()),
        // Someone advanced the version between the create attempt and here.
        // Not an error — the document is further along than this import, which
        // is exactly the state we refuse to overwrite.
        ogrenotes_storage::repo::doc_repo::SnapshotWrite::VersionConflict => {
            tracing::warn!(
                doc_id,
                "import: reserved document was written between attempts; \
                 leaving its content alone",
            );
            Ok(())
        }
    }
}

/// How a `StartQuipImport` handler run ended, when it didn't error.
///
/// Distinct from "success" precisely because the queue must finalize the two
/// differently — see [`JobDisposition`].
#[derive(Debug, PartialEq, Eq)]
pub enum ImportRunOutcome {
    /// This worker held the lease and drove the import as far as it could:
    /// through the content pass, or to a terminal non-retryable condition
    /// such as a rejected token.
    Ran,
    /// Another live runner holds the lease; this run did nothing at all.
    HeldByLiveRunner,
}

/// Phase 1 inventory handler for the `StartQuipImport` trigger.
///
/// Claims the import's runner lease, re-reads the (token-free trigger's)
/// token from the [`TokenStore`], BFS-walks the user's selected Quip roots,
/// and persists `FOLDER#`/`THREAD#` rows plus the discovered thread total,
/// advancing the import to phase 1. Every write is insert-if-absent for
/// threads, so a re-run (retry, reaper takeover, or a rare double-claim)
/// never downgrades a thread that has already advanced — the inventory is
/// resumable and the lease is only an optimization.
///
/// The token is read here and NEVER logged or formatted. A revoked token
/// (`Unauthorized`) is terminal for this run: status flips to
/// `TokenRejected` and the handler returns [`ImportRunOutcome::Ran`] rather
/// than burning the retry budget hammering Quip with a dead credential — the
/// UI polls status and prompts a reconnect. Transient errors return `Err` so
/// the queue's retry/reaper resumes the walk from scratch (cheap, thanks to
/// insert-if-absent).
///
/// `pub` so integration tests can drive it directly (the `execute_import_docx`
/// seam precedent) without standing up a full consumer loop.
pub async fn execute_start_quip_import(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
) -> Result<ImportRunOutcome, String> {
    let instance = worker_instance_id();
    let now_ms = ogrenotes_common::time::now_usec() / 1000;

    // Best-effort lease. `Ok(false)` means a genuinely-live *other* runner
    // owns this import — nothing to do (NOT an error), and we must NOT clear
    // the claim: it belongs to that runner. `Ok(true)` means either no claim
    // existed or the prior holder's heartbeat is stale (past CLAIM_STALE_MS,
    // which sits below the reaper interval so a crashed worker's lease is
    // reclaimable by the time the entry is redelivered).
    if !ctx
        .import_repo
        .claim_runner(import_id, &instance, now_ms, CLAIM_STALE_MS)
        .await
        .map_err(|e| format!("claim runner: {e}"))?
    {
        // NOT a success. Reporting it as one would let the caller ack the
        // queue entry, and with Phase 2a's multi-hour pass this is the
        // *guaranteed* case (the reaper reclaims after 60s idle), not a rare
        // race — so acking here would routinely delete the only outstanding
        // record of a running import. See [`JobDisposition::HeldByLiveRunner`].
        tracing::info!(import_id, "quip import: held by a live runner; leaving the entry pending");
        return Ok(ImportRunOutcome::HeldByLiveRunner);
    }

    // From here we OWN the lease, and a background task refreshes it for
    // exactly as long as we hold it (see [`LeaseHeartbeat`]).
    let heartbeat = LeaseHeartbeat::spawn(Arc::clone(&ctx.import_repo), import_id, &instance);

    // Clear the lease on EVERY exit — success OR error — so a mid-handler
    // failure never leaves a held claim that would make the queue's retry
    // (running under a *different* instance id) see a live lease, no-op, and
    // leave the entry pending until the claim ages out. This mirrors what
    // `mark_quip_failure` does for Quip errors, now applied uniformly to
    // DDB-error `?`-returns too. Stop heartbeating FIRST, so a tick can't race
    // in behind the clear and resurrect the lease.
    let result = run_inventory(ctx, import_id, owner_id, &instance).await;
    drop(heartbeat);
    match ctx.import_repo.clear_runner_claim(import_id, &instance).await {
        // We were superseded mid-pass. Leaving the new holder's lease alone is
        // the point of the owner check; say so, because it also means this
        // run and another one overlapped.
        Ok(false) => tracing::warn!(
            import_id,
            "quip import: lease was taken over mid-run; not clearing the new holder's claim",
        ),
        Ok(true) => {}
        Err(e) => tracing::warn!(import_id, error = %e, "quip import: clearing the lease failed"),
    }
    result.map(|()| ImportRunOutcome::Ran)
}

/// Keeps the DynamoDB runner lease fresh for as long as the handler holds it,
/// from a background task, and stops on drop.
///
/// The lease is what stops two workers running the same import concurrently,
/// and it is only as good as its refresh interval. Heartbeating from the work
/// loop — every N threads — ties liveness to the *slowest unit of work*: one
/// image-heavy thread whose `sideload_images` runs past [`CLAIM_STALE_MS`]
/// makes a perfectly healthy runner look dead. That was survivable when a
/// redelivered entry got acked away after the first attempt; now that the
/// entry stays reclaimable for the whole pass (see
/// [`JobDisposition::HeldByLiveRunner`]), a multi-hour import would get ~180
/// chances to hand a second runner the lease — doubling Quip API calls against
/// the per-import 45/min throttle.
///
/// A timer that runs independently of the work is the only shape that makes
/// the refresh interval a property of the *clock* rather than of the workload.
/// Ticks are best-effort: `heartbeat_runner` is conditional on still owning the
/// lease, so a superseded runner's ticks are no-ops rather than a way to steal
/// it back.
struct LeaseHeartbeat(tokio::task::JoinHandle<()>);

/// How often the background task refreshes the lease. A third of
/// [`CLAIM_STALE_MS`] leaves room for two consecutive failed ticks (a DynamoDB
/// blip) before the lease looks stale.
const LEASE_HEARTBEAT_MS: u64 = (CLAIM_STALE_MS as u64) / 3;

impl LeaseHeartbeat {
    fn spawn(import_repo: Arc<ImportRepo>, import_id: &str, instance: &str) -> Self {
        let (import_id, instance) = (import_id.to_string(), instance.to_string());
        Self(tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(LEASE_HEARTBEAT_MS));
            // The first tick fires immediately; the claim we just took is
            // already fresh, so skip it.
            tick.tick().await;
            loop {
                tick.tick().await;
                import_repo
                    .heartbeat_runner(
                        &import_id,
                        &instance,
                        ogrenotes_common::time::now_usec() / 1000,
                    )
                    .await
                    .ok();
            }
        }))
    }
}

impl Drop for LeaseHeartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The owned-lease body of the inventory handler. Split out so
/// [`execute_start_quip_import`] can clear the runner claim on every exit
/// path (via a single guard) regardless of where the body returns.
async fn run_inventory(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
    instance: &str,
) -> Result<(), String> {
    let record = ctx
        .import_repo
        .get(import_id)
        .await
        .map_err(|e| format!("get import: {e}"))?
        .ok_or_else(|| format!("import {import_id} not found"))?;
    if record.owner_id != owner_id {
        return Err(format!("owner mismatch for import {import_id}"));
    }

    // Token-free trigger: read the token from the store. A missing token
    // is terminal for this run (same disposition as a revoked one).
    let token = match ctx
        .quip_token_store
        .get(import_id)
        .await
        .map_err(|e| format!("token store: {e}"))?
    {
        Some(t) => t,
        None => {
            ctx.import_repo
                .set_status(import_id, ImportStatus::TokenRejected)
                .await
                .ok();
            tracing::warn!(import_id, "quip inventory: no token in store; TokenRejected");
            return Ok(());
        }
    };

    // Per-import client: a fresh 45/min throttle isolates each import's
    // rate budget (never reuse the API's shared `quip_client`).
    let client = QuipClient::new(ctx.quip_base.clone());
    ctx.import_repo
        .set_status(import_id, ImportStatus::Running)
        .await
        .map_err(|e| format!("set running: {e}"))?;

    // BFS the selected roots. The walker's closure captures references to
    // the client and token (not moving the client) so the per-BFS-level
    // fetch throttles through one shared client.
    let inv = match walk_inventory(&record.selected_roots, |ids| {
        let (client, token) = (&client, &token);
        async move { client.folders(token, &ids).await }
    })
    .await
    {
        Ok(inv) => inv,
        Err(e) => return mark_quip_failure(ctx, import_id, owner_id, &e).await,
    };

    // Heartbeat between the folder walk and the thread-meta fetch so a
    // large tree doesn't look stalled to the reaper.
    ctx.import_repo
        .heartbeat_runner(import_id, instance, ogrenotes_common::time::now_usec() / 1000)
        .await
        .ok();

    // Persist folders (idempotent upsert).
    for f in &inv.folders {
        ctx.import_repo
            .put_folder(
                import_id,
                &FolderRow {
                    quip_folder_id: f.quip_folder_id.clone(),
                    owner_id: owner_id.to_string(),
                    title: f.title.clone(),
                    parent_quip_id: f.parent_quip_id.clone(),
                    ogre_folder_id: None,
                },
            )
            .await
            .map_err(|e| format!("put folder: {e}"))?;
    }

    // Fetch thread metadata in id-batches, then persist THREAD# rows
    // (insert-if-absent → a re-run never downgrades an advanced thread).
    let meta = match fetch_thread_meta(&client, &token, &inv, ctx, import_id, instance).await {
        Ok(m) => m,
        Err(e) => return mark_quip_failure(ctx, import_id, owner_id, &e).await,
    };
    for t in &inv.threads {
        let m = meta.get(&t.quip_thread_id);
        ctx.import_repo
            .put_thread(
                import_id,
                &ThreadRow {
                    quip_thread_id: t.quip_thread_id.clone(),
                    owner_id: owner_id.to_string(),
                    title: m.map(|m| m.title.clone()).unwrap_or_default(),
                    thread_type: m.map(|m| m.thread_type.clone()).unwrap_or_default(),
                    updated_usec: m.map(|m| m.updated_usec).unwrap_or(0),
                    member_folders: t.member_folders.clone(),
                    first_folder: t.first_folder.clone(),
                    state: ThreadState::Pending,
                    ogre_doc_id: None,
                    reason: None,
                    attempts: 0,
                },
            )
            .await
            .map_err(|e| format!("put thread: {e}"))?;
    }

    let (total, _) = ctx
        .import_repo
        .count_threads_by_state(import_id)
        .await
        .map_err(|e| format!("count threads: {e}"))?;
    ctx.import_repo
        .set_inventory_total(import_id, total)
        .await
        .map_err(|e| format!("set total: {e}"))?;
    ctx.import_repo
        .set_phase(import_id, 1)
        .await
        .map_err(|e| format!("set phase: {e}"))?;
    tracing::info!(import_id, total, "quip inventory: phase 1 complete");

    // Phase 2 runs inside the same job, under the same lease and the same
    // per-import client, so a single `StartQuipImport` carries an import all
    // the way from "a list of Quip roots" to real documents.
    run_content_pass(ctx, import_id, owner_id, &client, &token, &record).await
}

// ─── Phase 2a: the per-thread content pass ───────────────────────

/// How many *observed, thread-attributable* failures one thread gets before
/// the content pass marks it `Failed` and moves on (issue #142).
///
/// Sits deliberately below the job-level [`MAX_RETRIES`] budget. The queue
/// gives a `StartQuipImport` job four runs (attempt 0 plus three retries);
/// a bad thread is charged one attempt per run, so it is resolved on run 3
/// and the fourth run is slack. Raising this past `MAX_RETRIES` would put the
/// give-up decision *after* the dead-letter, which is #142 renamed rather
/// than fixed.
const MAX_THREAD_ATTEMPTS: u32 = 3;

/// Consecutive **first-time** [`ThreadImportError::Transient`] thread failures
/// that convince the pass it is looking at a broken *Quip*, not a broken
/// thread, and that it should stop and let the queue's backoff run.
///
/// Without this, a Quip-wide 5xx outage would walk the entire manifest
/// charging an attempt to every thread, and three such runs would mark a
/// 10 000-thread import `Failed` thread by thread over an outage that lasted
/// an hour. The breaker bounds that blast radius to a handful of threads.
///
/// **Only a thread's first-ever failure counts** (see the `else` arm in
/// [`run_content_pass`]). Re-counting a known-bad thread's later failures is
/// what let a run of `MAX_CONSECUTIVE_THREAD_FAILURES + 1` sort-adjacent
/// deterministic failures dead-letter the import: the breaker re-tripped at
/// the same offset every run, so threads past it never accumulated attempts.
/// Counting only first failures keeps the breaker firing on the *leading
/// edge* of not-yet-charged threads — which is exactly where an outage lives,
/// and exactly what a still-climbing cluster has already moved past — so an
/// outage is still contained while an adjacent cluster resolves. The
/// resolvable cluster size is bounded (see `run_content_pass`); the residual
/// beyond it degrades to a *retriable* dead-letter, never to marking good
/// documents `Failed`.
const MAX_CONSECUTIVE_THREAD_FAILURES: usize = 5;

/// Durable, user-visible reason for a thread whose import panicked. Authored
/// here and `&'static` on purpose — the panic payload itself must never reach
/// a `reason` or a `ReportNote`; see [`ThreadImportError::transient`].
const PANIC_REASON: &str = "this document could not be converted (the importer failed on its content)";

/// The complete set of `REPORT` row keys this worker writes.
///
/// One module so the set is countable by reading one place.
/// [`ReportNote::kind`] is a free-form `String` budgeted at
/// `REPORT_MAX_NOTE_KINDS` (8) **distinct kinds per import** — past that a new
/// kind's notes are dropped outright, not merely truncated — so a kind names
/// an *outcome*, never a cause, and the cause lives in `detail`. Splitting
/// `image_dropped` into `image_dropped_403` / `image_dropped_too_large` twice
/// over is all it would take to silently starve the kind whose job is to name
/// the lost documents. Five kinds here leaves headroom for the passes still
/// to come (link fallbacks, section-map losses).
///
/// Counters are uncapped in value and bounded only by the number of distinct
/// keys, which is this compile-time set.
/// `pub(crate)` so the read side — `routes::imports::get_status`, which
/// projects this row into the wizard's completion state — names the same
/// constants the writer does. A reader that re-typed the key strings would
/// drift the first time one was renamed, and the drift would be silent: an
/// unmatched key reads as a zero counter, i.e. "nothing was lost".
pub(crate) mod report {
    /// Declare the note kinds and the roster of them together, so the two
    /// cannot disagree.
    ///
    /// A hand-kept list next to the constants would be checked by nothing:
    /// adding a `KIND_` and forgetting the list leaves the budget test green
    /// while the budget is being exceeded, which is the exact failure the
    /// test exists to catch. Declared this way, `ALL` **is** the constants —
    /// adding a kind without a roster entry is not something you can express.
    macro_rules! report_keys {
        ($roster:ident: $($(#[$attr:meta])* $name:ident = $value:literal;)*) => {
            $($(#[$attr])* pub const $name: &str = $value;)*
            /// Every key declared in this group, in declaration order.
            /// Derived from the constants — see `report_keys!`.
            ///
            /// `cfg(test)` because the roster's only job is to let the tests
            /// below check the set as a whole; the worker itself always names
            /// one constant at a time. Emitting it unconditionally would be
            /// dead code in every non-test build.
            #[cfg(test)]
            pub const $roster: &[&str] = &[$($name),*];
        };
    }

    // One note kind per outcome. Cause goes in `ReportNote::detail`.
    report_keys! {
        ALL_KINDS:
        KIND_THREAD_SKIPPED = "thread_skipped";
        KIND_THREAD_FAILED = "thread_failed";
        KIND_IMAGE_DROPPED = "image_dropped";
        KIND_CONTENT_TRUNCATED = "content_truncated";
        /// One outcome: mentions in this import lost their link. `detail`
        /// names the cause, per the "kind names an outcome, never a cause"
        /// rule above.
        KIND_MENTIONS_DEGRADED = "mentions_degraded";
        /// One outcome: an embedded Quip live app's contents did not come
        /// over (#191). Deliberately **not** split by app kind —
        /// `live_app_kanban` / `live_app_poll` / `live_app_calendar` is
        /// exactly the cause-splitting the paragraph above forbids, and three
        /// of them would take the last free slot and two we do not have.
        /// Which app it was goes in `detail`.
        KIND_LIVE_APP_DROPPED = "live_app_dropped";
        /// One outcome: this document's spreadsheet formulas did not come
        /// over (#192). One note per *document*, with the count in `detail` —
        /// a sheet with 300 formulas must not spend 300 of the 25 notes this
        /// kind gets, and the true total is on the counter, which is uncapped.
        KIND_FORMULAS_DROPPED = "formulas_dropped";
    }

    report_keys! {
        ALL_COUNTERS:
        THREADS_IMPORTED = "threads_imported";
        THREADS_SKIPPED_CHAT = "threads_skipped_chat";
        THREADS_SKIPPED_FORBIDDEN = "threads_skipped_forbidden";
        THREADS_FAILED = "threads_failed";
        IMAGES_DROPPED = "images_dropped";
        THREADS_TRUNCATED = "threads_deep_nesting_truncated";
        FOLDERS_FORBIDDEN = "folders_forbidden";
        /// Threads in which at least one person mention lost its link and
        /// became plain text. Counts *documents*, like the other `THREADS_*`
        /// keys, so a chatty document cannot dominate the number.
        THREADS_MENTIONS_DEGRADED = "threads_mentions_degraded";
        /// Embedded live-app blocks whose contents were not converted (#191).
        /// Counts **blocks**, not documents, like `IMAGES_DROPPED` — "3
        /// boards lost" is the sentence a user can act on; "2 documents had a
        /// board" is not.
        LIVE_APPS_DROPPED = "live_apps_dropped";
        /// Spreadsheet formulas that were not imported (#192). Counts
        /// **formulas**, for the same reason, and is the number that stays
        /// true after the notes hit their 25-per-kind budget.
        FORMULAS_DROPPED = "spreadsheet_formulas_dropped";
    }
}


/// Write one outcome to the import's `REPORT` row.
///
/// **Advisory by construction: this returns nothing, so it cannot enter the
/// import's control flow.** A report describes an import; it must never be
/// able to stop one. The failure this guards against is not hypothetical — a
/// `REPORT` row whose `counters` map has been written with a non-numeric
/// value fails `report_from_item` on *every* subsequent read, so both
/// `bump_report_counter` and `append_report_note` fail permanently for that
/// import. Propagating that would halt a migration over its own bookkeeping:
/// an import that dies because it could not write a note about a dying import
/// is the worst outcome available. So each half is attempted independently
/// (a poisoned counter must not also cost the note) and both are logged.
async fn record_report(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
    counter: &str,
    note: Option<ReportNote>,
) {
    record_report_by(ctx, import_id, owner_id, counter, 1, note).await
}

/// [`record_report`] for an outcome whose counter counts *things*, not
/// occasions: one note saying "30 formulas were dropped" has to leave the
/// counter at 30, not 1.
///
/// Advisory identically — it returns `()`, and the two writes are attempted
/// independently, for the reasons on `record_report`.
async fn record_report_by(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
    counter: &str,
    by: u64,
    note: Option<ReportNote>,
) {
    if let Err(e) = ctx.import_repo.bump_report_counter(import_id, owner_id, counter, by).await {
        tracing::warn!(
            import_id,
            counter,
            error = %e,
            "quip import: report counter write failed; the import continues",
        );
    }
    let Some(note) = note else { return };
    if let Err(e) = ctx.import_repo.append_report_note(import_id, owner_id, note).await {
        tracing::warn!(
            import_id,
            error = %e,
            "quip import: report note write failed; the import continues",
        );
    }
}

/// A short, class-only description of a [`QuipError`], safe to persist in a
/// user-visible `THREAD#.reason` or [`ReportNote::detail`].
///
/// Deliberately **not** the error's `Display`. Two variants carry text this
/// codebase did not author: `Api::message` is Quip's raw response body, and
/// `Parse` / `Http` wrap reqwest's own message (which embeds the request
/// URL). Neither can be shown to be free of a credential or a user
/// identifier, and both would land in DynamoDB and from there in the
/// frontend's report. The status code is the part that is simultaneously safe
/// and diagnostic, so it is the only part that survives into durable state;
/// the full `Display` still goes to the process log, which is where an
/// operator debugging one import looks.
///
/// `Unauthorized` and `Forbidden` never carry any text at all — asserted in
/// `quip-import`'s client tests — but they are spelled out here rather than
/// borrowed from `Display` so that no future variant can quietly inherit the
/// permissive branch.
fn safe_quip_reason(e: &QuipError) -> String {
    match e {
        QuipError::Unauthorized => "Quip rejected the import's credential (HTTP 401)".to_string(),
        QuipError::Forbidden => "Quip denied access to this content (HTTP 403)".to_string(),
        QuipError::RateLimited { .. } => "Quip rate-limited the import".to_string(),
        QuipError::Http(_) => "the request to Quip failed (network error or timeout)".to_string(),
        QuipError::Api { status, .. } => format!("Quip returned HTTP {status}"),
        QuipError::Parse(_) => "Quip returned a response this import could not read".to_string(),
    }
}

/// How one thread's import ended when it didn't succeed.
///
/// Four dispositions, because every failure has to answer two independent
/// questions — *is the run still viable?* and *is this thread still viable?* —
/// and a `String` would make the caller sniff for both.
#[derive(Debug)]
pub enum ThreadImportError {
    /// HTTP 401 — the stored credential is dead, so every remaining thread
    /// would fail identically. Terminal for this run: the caller flips it to
    /// `TokenRejected` and returns `Ok(())` instead of burning the retry
    /// budget hammering Quip with a revoked token.
    TokenRejected,
    /// HTTP 403 on the thread itself. **Not** a dead credential — one
    /// access-restricted document in an otherwise-readable account. This
    /// is issue #141: mapping it to `TokenRejected` halted the whole import
    /// and told the user to reconnect a token that was never the problem, so
    /// the reconnect changed nothing and the re-run wedged on the same thread.
    /// The thread is marked `Skipped` with this reason and the pass moves on;
    /// no retry could change the answer.
    Forbidden(String),
    /// A failure this thread might survive on a later attempt — a Quip 5xx, a
    /// timeout, an unreadable body. Charged to the thread's attempt counter;
    /// past [`MAX_THREAD_ATTEMPTS`] the thread is marked `Failed` and the pass
    /// moves on rather than stalling the other 999 documents behind it
    /// (issue #142). The string is token-free — see [`safe_quip_reason`].
    Transient(String),
    /// Not attributable to this thread at all: a DynamoDB or S3 failure, or
    /// Quip throttling the whole import. Aborts the pass with `Err` so the
    /// queue retries the job — the pre-#141 behavior, deliberately unchanged —
    /// and deliberately **not** charged to the thread's attempt budget, which
    /// would otherwise let a storage blip condemn an innocent thread.
    RunFailure(String),
}

impl ThreadImportError {
    /// Build a [`Self::Transient`] from something that is not a [`QuipError`].
    ///
    /// **Takes `&'static str`, not `String`, and that is the whole point.**
    /// `Transient`'s payload is the one error string that reaches durable,
    /// user-visible state (`THREAD#.reason`, `ReportNote::detail`), so the
    /// leak guarantee has to be checkable rather than merely true today. It
    /// is checkable because `Transient` has exactly two origins — this one
    /// and `From<QuipError>`, which routes through [`safe_quip_reason`] — and
    /// this one *cannot* carry runtime data at all: a `format!` result, a
    /// response body, a URL, or a panic payload will not compile through a
    /// `&'static str`. Anything that is not attributable to the thread should
    /// be [`Self::RunFailure`], which never reaches durable state.
    fn transient(reason: &'static str) -> Self {
        Self::Transient(reason.to_string())
    }
}

impl std::fmt::Display for ThreadImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TokenRejected => write!(f, "quip token rejected"),
            Self::Forbidden(m) | Self::Transient(m) | Self::RunFailure(m) => write!(f, "{m}"),
        }
    }
}

impl From<QuipError> for ThreadImportError {
    fn from(e: QuipError) -> Self {
        match e {
            QuipError::Unauthorized => Self::TokenRejected,
            QuipError::Forbidden => Self::Forbidden(safe_quip_reason(&QuipError::Forbidden)),
            // Throttling is never one thread's fault, and walking to the next
            // thread would only spend more of the same exhausted budget.
            // Abort and let the queue's backoff do the waiting — which is
            // exactly what this did before #141, so nothing regresses.
            e @ QuipError::RateLimited { .. } => Self::RunFailure(safe_quip_reason(&e)),
            e => Self::Transient(safe_quip_reason(&e)),
        }
    }
}

/// How many Quip person ids ride in one `/1/users/?ids=` request.
///
/// Quip's ids are short, so 50 of them is a query string of a few hundred
/// bytes — comfortably inside any URL limit — while keeping a
/// mention-heavy import to a single-digit number of requests against a
/// **50 requests/minute** budget.
const USER_LOOKUP_BATCH: usize = 50;

/// Quip person id → OgreNotes user id, resolved once per import.
///
/// **This exists to make mention resolution cost O(distinct people), not
/// O(mentions).** Quip's Automation API allows 50 requests per minute per
/// token; a naive per-mention lookup would spend an import's whole budget
/// on a single chatty document and then stall every remaining thread. So
/// lookups are batched ([`USER_LOOKUP_BATCH`]) *and* memoized for the
/// lifetime of the content pass: the tenth document that mentions the same
/// colleague costs nothing.
///
/// **Negative answers are cached too.** An outside collaborator with no
/// OgreNotes account is the common case in a real migration, and re-asking
/// Quip about them once per document is the same rate-limit hazard as
/// re-asking about a match.
///
/// **Emails never leave this type.** An address is read from Quip, handed
/// straight to `UserRepo::get_by_email`, and dropped; what is cached, and
/// all that any caller can observe, is an OgreNotes user id.
#[derive(Default)]
pub struct PersonDirectory {
    /// Decided answers only. Absent = not yet known, or a lookup failed and
    /// so stays retryable.
    known: std::collections::HashMap<String, PersonFact>,
    /// Set once `/1/users/` has answered in a way no retry can change. The
    /// whole run then stops asking: without this the doomed request is
    /// re-issued once per mention-bearing thread, and on a 1 000-thread
    /// import that is 1 000 calls against the 50 requests/minute budget the
    /// batching exists to protect.
    endpoint_dead: bool,
    /// How many chunks have come back with a **request-shaped** 4xx
    /// (400/422 — "I understood you and the request is wrong").
    ///
    /// Deliberately not fatal on the first one. A 400 on a batch is
    /// ambiguous: it can mean the `?ids=` shape is wrong, or that *one*
    /// malformed id in a chunk of fifty poisoned the request. Killing the
    /// endpoint on the first 400 would let a single bad id silently degrade
    /// every mention in the import. So a request-shaped 4xx decides only its
    /// own chunk, and only a run of [`MAX_PERMANENT_CHUNK_FAILURES`] of them
    /// is taken as evidence about the endpoint itself. A *path*-shaped 4xx
    /// (404/405/501) is unambiguous — no id in a query string can cause it —
    /// and kills the endpoint immediately.
    permanent_chunk_failures: usize,
}

/// Request-shaped 4xx responses tolerated before the person-lookup endpoint
/// is presumed wrong rather than the ids being wrong.
///
/// Bounds the waste at three doomed requests per run while keeping one bad
/// id from mass-degrading an import. See
/// [`PersonDirectory::permanent_chunk_failures`].
const MAX_PERMANENT_CHUNK_FAILURES: usize = 3;

/// One decided answer about a Quip person id.
///
/// The `NotAPerson` case exists because `<control>` is not exclusive to
/// people: the staged corpus wraps folder and thread chips in it too (see
/// `import_quip::walk_control`). Quip answering "no such user" for an id is
/// the signal that separates them, and routing it to a `DocMention` is what
/// keeps a wrapped document link back-patchable instead of degrading it to
/// plain text — which would be a regression against the pre-feature
/// behavior, where the wrapper was simply stripped.
#[derive(Clone)]
enum PersonFact {
    /// Matched to this OgreNotes user.
    User(String),
    /// Quip returned a profile, so this is a person; no OgreNotes account
    /// carries its address (or the profile exposed no address at all).
    NoAccount,
    /// Quip returned no profile for the id.
    NotAPerson,
}

impl From<&PersonFact> for ogrenotes_collab::import_quip::PersonOutcome {
    fn from(fact: &PersonFact) -> Self {
        match fact {
            PersonFact::User(id) => Self::User(id.clone()),
            PersonFact::NoAccount => Self::NoAccount,
            PersonFact::NotAPerson => Self::NotAPerson,
        }
    }
}

/// What [`PersonDirectory::resolve`] learned about the ids it was asked for.
struct PersonResolution {
    /// Every id the caller asked about that reached a decision.
    decided: std::collections::HashMap<String, ogrenotes_collab::import_quip::PersonOutcome>,
    /// The disposition for ids that reached no decision, or `None` when
    /// every id was decided.
    ///
    /// A decided negative degrades permanently and silently (correct — no
    /// retry widens it). A *failure* must not be checkpointed, or the thread
    /// is permanently and invisibly degraded by a blip — but **which**
    /// failure it was decides who pays. See [`LookupFault`].
    fault: Option<LookupFault>,
    /// True only on the call that concluded the endpoint is unusable, so the
    /// caller writes the durable report note exactly once per run.
    endpoint_just_died: bool,
}

/// Why some ids reached no decision — and therefore who is charged for it.
///
/// This mirrors the dispositions [`ThreadImportError`] already documents
/// rather than inventing a parallel policy: throttling is never one thread's
/// fault, a dead credential is run-terminal, and a storage blip must not be
/// allowed to condemn an innocent thread to `Failed`. Ordered by severity so
/// a mixed batch reports the disposition that protects the most.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum LookupFault {
    /// Quip could not answer (5xx, timeout, unreadable body). Attributable
    /// to the attempt, so the thread's own retry budget applies.
    Quip,
    /// The **identity store** could not answer. Not attributable to this
    /// thread at all: charging it would let a DynamoDB blip spend a good
    /// document's three attempts and mark it `Failed`. Aborts the pass so
    /// the queue retries the job, exactly as a `put secmap` failure does.
    Storage,
    /// Quip is throttling. Never one thread's fault, and walking to the next
    /// thread would only spend more of the same exhausted budget.
    RateLimited,
    /// The credential is dead. Run-terminal.
    TokenRejected,
}

impl LookupFault {
    /// The disposition this fault carries, per the policy on
    /// [`ThreadImportError`]'s variants.
    fn into_thread_error(self) -> ThreadImportError {
        match self {
            Self::Quip => ThreadImportError::transient("person lookup failed"),
            // `RunFailure`, not `Transient`: never charged to the thread, and
            // its payload never reaches durable state, so a storage error's
            // text — which is built from an *email* lookup key — cannot leak
            // into a `reason` or a `ReportNote`.
            Self::Storage => {
                ThreadImportError::RunFailure("person lookup: identity store unavailable".into())
            }
            Self::RateLimited => {
                ThreadImportError::RunFailure("person lookup: quip rate limited".into())
            }
            Self::TokenRejected => ThreadImportError::TokenRejected,
        }
    }
}

impl PersonDirectory {
    /// Resolve `wanted` Quip person ids to OgreNotes user ids, consulting
    /// the cache first and asking Quip only about the remainder.
    ///
    /// **Never returns an error, but does report what it could not decide.**
    /// A person Quip will not show us, and a profile with no visible email,
    /// are *decisions*: they are cached and
    /// [`ogrenotes_collab::import_quip::resolve_person_mentions`] turns them
    /// into named plain-text placeholders, permanently and correctly — no
    /// retry could widen either answer. A Quip 5xx, a 401/403, a rate limit,
    /// or a DynamoDB blip on the email pointer is *not* a decision: it leaves
    /// the id uncached and counts toward
    /// [`PersonResolution::undecided`], which the caller turns into a
    /// [`ThreadImportError::Transient`] so the thread is not checkpointed
    /// with losses it could have recovered.
    ///
    /// That is a deliberate change from the shape this feature shipped in,
    /// where a transient lookup failure still checkpointed `ContentDone` and
    /// a re-run skipped the thread with zero Quip calls — issue #155's
    /// pattern, and worse than #155 because the mention path writes no report
    /// note, so the loss was undiscoverable. Costing one thread a retry is
    /// much cheaper than permanently and invisibly degrading it.
    ///
    /// A **permanent** endpoint 4xx is the exception: it *is* a decision,
    /// because retrying a wrong URL forever is the failure mode, not the
    /// cure. See [`Self::endpoint_dead`].
    async fn resolve(
        &mut self,
        ctx: &WorkerCtx,
        import_id: &str,
        client: &QuipClient,
        token: &QuipToken,
        wanted: &[String],
    ) -> PersonResolution {
        let missing: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            wanted
                .iter()
                .filter(|id| !self.known.contains_key(*id) && seen.insert((*id).clone()))
                .cloned()
                .collect()
        };

        let mut fault: Option<LookupFault> = None;
        let mut endpoint_just_died = false;

        for chunk in missing.chunks(USER_LOOKUP_BATCH) {
            if self.endpoint_dead {
                // Already answered unusably this run. Degrade the rest for
                // free rather than re-buying the same 4xx per thread.
                //
                // `NoAccount`, not `NotAPerson`: with the endpoint down we
                // learned nothing about *which* of the two an id is, and
                // plain `@Name` text is the answer that is never actively
                // wrong. Calling a real colleague a missing document is —
                // and a `DocMention` would additionally persist that
                // person's Quip id into the stored document.
                for id in chunk {
                    self.known.insert(id.clone(), PersonFact::NoAccount);
                }
                continue;
            }
            let profiles = match client.users(token, chunk).await {
                Ok(profiles) => profiles,
                // 403: the token may not read these profiles. The 403
                // doctrine this worker already applies is "no retry could
                // widen the user's Quip permissions", so decide them — but
                // as `NoAccount`, never as a thread skip. The *document* is
                // readable; losing it over an invisible profile would be a
                // far worse trade than a name in plain text.
                Err(QuipError::Forbidden) => {
                    for id in chunk {
                        self.known.insert(id.clone(), PersonFact::NoAccount);
                    }
                    tracing::warn!(
                        import_id,
                        people = chunk.len(),
                        "quip content: the token may not read these profiles; \
                         they degrade to names",
                    );
                    continue;
                }
                Err(e) if is_permanent_lookup_failure(&e) => {
                    // Whatever else is true, this chunk will never be
                    // answered. Decide it so the mentions degrade once.
                    for id in chunk {
                        self.known.insert(id.clone(), PersonFact::NoAccount);
                    }
                    self.permanent_chunk_failures += 1;
                    // A path-shaped 4xx indicts the endpoint immediately; a
                    // request-shaped one only after it has happened enough
                    // times to rule out "one malformed id in the batch".
                    if is_endpoint_shape_failure(&e)
                        || self.permanent_chunk_failures >= MAX_PERMANENT_CHUNK_FAILURES
                    {
                        self.endpoint_dead = true;
                        endpoint_just_died = true;
                    }
                    // Class only, never the error's own text — `QuipError`
                    // carries Quip's raw body and reqwest's URL.
                    tracing::warn!(
                        import_id,
                        people = chunk.len(),
                        endpoint_dead = self.endpoint_dead,
                        error = %safe_quip_reason(&e),
                        "quip content: the person-lookup endpoint rejected a batch permanently; \
                         these mentions degrade to names",
                    );
                    continue;
                }
                Err(e) => {
                    // Left *uncached*: undecided, so the caller retries
                    // rather than checkpointing a recoverable loss. The
                    // fault class decides who pays for that retry.
                    fault = fault.max(Some(match e {
                        QuipError::Unauthorized => LookupFault::TokenRejected,
                        QuipError::RateLimited { .. } => LookupFault::RateLimited,
                        _ => LookupFault::Quip,
                    }));
                    tracing::warn!(
                        import_id,
                        people = chunk.len(),
                        error = %safe_quip_reason(&e),
                        "quip content: person lookup failed; the thread stays retryable",
                    );
                    continue;
                }
            };
            for id in chunk {
                // Quip returned no profile for this id. It is not a person:
                // `<control>` also wraps folder and thread chips, and this is
                // the signal that tells them apart. No retry widens that, so
                // decide it — and decide it as a *document*, which is what
                // the walker would have produced without the wrapper.
                let Some(profile) = profiles.get(id) else {
                    self.known.insert(id.clone(), PersonFact::NotAPerson);
                    continue;
                };
                // A profile came back, so this IS a person — just one we
                // cannot address. Never `NotAPerson`: rendering a real
                // colleague as a missing document is the bug this feature
                // exists to fix.
                //
                // EXACT email only. Matching on display name is what the
                // design's Phase-3 identity confirm gate exists to prevent:
                // two people called "Joel" would silently swap mentions.
                let Some(email) = profile.emails.iter().find(|e| !e.trim().is_empty()) else {
                    self.known.insert(id.clone(), PersonFact::NoAccount);
                    continue;
                };
                match ctx.user_repo.get_by_email(email).await {
                    Ok(found) => {
                        self.known.insert(
                            id.clone(),
                            found.map_or(PersonFact::NoAccount, |u| PersonFact::User(u.user_id)),
                        );
                    }
                    // Deliberately logged WITHOUT the error's text: the
                    // lookup key is an email address, and a DynamoDB error
                    // message is not something this code can prove is free
                    // of it. Left uncached, and charged to the *run* rather
                    // than to this thread — see [`LookupFault::Storage`].
                    Err(_) => {
                        fault = fault.max(Some(LookupFault::Storage));
                        tracing::warn!(
                            import_id,
                            "quip content: identity lookup failed for a mention; \
                             not charged to the thread",
                        );
                    }
                }
            }
        }

        PersonResolution {
            decided: wanted
                .iter()
                .filter_map(|id| Some((id.clone(), self.known.get(id)?.into())))
                .collect(),
            // A fault only matters if it actually cost us an answer: an id
            // decided by an earlier call is not owed a retry.
            fault: fault.filter(|_| wanted.iter().any(|id| !self.known.contains_key(id))),
            endpoint_just_died,
        }
    }
}

/// Whether a `/1/users/` failure is one no retry can fix.
///
/// A 4xx other than 429 means Quip understood the request and rejected it.
/// 401 and 403 are deliberately excluded: they arrive as their own
/// [`QuipError`] variants and describe the credential, not the request.
fn is_permanent_lookup_failure(e: &QuipError) -> bool {
    matches!(e, QuipError::Api { status, .. } if (400..500).contains(status) && *status != 429)
}

/// Whether a permanent failure indicts the **endpoint** rather than the ids.
///
/// 404/405/501 are answers about the path and the method: no value in a
/// query string can produce them, so one of these settles the `?ids=` batch
/// shape — an *assumption* this client documents openly — on the spot. A
/// 400/422 cannot be attributed that way, because a single malformed id in a
/// batch of fifty produces the same status; those are counted instead, and
/// only [`MAX_PERMANENT_CHUNK_FAILURES`] of them indict the endpoint.
fn is_endpoint_shape_failure(e: &QuipError) -> bool {
    matches!(e, QuipError::Api { status, .. } if matches!(status, 404 | 405 | 501))
}

/// Where an imported thread's document is filed.
///
/// `THREAD#` rows carry **Quip** folder ids; documents need OgreNotes ones.
/// The mapping is each `FOLDER#` row's `ogre_folder_id`, populated by
/// [`mirror_folder_tree`] immediately before this is built (#236).
///
/// `fallback` — the import's `target_folder_id` — is what a thread resolves
/// to when its folder has no mapping. That is no longer the *normal* case, as
/// it was while Phase 1 wrote `ogre_folder_id: None` for every folder, but it
/// stays reachable and stays correct: a `THREAD#` row can name a folder that
/// is not in this import's `FOLDER#` set (a manifest written by an older
/// inventory), and filing such a document flat under the destination beats
/// failing the import over its filing.
pub struct FolderMapping {
    by_quip_id: std::collections::HashMap<String, String>,
    fallback: String,
}

impl FolderMapping {
    fn resolve(&self, quip_folder_id: &str) -> &str {
        self.by_quip_id
            .get(quip_folder_id)
            .map(String::as_str)
            .unwrap_or(&self.fallback)
    }

    /// `(primary, additional)` OgreNotes folders for a thread, deduplicated
    /// and with the primary never repeated in `additional`. With the
    /// `target_folder_id` fallback in play these collapse to one folder,
    /// which is the correct (if flat) answer, not a bug.
    fn folders_for(&self, thread: &ThreadRow) -> (String, Vec<String>) {
        let primary = self.resolve(&thread.first_folder).to_string();
        let mut additional: Vec<String> = Vec::new();
        for quip_folder in &thread.member_folders {
            let mapped = self.resolve(quip_folder);
            if mapped != primary && !additional.iter().any(|f| f == mapped) {
                additional.push(mapped.to_string());
            }
        }
        (primary, additional)
    }
}

/// Build the Quip-folder → OgreNotes-folder mapping for an import from its
/// `FOLDER#` rows plus the `META` row's `target_folder_id`.
///
/// A missing `target_folder_id` is an error, not a guess: the worker has no
/// auth context with which to invent a destination, exactly as
/// [`execute_import_docx`] refuses a `None` folder.
pub async fn build_folder_mapping(
    ctx: &WorkerCtx,
    import_id: &str,
    record: &ogrenotes_storage::models::import::ImportRecord,
) -> Result<FolderMapping, String> {
    let fallback = record.target_folder_id.clone().ok_or_else(|| {
        format!("import {import_id} has no target_folder_id; its destination was never chosen")
    })?;
    let folders = ctx
        .import_repo
        .list_folders(import_id)
        .await
        .map_err(|e| format!("list folders: {e}"))?;
    let by_quip_id = folders
        .into_iter()
        .filter_map(|f| f.ogre_folder_id.map(|ogre| (f.quip_folder_id, ogre)))
        .collect::<std::collections::HashMap<_, _>>();
    if by_quip_id.is_empty() {
        tracing::info!(
            import_id,
            target_folder_id = %fallback,
            "quip content: no FOLDER# row carries an ogre_folder_id; \
             every thread files into the import's target folder (flat import)",
        );
    }
    Ok(FolderMapping { by_quip_id, fallback })
}

// ─── Mirroring the Quip folder tree (#236) ───────────────────────

/// Where one mirrored folder hangs, as decided by
/// [`order_folders_parent_first`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MirrorParent<'a> {
    /// Directly under the import's destination folder. A selected root, and
    /// also the documented fallback for a folder whose parent cannot be
    /// placed — see [`order_folders_parent_first`].
    ImportRoot,
    /// Under the OgreNotes folder mirroring this Quip folder, which the
    /// ordering guarantees appears **earlier** in the returned sequence.
    Quip(&'a str),
}

/// Order an import's `FOLDER#` rows so that no folder is created before the
/// folder it hangs under, and say what each one hangs under.
///
/// `list_folders` returns rows in `SK` order — the Quip folder id's order,
/// which says nothing about depth — so the creation order has to be derived
/// here rather than assumed. Every input row appears in the output exactly
/// once: a folder this function dropped would silently never be mirrored.
///
/// # The rule for a parent that cannot be placed
///
/// Two shapes reach it, and both resolve to [`MirrorParent::ImportRoot`]:
///
/// - **A parent outside the selected scope.** The user picked a sub-folder as
///   an import root, or a folder's parent was simply not selected. Its
///   `parent_quip_id` names a folder with no `FOLDER#` row.
/// - **A cycle.** `walk_inventory` records the BFS *tree*, so it cannot emit
///   one — but rows written by an older inventory pass, or half-migrated, are
///   not covered by that argument, and "this terminates" must not rest on an
///   invariant enforced in another crate.
///
/// **Re-parent, never drop.** A dropped folder takes its documents' filing
/// with it and the user is never told which piece of their structure went
/// missing; a re-parented folder is visibly present, at worst one level
/// shallower than it was in Quip. That is a fidelity loss the user can see
/// and fix in ten seconds, which is the strictly better failure.
///
/// Placement proceeds in waves, so the cost is O(rows × depth) — a few
/// hundred folders at Quip-account scale, all in memory, no I/O.
fn order_folders_parent_first(rows: &[FolderRow]) -> Vec<(&FolderRow, MirrorParent<'_>)> {
    let in_scope: std::collections::HashSet<&str> =
        rows.iter().map(|r| r.quip_folder_id.as_str()).collect();
    let mut placed: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut ordered: Vec<(&FolderRow, MirrorParent<'_>)> = Vec::with_capacity(rows.len());

    // Sorted so two runs over the same manifest create folders in the same
    // sequence, which keeps two runs' logs comparable.
    let mut pending: Vec<&FolderRow> = rows.iter().collect();
    pending.sort_by(|a, b| a.quip_folder_id.cmp(&b.quip_folder_id));

    while !pending.is_empty() {
        let mut deferred: Vec<&FolderRow> = Vec::new();
        let mut progressed = false;
        for row in pending {
            let parent = match row.parent_quip_id.as_deref() {
                // A selected root, or a parent nobody selected.
                None => Some(MirrorParent::ImportRoot),
                Some(p) if !in_scope.contains(p) => Some(MirrorParent::ImportRoot),
                Some(p) if placed.contains(p) => Some(MirrorParent::Quip(p)),
                // In scope but not placed yet — possibly later in this very
                // wave, so try again next time round.
                Some(_) => None,
            };
            match parent {
                Some(parent) => {
                    placed.insert(row.quip_folder_id.as_str());
                    ordered.push((row, parent));
                    progressed = true;
                }
                None => deferred.push(row),
            }
        }
        if !progressed {
            // Every survivor names an in-scope parent that will never be
            // placed, i.e. they form one or more cycles. Same rule as an
            // unselected parent — and this is the branch that makes the loop
            // terminate rather than spin.
            tracing::warn!(
                folders = deferred.len(),
                "quip content: folder rows form a cycle; \
                 mirroring them directly under the import's destination",
            );
            ordered.extend(deferred.into_iter().map(|row| (row, MirrorParent::ImportRoot)));
            break;
        }
        pending = deferred;
    }
    ordered
}

/// Create one OgreNotes folder per inventoried Quip folder, under the
/// import's destination, and record each one on its `FOLDER#` row.
/// [`build_folder_mapping`] reads those ids straight afterwards, so the
/// documents file themselves into the mirrored tree with no further change.
///
/// # Idempotency — the property the whole pass turns on
///
/// The importer is re-startable and the reaper re-runs crashed jobs, so this
/// runs many times for one import. `ogre_folder_id` is the idempotency key,
/// used exactly as `routes::imports::ensure_import_folder` uses
/// `import_folder_id`:
///
/// - **present** → a previous run already mirrored this folder; adopt it and
///   create nothing.
/// - **absent** → create the folder, *then* record the id under a conditional
///   write ([`ImportRepo::record_ogre_folder`]). A concurrent loser reads the
///   winner's id back and leaves its own folder as a harmless empty orphan,
///   owned by the user and linked under its parent.
///
/// Ordering matters the same way it does in `ensure_import_folder`: the
/// folder is materialized **before** its id is recorded, so a crash between
/// the two can only leave an unreferenced folder — never a recorded id
/// pointing at a folder that does not exist, which would file documents into
/// a destination that is not there.
///
/// Two consequences worth being explicit about:
///
/// - **Resumability is free.** A run that dies half way through the tree
///   leaves the folders it made recorded; the next run adopts them and picks
///   up at the first unrecorded one.
/// - **Nothing here touches a document.** Folder location is mutable after an
///   import: if the user moves an imported document, a later run must respect
///   that. There is deliberately no "repair the tree" step — a document is
///   filed once, by the content pass, and only while its thread is still
///   `Pending`.
///
/// # Empty folders are created anyway
///
/// A Quip folder holding only chats (skipped), only inaccessible threads, or
/// nothing at all yields an empty OgreNotes folder, and that is the intended
/// outcome. The user asked for their structure; an empty folder is an honest
/// account of what was there, and pruning would quietly disagree with the
/// Quip window the user is comparing against.
///
/// # Failure is run-terminal, on purpose
///
/// A folder that cannot be created returns `Err`, which fails the job and
/// lets the queue retry it — rather than continuing with a partial tree.
/// Continuing looks kinder and is not: the documents under the missing branch
/// would file into the fallback, and because a re-run must never move a
/// document that is already filed, that flattening would be **permanent**. An
/// all-or-nothing tree with a retry behind it is the only version that can
/// still come out right. It is also why this needs no new report note kind:
/// there is no "folder we could not create" for a *completed* import to
/// report.
async fn mirror_folder_tree(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
    record: &ogrenotes_storage::models::import::ImportRecord,
) -> Result<(), String> {
    use ogrenotes_storage::models::folder::{Folder, FolderChild};
    use ogrenotes_storage::models::{ChildType, FolderType};

    let rows = ctx
        .import_repo
        .list_folders(import_id)
        .await
        .map_err(|e| format!("list folders: {e}"))?;
    if rows.is_empty() {
        return Ok(());
    }
    let destination = record.target_folder_id.clone().ok_or_else(|| {
        format!("import {import_id} has no target_folder_id; its destination was never chosen")
    })?;

    // quip folder id -> the OgreNotes folder durably recorded for it. Built
    // as we go, and read for a child's parent — which the ordering guarantees
    // is already in here.
    let mut mirrored: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    let mut created = 0usize;

    for (row, parent) in order_folders_parent_first(&rows) {
        let quip_id = row.quip_folder_id.as_str();
        // Fast path: a previous run already mirrored this folder.
        if let Some(existing) = &row.ogre_folder_id {
            mirrored.insert(quip_id, existing.clone());
            continue;
        }
        let parent_id = match parent {
            MirrorParent::ImportRoot => destination.clone(),
            // `unwrap_or` rather than an unreachable: the ordering makes the
            // miss impossible, and a stuck import is too high a price for
            // being right about that.
            MirrorParent::Quip(p) => mirrored.get(p).cloned().unwrap_or_else(|| {
                tracing::warn!(
                    import_id,
                    quip_folder_id = quip_id,
                    parent = p,
                    "quip content: mirrored parent missing at creation time; \
                     filing this folder under the import's destination",
                );
                destination.clone()
            }),
        };

        let now = ogrenotes_common::time::now_usec();
        let candidate = ogrenotes_common::id::new_id();
        // Same create + link order as `routes::folders::create_folder` and
        // `ensure_import_folder`: link under the parent first, so the folder
        // is never orphaned, then write metadata + the owner MEMBER row.
        ctx.folder_repo
            .add_child(&FolderChild {
                folder_id: parent_id.clone(),
                child_id: candidate.clone(),
                child_type: ChildType::Folder,
                added_at: now,
            })
            .await
            .map_err(|e| format!("link mirrored folder {quip_id}: {e}"))?;
        ctx.folder_repo
            .create(&Folder {
                folder_id: candidate.clone(),
                // Quip's own name for it. Empty titles do occur; "Untitled"
                // matches what the content pass names an untitled document,
                // so the two read as one import.
                title: if row.title.trim().is_empty() { "Untitled" } else { &row.title }
                    .to_string(),
                color: 0,
                parent_id: Some(parent_id),
                owner_id: owner_id.to_string(),
                folder_type: FolderType::User,
                inherit_mode: ogrenotes_storage::models::InheritMode::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .map_err(|e| format!("create mirrored folder {quip_id}: {e}"))?;

        let recorded = ctx
            .import_repo
            .record_ogre_folder(import_id, quip_id, &candidate)
            .await
            .map_err(|e| format!("record mirrored folder {quip_id}: {e}"))?;
        if recorded != candidate {
            tracing::info!(
                import_id,
                quip_folder_id = quip_id,
                "quip content: another run recorded this folder first; adopting it",
            );
        }
        mirrored.insert(quip_id, recorded);
        created += 1;
    }

    tracing::info!(
        import_id,
        folders = rows.len(),
        created,
        "quip content: Quip folder tree mirrored under the import's destination",
    );
    Ok(())
}

/// Phase 2 content pass: turn every `Pending` thread into a document.
///
/// Resumable by construction — the per-thread `ContentDone` checkpoint is the
/// unit of progress, so a retry re-lists the manifest and skips finished
/// threads *before* spending a Quip request on them.
///
/// **One bad thread must not cost the import.** The dispositions are:
///
/// | condition | disposition |
/// |---|---|
/// | 401 | run-terminal: status `TokenRejected`, `Ok(())` |
/// | 403 on a thread | `Skipped` + report note, **continue** (#141) |
/// | transient, under budget | charge an attempt, **continue**, `Err` at the end |
/// | transient, over budget | `Failed` + report note, **continue** (#142) |
/// | storage / S3 / rate limit | run-terminal: `Err`, queue retries |
///
/// The load-bearing subtlety is *why the pass continues after an under-budget
/// transient failure instead of returning `Err` on the spot*, which is what it
/// did before #142. The queue gives this job four runs ([`MAX_RETRIES`] plus
/// the first attempt), and an abort-on-first-bad-thread run advances exactly
/// **one** thread's attempt counter per run. One bad thread would resolve in
/// three runs; *two* would need five, and the import would dead-letter with
/// the second one still `Pending` — #142 renamed rather than fixed. By
/// continuing, a single run charges an attempt to *every* bad thread it meets,
/// so any number of them resolve within [`MAX_THREAD_ATTEMPTS`] runs and the
/// job-level budget is never the binding constraint. The `Err` still happens,
/// just at the end of the pass, so "a transient failure retries the job" is
/// preserved exactly.
///
/// The counterweight is [`MAX_CONSECUTIVE_THREAD_FAILURES`]: continuing is
/// right when the bad threads are scattered, and wrong when *everything* is
/// failing, so a run of back-to-back **first-time** transient failures stops
/// the pass instead of charging an attempt to the whole manifest during a Quip
/// outage.
///
/// ## Why the breaker counts only *first-time* failures
///
/// A naive "any consecutive failure trips it" breaker reintroduces the
/// dead-letter it exists to prevent. With a run of `N` sort-adjacent
/// deterministic failures, the breaker trips at offset
/// `MAX_CONSECUTIVE_THREAD_FAILURES` **every run** and returns `Err` before
/// the walk ever reaches thread `N`; threads past the trip point never get a
/// first attempt, so they never climb to `Failed`, and the job dead-letters
/// with them still `Pending`. It is #142 in miniature — bounded to `N > 5`
/// instead of `N > 1`, but the same bug.
///
/// Counting only a thread's *first* failure fixes it. A known-bad thread
/// (`attempts > 1`) is already climbing toward its own give-up — forward
/// progress, not new outage evidence — so it no longer re-arms the breaker.
/// The breaker therefore fires only on the **leading edge** of not-yet-charged
/// threads, and each run that edge advances by up to
/// `MAX_CONSECUTIVE_THREAD_FAILURES` threads while the ones behind it climb in
/// parallel. Concretely, with the constants here (`MAX_THREAD_ATTEMPTS = 3`,
/// four job runs, threshold 5) an adjacent cluster of up to
/// `2 * MAX_CONSECUTIVE_THREAD_FAILURES` threads is fully marked `Failed`
/// inside the budget and the import *completes*.
///
/// **The two invariants this preserves:**
///
/// - *Progress.* Every deterministically-failing thread within
///   `2 * MAX_CONSECUTIVE_THREAD_FAILURES` positions of a success is charged
///   to give-up within the job budget, so no realistic bad-thread cluster
///   dead-letters the import.
/// - *Outage containment.* In a Quip-wide outage nothing succeeds, so the
///   walk never advances past its leading edge — those threads are always on
///   their *first* failure — and the breaker still trips after
///   `MAX_CONSECUTIVE_THREAD_FAILURES` fresh failures **every run**, bounding
///   per-run Quip calls to ≈ threshold rather than the whole manifest. This is
///   the property the fix had to not break: first-time-only does **not** blind
///   the breaker to a sustained outage, precisely because a tripping breaker
///   never lets the walk get far enough ahead for the deep threads to reach a
///   second attempt.
///
/// The residual: an adjacent run of more than `2 *
/// MAX_CONSECUTIVE_THREAD_FAILURES` deterministically-failing threads still
/// exhausts the job budget and dead-letters — but that is a *retriable*
/// dead-letter over threads that genuinely fail, strictly better than the
/// pre-fix behavior and categorically better than marking a manifest of good
/// documents `Failed` over a transient outage. Resolving it fully is not
/// possible within a fixed retry budget: charging a huge cluster's attempts is
/// byte-for-byte the same operation as walking an outage, so no in-pass signal
/// can separate them — the only lever is how large a cluster the pass spends
/// resources resolving before it concludes "outage" and bails.
async fn run_content_pass(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
    client: &QuipClient,
    token: &QuipToken,
    record: &ogrenotes_storage::models::import::ImportRecord,
) -> Result<(), String> {
    // The runner lease is refreshed by `LeaseHeartbeat` on a wall-clock timer
    // for the whole handler, so this loop deliberately does no heartbeating of
    // its own — tying liveness to a per-N-threads tick made one slow thread
    // look like a dead worker.
    // Mirror the Quip folder tree, then read the mapping it just recorded.
    // In this order and adjacent, because the second reads exactly what the
    // first writes; a run that mirrored after mapping would file its first
    // pass of documents flat and — since a re-run never moves a filed
    // document — leave them that way permanently.
    mirror_folder_tree(ctx, import_id, owner_id, record).await?;
    let folders = build_folder_mapping(ctx, import_id, record).await?;

    let mut threads = ctx
        .import_repo
        .list_threads(import_id)
        .await
        .map_err(|e| format!("list threads: {e}"))?;
    // Stable order so a retry walks the manifest the same way twice and the
    // logs of two runs line up. DynamoDB query order is already SK order, but
    // sorting makes that a guarantee rather than an incidental property.
    threads.sort_by(|a, b| a.quip_thread_id.cmp(&b.quip_thread_id));

    // Set by the first under-budget transient failure and returned as `Err`
    // once the whole manifest has been walked, so the queue still retries the
    // job without the remaining threads paying for this one.
    let mut retry_after_pass: Option<String> = None;
    // Reset by any thread that reaches a *decided* outcome — imported,
    // skipped, or given up on. Only unresolved failures accumulate.
    let mut consecutive_failures: usize = 0;
    // Person-mention identities, memoized across the whole manifest: the
    // same handful of colleagues is mentioned throughout a real import, and
    // Quip's rate limit does not tolerate re-asking per document.
    let mut people = PersonDirectory::default();

    for thread in &threads {
        // A panic in the per-thread work is contained HERE, not at
        // `execute_and_finalize`. The document body is authored entirely by
        // Quip and the walker that reads it (`from_quip_html`, plus the yrs
        // `materialize` behind it) is the newest parser in the crate, so one
        // malformed document must not be able to abort the pass and
        // dead-letter the import — that is #142's shape arriving through a
        // different door, and the only door `catch_unwind` at the job level
        // cannot close.
        //
        // Mapping the panic to `Transient` rather than swallowing it keeps the
        // bump-after-an-observed-failure invariant exactly as it was: a panic
        // *is* an observed, thread-attributable failure, so it costs the
        // thread one attempt and a genuinely poisonous document is marked
        // `Failed` after [`MAX_THREAD_ATTEMPTS`] while the rest of the import
        // completes. If instead the panic source is global (a yrs regression,
        // say), every thread panics, and
        // [`MAX_CONSECUTIVE_THREAD_FAILURES`] trips and retries the job
        // rather than condemning the whole manifest — the two guards compose.
        let outcome = std::panic::AssertUnwindSafe(import_one_thread(
            ctx, import_id, owner_id, client, token, thread, &folders, &mut people,
        ))
        .catch_unwind()
        .await
        .unwrap_or_else(|panic| {
            // The payload goes to the log only. It is arbitrary runtime data
            // — plausibly a slice of the document being parsed — and
            // `ThreadImportError::transient` will not accept it.
            tracing::error!(
                import_id,
                thread = %thread.quip_thread_id,
                panic = %panic_message(&panic),
                "quip content: thread import panicked; charged to the thread, not the import",
            );
            Err(ThreadImportError::transient(PANIC_REASON))
        });
        match outcome {
            Ok(()) => consecutive_failures = 0,
            Err(ThreadImportError::TokenRejected) => {
                ctx.import_repo
                    .set_status(import_id, ImportStatus::TokenRejected)
                    .await
                    .ok();
                tracing::warn!(
                    import_id,
                    thread = %thread.quip_thread_id,
                    "quip content: credential rejected (401); TokenRejected",
                );
                return Ok(());
            }
            // #141: one inaccessible document, not a dead credential. Skip it
            // by name and keep importing — a retry could never widen the
            // user's Quip permissions, so there is nothing to retry.
            Err(ThreadImportError::Forbidden(reason)) => {
                consecutive_failures = 0;
                ctx.import_repo
                    .set_thread_skipped(import_id, &thread.quip_thread_id, &reason)
                    .await
                    .map_err(|e| format!("skip forbidden thread: {e}"))?;
                record_report(
                    ctx,
                    import_id,
                    owner_id,
                    report::THREADS_SKIPPED_FORBIDDEN,
                    Some(ReportNote {
                        quip_thread_id: thread.quip_thread_id.clone(),
                        kind: report::KIND_THREAD_SKIPPED.to_string(),
                        detail: reason.clone(),
                    }),
                )
                .await;
                tracing::info!(
                    import_id,
                    thread = %thread.quip_thread_id,
                    "quip content: thread skipped (403); the import continues",
                );
            }
            // Not this thread's fault. Unchanged from before #141: stop and
            // let the queue retry the job.
            Err(ThreadImportError::RunFailure(msg)) => {
                tracing::warn!(
                    import_id,
                    thread = %thread.quip_thread_id,
                    error = %msg,
                    "quip content: run-level failure; will retry",
                );
                return Err(format!("quip content transient error: {msg}"));
            }
            Err(ThreadImportError::Transient(reason)) => {
                // The counter lives on the `THREAD#` row (an atomic DynamoDB
                // `ADD`), not in this loop, so it survives a worker restart,
                // a lease takeover, and the queue's own redelivery.
                let attempts = ctx
                    .import_repo
                    .bump_thread_attempts(import_id, &thread.quip_thread_id)
                    .await
                    .map_err(|e| format!("bump thread attempts: {e}"))?;
                if attempts >= MAX_THREAD_ATTEMPTS {
                    // #142: give up on the thread, NOT on the import. This
                    // path deliberately does not return `Err`, so the job's
                    // own retry budget is untouched and the pass can still
                    // finish — "999 documents and a report line" rather than
                    // "nothing after t0042".
                    consecutive_failures = 0;
                    let detail = format!("{reason}; gave up after {attempts} attempts");
                    ctx.import_repo
                        .set_thread_failed(import_id, &thread.quip_thread_id, &detail)
                        .await
                        .map_err(|e| format!("fail thread: {e}"))?;
                    record_report(
                        ctx,
                        import_id,
                        owner_id,
                        report::THREADS_FAILED,
                        Some(ReportNote {
                            quip_thread_id: thread.quip_thread_id.clone(),
                            kind: report::KIND_THREAD_FAILED.to_string(),
                            detail,
                        }),
                    )
                    .await;
                    tracing::warn!(
                        import_id,
                        thread = %thread.quip_thread_id,
                        attempts,
                        error = %reason,
                        "quip content: thread failed too many times; marked Failed and skipped",
                    );
                } else {
                    retry_after_pass.get_or_insert(format!(
                        "thread {} failed ({reason}); attempt {attempts} of {MAX_THREAD_ATTEMPTS}",
                        thread.quip_thread_id,
                    ));
                    // Only a thread's FIRST-EVER failure arms the breaker. A
                    // thread we have already seen fail (`attempts > 1`) is
                    // known-bad and climbing toward its own give-up — it is
                    // making forward progress, not evidence that the outage is
                    // spreading — so re-counting it would let a cluster of bad
                    // threads keep re-tripping the breaker at the same point
                    // and starve every thread past it of attempts. That is the
                    // dead-letter this branch was reworked to close: see
                    // `run_content_pass`'s doc for the arithmetic and the
                    // outage invariant that survives it.
                    if attempts == 1 {
                        consecutive_failures += 1;
                        tracing::warn!(
                            import_id,
                            thread = %thread.quip_thread_id,
                            attempts,
                            error = %reason,
                            "quip content: thread failed for the first time; will retry it",
                        );
                        if consecutive_failures >= MAX_CONSECUTIVE_THREAD_FAILURES {
                            tracing::warn!(
                                import_id,
                                consecutive_failures,
                                "quip content: too many first-time thread failures in a row; \
                                 treating this as a Quip-wide outage and retrying the job",
                            );
                            return Err(format!(
                                "quip content transient error: {consecutive_failures} consecutive \
                                 first-time thread failures (last: {reason})",
                            ));
                        }
                    } else {
                        // Known-bad, still under budget: does not arm the
                        // breaker, but does not disarm it either (leaving
                        // `consecutive_failures` untouched), so a run of fresh
                        // failures on either side of it still accumulates.
                        tracing::warn!(
                            import_id,
                            thread = %thread.quip_thread_id,
                            attempts,
                            error = %reason,
                            "quip content: known-bad thread failed again; will retry it",
                        );
                    }
                }
            }
        }
    }

    // Every thread was reached, but at least one is still `Pending` and under
    // its attempt budget. Fail the job so the queue retries it; the re-run
    // skips everything already `ContentDone`/`Skipped`/`Failed` without a
    // single Quip call and picks up exactly where this one left off.
    if let Some(reason) = retry_after_pass {
        tracing::warn!(import_id, reason = %reason, "quip content: pass incomplete; will retry");
        return Err(format!("quip content transient error: {reason}"));
    }

    ctx.import_repo
        .set_phase(import_id, 2)
        .await
        .map_err(|e| format!("set phase: {e}"))?;
    // Terminal success. Written AFTER the phase bump so the strongest claim
    // lands last, and written at all because otherwise a finished import and
    // a *stranded* one are the same record state (`Running`, phase 1-or-2) —
    // which makes any recovery sweep over `Running` imports unwriteable, and
    // leaves the wizard's "done" signal resting on `phase` alone. The wizard
    // already terminates on `phase >= 2` and treats only
    // `failed`/`tokenrejected`/`cancelled` as terminal *failures*, so
    // `succeeded` is additive there.
    ctx.import_repo
        .set_status(import_id, ImportStatus::Succeeded)
        .await
        .map_err(|e| format!("set succeeded: {e}"))?;
    // Import-terminal, and only here: every earlier exit from this pass leaves
    // the import re-runnable (a returned `Err` is retried by the queue, a
    // `TokenRejected` resumes after a reconnect), and the staged HTML is the
    // in-flight run's diagnostic material until the run is over. Deliberately
    // AFTER the `Succeeded` write, and only if that write landed: a status
    // this pass could not record is a pass the queue will retry.
    cleanup_quip_staging(ctx, import_id).await;
    tracing::info!(import_id, threads = threads.len(), "quip content: phase 2 complete");
    Ok(())
}

/// Import one Quip thread into one OgreNotes document.
///
/// Ordered so that **every** failure leaves the thread retryable: nothing is
/// checkpointed until the document, its section map and its unresolved links
/// are all durable, and the checkpoint itself is the last write. A retry that
/// lands after a partial run redoes the work from the fetch.
///
/// Redoing it is safe — and, critically, *duplicate-free* — because the
/// document id is **reserved on the `THREAD#` row before the first
/// `DocRepo::create`**. Steps 8, 9 and 10 (section map, unresolved links,
/// checkpoint) all return `Transient` on failure while the thread is still
/// `Pending`, so retrying them is the ordinary case, not an exotic one: a
/// single DynamoDB throttle would otherwise mint a fresh id and hand the user
/// a second copy of the same document (up to four across `MAX_RETRIES`).
/// With the reservation, the retry adopts its own earlier document
/// ([`OnExistingDoc::ReconcileReservedId`]) and finishes the tail. The plan's
/// invariant — never two documents for one thread — is therefore upheld for
/// transient failures *and* for a hard crash, not just the latter.
///
/// Still orphaned by a crash: side-loaded blobs written under a `doc_id`
/// whose reservation itself never landed. Cheap and bounded; S3 lifecycle
/// work is tracked separately.
///
/// `pub` for the same reason [`execute_import_docx`] is: it's the test seam
/// for driving one thread without a consumer loop.
///
/// `people` is the pass-wide identity cache for person mentions; it is
/// `&mut` because resolving one thread's mentions makes the next thread's
/// cheaper (see [`PersonDirectory`]).
#[allow(clippy::too_many_arguments)]
pub async fn import_one_thread(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
    client: &QuipClient,
    token: &QuipToken,
    thread: &ThreadRow,
    folders: &FolderMapping,
    people: &mut PersonDirectory,
) -> Result<(), ThreadImportError> {
    use ogrenotes_storage::models::import_inventory::{
        PendingLinkItem, SecMapRow, UnresolvedRow, SECMAP_CHUNK_ENTRIES,
    };

    // 1. Already done (or deliberately skipped, or given up on) — no refetch.
    //    This is the resumability guarantee, and it must come before any
    //    network call.
    if thread.state != ThreadState::Pending {
        return Ok(());
    }

    // 2. Disposition by Quip thread type.
    let doc_type = match thread.thread_type.as_str() {
        "chat" => {
            ctx.import_repo
                .set_thread_skipped(import_id, &thread.quip_thread_id, "chat thread")
                .await
                .map_err(|e| ThreadImportError::RunFailure(format!("skip thread: {e}")))?;
            // Counted but not *named*: chats are an expected bulk category, and
            // a chat-heavy import would otherwise spend the whole
            // `thread_skipped` note budget on them and name none of the
            // documents it actually lost.
            record_report(ctx, import_id, owner_id, report::THREADS_SKIPPED_CHAT, None).await;
            tracing::info!(
                import_id,
                thread = %thread.quip_thread_id,
                "quip content: chat thread skipped (chats are not documents)",
            );
            return Ok(());
        }
        "spreadsheet" => DocType::Spreadsheet,
        _ => DocType::Document,
    };

    // 3. Fetch the section-id-bearing `/2` HTML.
    let html = client.thread_html(token, &thread.quip_thread_id).await?;

    // 4. Stage the raw HTML, so a conversion bug hit part-way through a long
    //    import can be diagnosed — and re-run — without going back to Quip and
    //    its per-minute rate budget. Kept for the life of the *import*, not
    //    forever: this is the user's full document text, so the prefix is
    //    swept the moment the import is terminal (see
    //    [`cleanup_quip_staging`]; issue #196). Nothing reads it back — a
    //    resumed pass skips on the `THREAD#` row's `ContentDone` checkpoint
    //    and re-fetches anything still `Pending` — so its lifetime is purely
    //    a retention decision.
    let staged_key = format!("{}{}.html", quip_staging_prefix(import_id), thread.quip_thread_id);
    ctx.s3
        .put_object(&staged_key, html.clone().into_bytes())
        .await
        .map_err(|e| ThreadImportError::RunFailure(format!("stage html: {e}")))?;

    // 5. Walk it, telling the walker which kind of thread this is.
    //
    // Not a hint — the walker cannot work it out for itself. Quip renders a
    // spreadsheet as a table wrapped in the grid's own rulers (a column-letter
    // `<thead>`, a row-number gutter column), and it wraps ordinary document
    // tables in the very same markup, so the body alone does not say which is
    // which. Only `thread_type` does, and it arrives here rather than in the
    // HTML (#230).
    //
    // What that decides is now just the `<thead>`: a document's is real
    // headings and a sheet's is `A B C …`, and nothing in the markup tells
    // them apart. The gutter column needs no such help and is stripped on
    // either kind — no author can produce an id-less cell (#232).
    let mut quip_doc = ogrenotes_collab::import_quip::from_quip_html_as(
        &html,
        match doc_type {
            DocType::Spreadsheet => ogrenotes_collab::import_quip::QuipThreadKind::Spreadsheet,
            _ => ogrenotes_collab::import_quip::QuipThreadKind::Document,
        },
    );

    // Nesting deeper than the walker will descend is a named loss, reported
    // like a dropped image rather than passed over in silence. The cap itself
    // is a process-liveness bound — see `import_quip::MAX_NESTING_DEPTH`; an
    // uncapped walk on third-party HTML exhausts the stack, and stack
    // exhaustion aborts the whole worker rather than unwinding.
    if quip_doc.deep_nesting_truncated > 0 {
        record_report(
            ctx,
            import_id,
            owner_id,
            report::THREADS_TRUNCATED,
            Some(ReportNote {
                quip_thread_id: thread.quip_thread_id.clone(),
                kind: report::KIND_CONTENT_TRUNCATED.to_string(),
                detail: format!(
                    "nesting deeper than {} levels was flattened in {} place(s); \
                     the text was kept, the structure below that point was not",
                    ogrenotes_collab::import_quip::MAX_NESTING_DEPTH,
                    quip_doc.deep_nesting_truncated,
                ),
            }),
        )
        .await;
    }

    // Two losses the walker knows it took, and that a reader of the imported
    // document cannot possibly infer (#191, #192). Both differ from every
    // other loss reported here in one way that matters: the data *is* in the
    // export. A dropped image is a fetch that failed; these are content this
    // importer chose not to carry, and until it does, the only honest thing
    // is to say so.
    //
    // `detail` names what was lost and where, never what it contained — a
    // board's card titles and a formula's text are document content, and
    // this string is durable and user-visible (`safe_quip_reason`'s rule).
    if quip_doc.live_apps_dropped > 0 {
        record_report_by(
            ctx,
            import_id,
            owner_id,
            report::LIVE_APPS_DROPPED,
            quip_doc.live_apps_dropped as u64,
            Some(ReportNote {
                quip_thread_id: thread.quip_thread_id.clone(),
                kind: report::KIND_LIVE_APP_DROPPED.to_string(),
                detail: format!(
                    "{} embedded Quip live app(s) — a Kanban board or similar — could not be \
                     converted; whatever the block displayed in Quip is not in the imported \
                     document",
                    quip_doc.live_apps_dropped,
                ),
            }),
        )
        .await;
    }
    if quip_doc.formulas_dropped > 0 {
        record_report_by(
            ctx,
            import_id,
            owner_id,
            report::FORMULAS_DROPPED,
            quip_doc.formulas_dropped as u64,
            Some(ReportNote {
                quip_thread_id: thread.quip_thread_id.clone(),
                kind: report::KIND_FORMULAS_DROPPED.to_string(),
                detail: format!(
                    "{} spreadsheet formula(s) were not imported; the cells keep the values \
                     Quip had last calculated and will not recalculate",
                    quip_doc.formulas_dropped,
                ),
            }),
        )
        .await;
    }

    // 6. Settle this thread's document id BEFORE anything durable is written
    //    under it. It has to be known early anyway — every side-loaded blob
    //    key embeds it and those keys go into the snapshot — but it is also
    //    *reserved* on the `THREAD#` row so a retry re-uses it instead of
    //    minting a second document for the same thread. A row that already
    //    carries one is a previous attempt's reservation; reuse it verbatim.
    let doc_id = match &thread.ogre_doc_id {
        Some(reserved) => reserved.clone(),
        None => ctx
            .import_repo
            .reserve_thread_doc_id(
                import_id,
                &thread.quip_thread_id,
                &ogrenotes_common::id::new_id(),
            )
            .await
            .map_err(|e| ThreadImportError::RunFailure(format!("reserve doc id: {e}")))?,
    };
    let src_updates = sideload_images(
        ctx,
        import_id,
        owner_id,
        client,
        token,
        thread,
        &doc_id,
        &quip_doc.images,
    )
    .await?;
    ogrenotes_collab::blob_ref::set_image_srcs(&quip_doc.doc, &src_updates);

    // 6b. Give every person mention an identity — or a name.
    //
    //     The walker leaves each `<control>`-wrapped mention as an
    //     unfinished `Mention` leaf carrying the *Quip* person id; only a
    //     network + DynamoDB round trip can turn that into an OgreNotes
    //     user, so it happens here. `resolve_person_mentions` is total over
    //     the document, which is why this runs unconditionally on any
    //     thread that had mentions: a chip that reached the snapshot still
    //     pointing at a Quip id would be a mention of nobody.
    //
    //     This must land BEFORE the snapshot is taken in step 7.
    let mention_count = quip_doc.person_mentions.len();
    if mention_count > 0 {
        let wanted: Vec<String> =
            quip_doc.person_mentions.iter().map(|m| m.quip_user_id.clone()).collect();
        let outcome = people.resolve(ctx, import_id, client, token, &wanted).await;
        // The lookup endpoint just proved unusable for the whole run. Say so
        // once, durably — a `tracing::warn!` is invisible to the person who
        // ran the import, and this degrades every mention in every remaining
        // document. Written before the bail below so it lands even if the
        // same call also produced a fault.
        if outcome.endpoint_just_died {
            record_report(
                ctx,
                import_id,
                owner_id,
                report::THREADS_MENTIONS_DEGRADED,
                Some(ReportNote {
                    quip_thread_id: thread.quip_thread_id.clone(),
                    kind: report::KIND_MENTIONS_DEGRADED.to_string(),
                    detail: "the Quip person-lookup endpoint rejected this import's requests; \
                             @mentions from here on are imported as plain text names rather \
                             than links, in this and every remaining document"
                        .to_string(),
                }),
            )
            .await;
        }
        // A person we could not even *decide* about must not be checkpointed.
        // Step 10 marks the thread `ContentDone` unconditionally and a re-run
        // skips a `ContentDone` thread with zero Quip calls, so a Quip 5xx or
        // a DynamoDB blip lasting seconds would otherwise degrade every
        // mention in every thread it touched — permanently, and with no
        // report note to make the loss discoverable. Bail *before* the
        // snapshot in step 7: the thread stays `Pending` and the existing
        // per-thread attempt budget bounds the retries. A person genuinely
        // *without* an account is a decision, not a failure, and does not
        // come through here — it degrades permanently and silently, which is
        // correct.
        if let Some(fault) = outcome.fault {
            tracing::warn!(
                import_id,
                thread = %thread.quip_thread_id,
                mentions = mention_count,
                fault = ?fault,
                "quip content: person lookup undecided; retrying rather than checkpointing \
                 the thread with unrecoverable placeholders",
            );
            return Err(fault.into_thread_error());
        }
        let rewritten = ogrenotes_collab::import_quip::resolve_person_mentions(
            &quip_doc.doc,
            &outcome.decided,
        );
        // A chip Quip does not know as a person was a `<control>`-wrapped
        // folder or thread link, and is now a `DocMention`. Its back-patch
        // record has to join the walker's own before step 9 writes the row,
        // or Phase 2b will never see it.
        let doc_links = rewritten.doc_links.len();
        quip_doc.pending_links.extend(rewritten.doc_links);
        // Every mention that lost its link is a named loss in the user's
        // report, exactly as a dropped image is. Counted per *document* so a
        // chatty one cannot dominate; the systemic cause, when there is one,
        // is the note written above.
        // `!endpoint_just_died` because the note written above already bumped
        // this counter for this thread, and the counter counts documents.
        if rewritten.degraded > 0 && !outcome.endpoint_just_died {
            record_report(ctx, import_id, owner_id, report::THREADS_MENTIONS_DEGRADED, None).await;
        }
        // Counts only. A mention's *subject* — their name, and above all
        // the email the match was made on — is not something this worker
        // writes to a log or to any durable row.
        tracing::info!(
            import_id,
            thread = %thread.quip_thread_id,
            mentions = mention_count,
            degraded = rewritten.degraded,
            doc_links,
            "quip content: person mentions resolved",
        );
    }

    // 7. Persist through the ordinary document-creation path, preserving
    //    Quip's timestamps so an old document doesn't arrive looking new.
    let snapshot = ogrenotes_collab::snapshot::doc_to_bytes(&quip_doc.doc);
    let (folder_id, additional_folder_ids) = folders.folders_for(thread);
    let title = if thread.title.trim().is_empty() { "Untitled" } else { thread.title.as_str() };
    persist_imported_document(
        &ctx.doc_repo,
        &ctx.folder_repo,
        &snapshot,
        title,
        owner_id,
        &folder_id,
        doc_type,
        &additional_folder_ids,
        thread.updated_usec,
        thread.updated_usec,
        &doc_id,
        OnExistingDoc::ReconcileReservedId,
    )
    .await
    .map_err(ThreadImportError::RunFailure)?;

    // 8. Section map, chunked — a thread with thousands of anchors would
    //    otherwise blow DynamoDB's per-item size cap.
    for (chunk_index, entries) in quip_doc.sections.chunks(SECMAP_CHUNK_ENTRIES).enumerate() {
        ctx.import_repo
            .put_secmap(
                import_id,
                &SecMapRow {
                    quip_thread_id: thread.quip_thread_id.clone(),
                    chunk: chunk_index as u32,
                    owner_id: owner_id.to_string(),
                    entries: entries.to_vec(),
                },
            )
            .await
            .map_err(|e| ThreadImportError::RunFailure(format!("put secmap: {e}")))?;
    }

    // 9. Cross-thread links, for Phase 2b to back-patch. A link-free thread
    //    writes no row at all rather than an empty one.
    if !quip_doc.pending_links.is_empty() {
        ctx.import_repo
            .put_unresolved(
                import_id,
                &UnresolvedRow {
                    source_quip_thread_id: thread.quip_thread_id.clone(),
                    owner_id: owner_id.to_string(),
                    links: quip_doc
                        .pending_links
                        .iter()
                        .map(|l| PendingLinkItem {
                            source_block_id: l.source_block_id.clone(),
                            target_quip_thread_id: l.target_quip_thread_id.clone(),
                            target_quip_section_id: l.target_quip_section_id.clone(),
                        })
                        .collect(),
                },
            )
            .await
            .map_err(|e| ThreadImportError::RunFailure(format!("put unresolved: {e}")))?;
    }

    // 10. Checkpoint last — this is what a re-run reads to skip the thread.
    ctx.import_repo
        .set_thread_content_done(import_id, &thread.quip_thread_id, &doc_id, &staged_key)
        .await
        .map_err(|e| ThreadImportError::RunFailure(format!("checkpoint thread: {e}")))?;

    record_report(ctx, import_id, owner_id, report::THREADS_IMPORTED, None).await;

    tracing::info!(
        import_id,
        thread = %thread.quip_thread_id,
        doc_id,
        images = quip_doc.images.len(),
        sections = quip_doc.sections.len(),
        pending_links = quip_doc.pending_links.len(),
        "quip content: thread imported",
    );
    Ok(())
}

/// Copy every image the thread references out of Quip and into this
/// document's blob prefix, returning the `blockId -> new Image.src` map
/// [`ogrenotes_collab::blob_ref::set_image_srcs`] applies.
///
/// One *permanently* unfetchable image must not cost the reader the whole
/// document, so such a failure maps that block to `None` (drop the `src`, keep
/// the node and its `alt`), records the loss on the report, and the pass
/// continues.
///
/// **A failure a later attempt could still recover propagates instead (#155).**
/// Step 10 marks the thread `ContentDone` unconditionally and a re-run skips a
/// `ContentDone` thread with zero Quip calls, so dropping the `src` on a Quip
/// 503 or a timeout checkpointed the document image-less *permanently* — a
/// rate-limit storm silently persisting a migration's worth of pictureless
/// documents while reporting success. [`blob_failure_is_recoverable`] draws the
/// line; the recoverable side returns `Err` before step 7 writes anything, so
/// the thread stays `Pending` and #142's per-thread attempt budget bounds the
/// retries exactly as it does for a failed thread-HTML fetch. This mirrors what
/// [`PersonDirectory::resolve`] already does for an undecided mention, and for
/// the same reason: costing one thread a retry is much cheaper than
/// permanently degrading it.
///
/// The dispositions are `From<QuipError>`'s own rather than a parallel policy —
/// a rate limit is never one thread's fault (`RunFailure`; the queue's backoff
/// waits), a 5xx or a timeout is (`Transient`; charged to the thread).
///
/// Two failures that are *not* recoverable, and where checkpointing is
/// therefore the right answer rather than a bug:
///
/// - **A 403 is an image-drop, not a thread-skip.** It is tempting to treat
///   blob-403 the way thread-403 is treated (#141's table reads that way), but
///   the thread's HTML already came back `200`: the user can have this
///   document, just not this one picture. Skipping the whole thread would make
///   an attachment the user never had permission to see cost them the document
///   they did.
/// - **`Unauthorized` is neither.** That isn't "this image is broken", it's
///   "the credential is dead", and every remaining fetch in the import would
///   fail the same way — so it propagates and stops the run.
// `import_id`/`owner_id` ride along purely so an image drop can be recorded
// on the REPORT row at the point the loss actually happens; same precedent as
// `persist_imported_document` above.
#[allow(clippy::too_many_arguments)]
async fn sideload_images(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
    client: &QuipClient,
    token: &QuipToken,
    thread: &ThreadRow,
    doc_id: &str,
    images: &[ogrenotes_collab::import_quip::QuipImageRef],
) -> Result<std::collections::HashMap<String, Option<String>>, ThreadImportError> {
    let mut updates = std::collections::HashMap::new();
    for image in images {
        let Some((blob_thread_id, blob_id)) = quip_blob_ref(&image.src) else {
            tracing::warn!(
                thread = %thread.quip_thread_id,
                "quip content: image src is not a Quip blob; dropping the src",
            );
            updates.insert(image.block_id.clone(), None);
            drop_image(ctx, import_id, owner_id, thread, "the image was not a Quip attachment")
                .await;
            continue;
        };
        let bytes = match client.blob(token, blob_thread_id, blob_id).await {
            Ok(bytes) => bytes,
            Err(QuipError::Unauthorized) => return Err(ThreadImportError::TokenRejected),
            // #155: a later attempt could still fetch this. Bail BEFORE step 7
            // so the thread is not checkpointed with a loss it can recover;
            // `From<QuipError>` decides who pays for the retry. The blob id is
            // named in the log only — routing the error through `From` rather
            // than hand-building a `Transient` keeps that variant's two-origin
            // leak guarantee (see `ThreadImportError::transient`) intact.
            Err(e) if blob_failure_is_recoverable(&e) => {
                tracing::warn!(
                    thread = %thread.quip_thread_id,
                    blob_id,
                    error = %e,
                    "quip content: blob fetch failed recoverably; the thread stays retryable \
                     rather than checkpointing without the image",
                );
                return Err(e.into());
            }
            Err(e) => {
                tracing::warn!(
                    thread = %thread.quip_thread_id,
                    blob_id,
                    error = %e,
                    "quip content: blob fetch failed permanently; keeping the image's alt \
                     text only",
                );
                updates.insert(image.block_id.clone(), None);
                // `safe_quip_reason`, not `e` — `QuipError::Api` carries
                // Quip's raw response body, and this string becomes a durable,
                // user-visible `ReportNote::detail`.
                let detail = format!("image {blob_id}: {}", safe_quip_reason(&e));
                drop_image(ctx, import_id, owner_id, thread, &detail).await;
                continue;
            }
        };
        let key = format!("blobs/{doc_id}/{blob_id}/{}", blob_filename(&image.alt, blob_id));
        match ctx.s3.put_object(&key, bytes).await {
            Ok(()) => {
                updates.insert(
                    image.block_id.clone(),
                    Some(ogrenotes_collab::blob_ref::blob_ref(blob_id, &key)),
                );
            }
            // Our storage, not Quip's — and #155's shape verbatim: dropping the
            // `src` here checkpointed the document image-less over an S3 blip
            // that lasted seconds. `RunFailure`, matching every other S3/DynamoDB
            // failure in this function (`stage html`, `put secmap`) and
            // `LookupFault::Storage`: the pass aborts, the queue retries the job,
            // and the thread is charged nothing — a storage blip must never spend
            // an innocent thread's attempts.
            Err(e) => {
                tracing::warn!(
                    thread = %thread.quip_thread_id,
                    blob_id,
                    error = %e,
                    "quip content: blob upload failed; the thread stays retryable",
                );
                return Err(ThreadImportError::RunFailure(format!("side-load image: {e}")));
            }
        }
    }
    Ok(updates)
}

/// Whether a `GET /1/blob/…` failure is one a later attempt could still
/// recover — and therefore one that must **not** be checkpointed (#155).
///
/// This is the whole judgement call in #155, because the two mistakes are not
/// symmetric. Treating a recoverable failure as permanent loses the image
/// forever and says nothing a re-run can act on. Treating a *permanent* failure
/// as recoverable is worse: the thread climbs [`MAX_THREAD_ATTEMPTS`] and is
/// marked `Failed`, so a document that would have imported fine-but-imageless
/// does not import at all. The default therefore has to be "permanent", and a
/// class earns `true` only by being genuinely retryable.
///
/// Class by class, for the endpoint [`QuipClient::blob`] actually is:
///
/// - `RateLimited` (Quip's 503) — the definition of "not now, ask again".
/// - `Http` — reqwest could not complete the request: a timeout, a reset
///   connection, a truncated body. Nothing about the blob is implicated.
/// - `Api` 5xx and 429 — Quip failed to answer, or asked us to slow down.
/// - `Api` 4xx (429 aside) — Quip understood the request and refused it; a 404
///   is the common one, an attachment that is simply gone. No retry widens
///   that. Same rule, same reasoning, as [`is_permanent_lookup_failure`].
/// - `Parse` — **deterministically permanent here, and this is the class the
///   issue got wrong.** `blob()` parses nothing: it returns raw bytes, and its
///   only `Parse` producer is `check_blob_size` refusing a body over
///   `MAX_BLOB_BYTES`. A 40 MiB attachment is 40 MiB on the next run too, so
///   retrying it three times and then marking the thread `Failed` would trade a
///   document with one missing picture for no document at all.
/// - `Unauthorized` / `Forbidden` — decided elsewhere in `sideload_images`
///   (run-terminal and image-drop respectively) and never reach here; spelled
///   out anyway so the match stays exhaustive and no future variant can
///   silently inherit either answer.
fn blob_failure_is_recoverable(e: &QuipError) -> bool {
    match e {
        QuipError::RateLimited { .. } | QuipError::Http(_) => true,
        QuipError::Api { status, .. } => *status >= 500 || *status == 429,
        QuipError::Unauthorized | QuipError::Forbidden | QuipError::Parse(_) => false,
    }
}

/// Record one dropped attachment on the report. Advisory, like every other
/// [`record_report`] call — an image that could not be copied must not also
/// take down the document it belonged to.
///
/// `detail` must be caller-authored or [`safe_quip_reason`]-derived text: it
/// is persisted and shown to the user, so a raw `QuipError`/S3 error string
/// (which can carry a response body or a signed URL) must never reach it.
async fn drop_image(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
    thread: &ThreadRow,
    detail: &str,
) {
    record_report(
        ctx,
        import_id,
        owner_id,
        report::IMAGES_DROPPED,
        Some(ReportNote {
            quip_thread_id: thread.quip_thread_id.clone(),
            kind: report::KIND_IMAGE_DROPPED.to_string(),
            detail: detail.to_string(),
        }),
    )
    .await;
}

/// Split a Quip image `src` into `(thread_id, blob_id)`.
///
/// Quip spells an attachment as `/blob/<thread_id>/<blob_id>`, either
/// relative (what the walker sees in a `/2` body) or absolute on a quip.com
/// host. The thread id is read from the `src` rather than assumed to be the
/// containing thread's, because a copied Quip block can reference another
/// thread's blob.
///
/// Returns `None` for anything else — an external image URL, a `data:` URI,
/// or a blob id carrying characters that have no business in an S3 key
/// segment (`/` and `.` in particular, which would let a crafted `src` write
/// outside the document's blob prefix and defeat the ownership check the
/// download route performs on that same prefix).
fn quip_blob_ref(src: &str) -> Option<(&str, &str)> {
    let path = src.split(['?', '#']).next()?;
    let (_, rest) = path.rsplit_once("/blob/")?;
    let (thread_id, blob_id) = rest.split_once('/')?;
    let ok = |s: &str| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    };
    (ok(thread_id) && ok(blob_id)).then_some((thread_id, blob_id))
}

/// Filename segment for a side-loaded blob's S3 key. Quip's `src` carries no
/// filename, so the alt text stands in when it survives the same sanitizing
/// the upload route applies (`routes::documents::request_upload_url`);
/// otherwise the blob id does. Purely cosmetic — the blob is addressed by
/// the full key — but it makes a bucket listing readable.
fn blob_filename(alt: &str, blob_id: &str) -> String {
    let safe: String = alt
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(64)
        .collect();
    if safe.is_empty() || safe.starts_with('.') {
        blob_id.to_string()
    } else {
        safe
    }
}

/// Map a [`QuipError`] hit during the inventory walk to the handler's
/// return disposition. The runner claim is released by the caller's
/// clear-on-every-exit guard in [`execute_start_quip_import`], so this
/// helper only decides status + Ok/Err:
///
/// - `Unauthorized` → the stored token is revoked/expired and won't recover
///   on retry. Flip status to `TokenRejected` and return `Ok(())` — terminal
///   for this run; the UI prompts a reconnect. Returning `Err` here would
///   burn the retry budget hammering Quip with a dead token.
/// - `Forbidden` → the credential is fine; a *selected root* is not readable.
///   Still terminal for this run — `walk_inventory` fails a whole BFS level at
///   once, so there is no per-folder granularity to skip with, and the content
///   pass has nothing to run against. But it is terminal as `Failed`, **not**
///   as `TokenRejected`: telling the user to reconnect a working token is
///   issue #141's misleading diagnosis, and the reconnect wedges on the same
///   folder. A report note names the cause instead.
/// - transient (`RateLimited`/`Http`/`Api`/`Parse`) → return `Err` so the
///   queue's retry/reaper resumes the job. The walk restarts from scratch,
///   which insert-if-absent makes cheap and safe.
///
/// Never logs or formats the token (the `QuipError` variants never carry it).
async fn mark_quip_failure(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
    err: &QuipError,
) -> Result<(), String> {
    match err {
        QuipError::Unauthorized => {
            ctx.import_repo
                .set_status(import_id, ImportStatus::TokenRejected)
                .await
                .ok();
            tracing::warn!(import_id, "quip inventory: credential rejected (401); TokenRejected");
            Ok(())
        }
        QuipError::Forbidden => {
            record_report(
                ctx,
                import_id,
                owner_id,
                report::FOLDERS_FORBIDDEN,
                Some(ReportNote {
                    // No single thread is to blame — the whole BFS level was
                    // refused. The empty id is what "not thread-scoped" looks
                    // like on a `ReportNote`.
                    quip_thread_id: String::new(),
                    kind: report::KIND_THREAD_SKIPPED.to_string(),
                    detail: format!(
                        "{}; a selected folder could not be read, so the import could not be \
                         scoped",
                        safe_quip_reason(err),
                    ),
                }),
            )
            .await;
            ctx.import_repo.set_status(import_id, ImportStatus::Failed).await.ok();
            // Import-terminal (`Failed`, and this run returns `Ok` so the queue
            // acks rather than retries): nothing will re-run this import, so
            // its staged thread HTML — which an earlier run of the same import
            // may well have written before a selected root became unreadable —
            // has no reader left. The sibling `Unauthorized` arm above
            // deliberately does NOT sweep: `TokenRejected` is resumable.
            cleanup_quip_staging(ctx, import_id).await;
            tracing::warn!(
                import_id,
                "quip inventory: a selected root is not readable (403); Failed (not TokenRejected \
                 — the credential is valid, so a reconnect would not help)",
            );
            Ok(())
        }
        transient => {
            tracing::warn!(import_id, error = %transient, "quip inventory: transient failure; will retry");
            Err(format!("quip inventory transient error: {transient}"))
        }
    }
}

/// Fetch thread metadata for every discovered thread in id-batches of
/// [`THREAD_META_CHUNK`], collecting into a `quip_thread_id -> QuipThread`
/// map. Heartbeats the runner lease after each chunk so a long metadata
/// fetch keeps its lease fresh and isn't needlessly reclaimed. Threads with
/// no returned metadata are simply absent (the caller defaults their fields).
async fn fetch_thread_meta(
    client: &QuipClient,
    token: &QuipToken,
    inv: &ogrenotes_quip_import::Inventory,
    ctx: &WorkerCtx,
    import_id: &str,
    instance: &str,
) -> Result<std::collections::HashMap<String, QuipThread>, QuipError> {
    let mut meta = std::collections::HashMap::new();
    let ids: Vec<String> = inv.threads.iter().map(|t| t.quip_thread_id.clone()).collect();
    for chunk in ids.chunks(THREAD_META_CHUNK) {
        for t in client.threads(token, chunk).await? {
            meta.insert(t.id.clone(), t);
        }
        ctx.import_repo
            .heartbeat_runner(import_id, instance, ogrenotes_common::time::now_usec() / 1000)
            .await
            .ok();
    }
    Ok(meta)
}

/// Stable-per-invocation runner identity for the DynamoDB inventory lease
/// (`claim_runner`/`heartbeat_runner`). Host + pid is enough to distinguish
/// two worker tasks on different hosts; a random suffix disambiguates two
/// on the same host (e.g. a dev laptop where HOSTNAME isn't unique).
fn worker_instance_id() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "worker".to_string());
    format!("{host}-{}-{}", std::process::id(), nanoid::nanoid!(6))
}

/// Stable per-task identifier for the consumer-id prefix. ECS sets
/// HOSTNAME to a task arn segment, which is great for log correlation;
/// fall back to a fixed string outside of ECS. The 8-char nanoid
/// suffix prevents two locally-launched workers from colliding when
/// HOSTNAME isn't unique (e.g. on a developer laptop).
fn consumer_prefix() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "worker".to_string());
    format!("{host}-{}", nanoid::nanoid!(8))
}

#[cfg(unix)]
async fn await_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let sigterm = signal(SignalKind::terminate());
    match sigterm {
        Ok(mut s) => tokio::select! {
            _ = s.recv() => tracing::info!("SIGTERM received"),
            _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received"),
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                "could not install SIGTERM handler; only SIGINT will trigger drain",
            );
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn await_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_storage::models::import_inventory::FolderRow;
    use std::collections::BTreeSet;

    fn thread(first: &str, members: &[&str]) -> ThreadRow {
        ThreadRow {
            quip_thread_id: "t1".into(),
            owner_id: "u1".into(),
            title: "Doc".into(),
            thread_type: "document".into(),
            updated_usec: 111,
            member_folders: members.iter().map(|s| s.to_string()).collect(),
            first_folder: first.into(),
            state: ThreadState::Pending,
            ogre_doc_id: None,
            reason: None,
            attempts: 0,
        }
    }

    fn mapping(pairs: &[(&str, &str)], fallback: &str) -> FolderMapping {
        FolderMapping {
            by_quip_id: pairs
                .iter()
                .map(|(q, o)| (q.to_string(), o.to_string()))
                .collect(),
            fallback: fallback.to_string(),
        }
    }

    /// The 8-kind budget is enforced at *runtime*, in `ReportRow::push_note`,
    /// and its penalty is the harshest in the report machinery: the 9th
    /// distinct kind to appear in an import has its notes **discarded
    /// outright**, not truncated, for the life of that import. Which kind
    /// loses depends on the order documents happen to fail in, so the bug
    /// would be silent, intermittent, and unattributable.
    ///
    /// Nothing else fails when a sixth, ninth or twentieth `KIND_*` is added
    /// to `report` — not the compiler, not CI. This is that check. #191 and
    /// #192 took the set from five to seven; a slot remains, and it is one
    /// slot, not a habit.
    ///
    /// It checks what it claims to because `ALL_KINDS` is **derived** from
    /// the constants by `report_keys!`, not typed out beside them: a new kind
    /// that forgot to register itself is not a state this module can be in.
    #[test]
    fn the_worker_stays_within_the_report_rows_note_kind_budget() {
        use ogrenotes_storage::models::import_inventory::REPORT_MAX_NOTE_KINDS;
        let mut sorted = report::ALL_KINDS.to_vec();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "two note kinds share a string: {sorted:?}");
        assert!(
            report::ALL_KINDS.len() <= REPORT_MAX_NOTE_KINDS,
            "{} note kinds against a budget of {REPORT_MAX_NOTE_KINDS}; past it, whichever \
             kind appears 9th in a given import loses every note it writes: {:?}",
            report::ALL_KINDS.len(),
            report::ALL_KINDS,
        );
    }

    /// A kind names an outcome; a counter counts things. They live in
    /// different maps on the `REPORT` row, so an identical string is legal —
    /// and would still be a trap for anyone reading a stored row, where the
    /// two are told apart only by which map they came out of.
    #[test]
    fn no_note_kind_collides_with_a_counter_key() {
        for kind in report::ALL_KINDS {
            assert!(
                !report::ALL_COUNTERS.contains(kind),
                "{kind:?} is both a note kind and a counter key",
            );
        }
        let mut counters = report::ALL_COUNTERS.to_vec();
        counters.sort_unstable();
        let before = counters.len();
        counters.dedup();
        assert_eq!(before, counters.len(), "two counters share a key: {counters:?}");
    }

    /// #155's per-class decision, pinned. The classes that reach
    /// `sideload_images` as a *drop* are the ones no retry can fix; everything
    /// else must leave the thread retryable.
    #[test]
    fn only_a_genuinely_retryable_blob_failure_is_recoverable() {
        assert!(blob_failure_is_recoverable(&QuipError::RateLimited { retry_after_ms: None }));
        assert!(blob_failure_is_recoverable(&QuipError::Api {
            status: 500,
            message: String::new()
        }));
        assert!(blob_failure_is_recoverable(&QuipError::Api {
            status: 503,
            message: String::new()
        }));
        // 429 is a rate limit wearing a different status. `observe_and_check`
        // only maps 503 to `RateLimited`, so this is the arm that catches it.
        assert!(blob_failure_is_recoverable(&QuipError::Api {
            status: 429,
            message: String::new()
        }));

        // Quip understood and refused: the attachment is gone, or the request
        // is wrong. Retrying to a `Failed` thread would cost the document.
        for status in [400, 404, 410, 422] {
            assert!(
                !blob_failure_is_recoverable(&QuipError::Api { status, message: String::new() }),
                "a {status} is a decision, not a delay",
            );
        }

        // The class the issue named as in-scope and that measurably is not.
        // `QuipClient::blob` returns raw bytes and parses nothing; its only
        // `Parse` producer is the `MAX_BLOB_BYTES` refusal, and an oversized
        // attachment is oversized on every future run too.
        assert!(!blob_failure_is_recoverable(&QuipError::Parse("too big".into())));

        // Decided before the classifier is consulted, but never by inheriting
        // the permissive answer.
        assert!(!blob_failure_is_recoverable(&QuipError::Unauthorized));
        assert!(!blob_failure_is_recoverable(&QuipError::Forbidden));
    }

    /// The dispositions #155 hands to `From<QuipError>` are the ones #141/#142
    /// already defined, not a parallel policy — which is what makes the
    /// per-thread attempt bound apply to the new path for free.
    #[test]
    fn a_recoverable_blob_failure_carries_the_existing_dispositions() {
        assert!(matches!(
            ThreadImportError::from(QuipError::RateLimited { retry_after_ms: None }),
            // Never one thread's fault: the queue's backoff waits, and the
            // thread is charged nothing.
            ThreadImportError::RunFailure(_),
        ));
        assert!(matches!(
            ThreadImportError::from(QuipError::Api { status: 500, message: String::new() }),
            // Charged to the thread, so `MAX_THREAD_ATTEMPTS` bounds it.
            ThreadImportError::Transient(_),
        ));
    }

    #[test]
    fn quip_blob_ref_accepts_relative_and_absolute_quip_srcs() {
        assert_eq!(quip_blob_ref("/blob/t1/b9"), Some(("t1", "b9")));
        assert_eq!(
            quip_blob_ref("https://acme.quip.com/blob/THREAD_1/blob-9"),
            Some(("THREAD_1", "blob-9"))
        );
        // Query strings and fragments are not part of the id.
        assert_eq!(quip_blob_ref("/blob/t1/b9?s=200"), Some(("t1", "b9")));
        assert_eq!(quip_blob_ref("/blob/t1/b9#x"), Some(("t1", "b9")));
        // A blob the src names in ANOTHER thread is honored as written — a
        // copied Quip block can legitimately reference one.
        assert_eq!(quip_blob_ref("/blob/other/b1"), Some(("other", "b1")));
    }

    /// The blob id becomes an S3 key segment inside `blobs/{doc_id}/{blob_id}/`,
    /// and the download route authorizes on exactly that prefix — so anything
    /// that could escape it (a slash, a dot segment) must not parse at all.
    #[test]
    fn quip_blob_ref_rejects_non_blob_and_key_escaping_srcs() {
        assert_eq!(quip_blob_ref("https://example.com/cat.png"), None);
        assert_eq!(quip_blob_ref("data:image/png;base64,AAAA"), None);
        assert_eq!(quip_blob_ref(""), None);
        // No blob id segment at all.
        assert_eq!(quip_blob_ref("/blob/t1"), None);
        assert_eq!(quip_blob_ref("/blob/t1/"), None);
        // Path traversal / extra segments in either id.
        assert_eq!(quip_blob_ref("/blob/t1/../../secret"), None);
        assert_eq!(quip_blob_ref("/blob/t1/b9/extra"), None);
        assert_eq!(quip_blob_ref("/blob/../t1/b9"), None);
    }

    #[test]
    fn blob_filename_sanitizes_alt_text_and_falls_back_to_the_blob_id() {
        assert_eq!(blob_filename("pic.png", "b9"), "pic.png");
        assert_eq!(blob_filename("my photo/../x.png", "b9"), "myphoto..x.png");
        // Nothing usable survives sanitizing → the blob id stands in.
        assert_eq!(blob_filename("", "b9"), "b9");
        assert_eq!(blob_filename("///", "b9"), "b9");
        // A leading dot would make a hidden/relative-looking segment.
        assert_eq!(blob_filename(".hidden", "b9"), "b9");
        // Long alt text is truncated rather than becoming an unbounded key.
        assert_eq!(blob_filename(&"a".repeat(200), "b9").len(), 64);
    }

    #[test]
    fn folders_for_maps_quip_folders_and_never_repeats_the_primary() {
        let m = mapping(&[("qf1", "of1"), ("qf2", "of2")], "target");
        let (primary, additional) = m.folders_for(&thread("qf1", &["qf1", "qf2"]));
        assert_eq!(primary, "of1");
        assert_eq!(additional, vec!["of2".to_string()]);
    }

    /// The unmapped fallback, which #236 changed the *meaning* of without
    /// changing the behaviour asserted here.
    ///
    /// This test was written anticipating this work — "pinned so the day
    /// something populates `ogre_folder_id`, the change is visible here" —
    /// and the honest answer is that the change is **not** visible here,
    /// because this exercises `FolderMapping` with an explicitly empty map.
    /// What changed is who reaches that state. It used to be every import
    /// (Phase 1 wrote `ogre_folder_id: None` for every folder, so this was
    /// the whole story and the docs above described it as such); now it is
    /// only a thread naming a folder outside the import's `FOLDER#` set —
    /// a manifest written by an older inventory pass.
    ///
    /// So the assertions stand unchanged and the doc comment is what moves:
    /// filing such a document flat under the destination is still strictly
    /// better than failing an import over one thread's filing. Renamed from
    /// `folders_for_collapses_to_the_target_folder_when_nothing_is_mapped`
    /// because "when nothing is mapped" described the old normal case and now
    /// describes an edge.
    #[test]
    fn an_unmapped_quip_folder_still_files_under_the_imports_destination() {
        let m = mapping(&[], "target");
        let (primary, additional) = m.folders_for(&thread("qf1", &["qf1", "qf2"]));
        assert_eq!(primary, "target");
        assert!(additional.is_empty(), "{additional:?}");
    }

    #[test]
    fn folders_for_dedupes_repeated_member_folders() {
        let m = mapping(&[("qf1", "of1"), ("qf2", "of2"), ("qf3", "of2")], "target");
        let (primary, additional) = m.folders_for(&thread("qf1", &["qf1", "qf2", "qf3", "qf2"]));
        assert_eq!(primary, "of1");
        assert_eq!(additional, vec!["of2".to_string()], "of2 appears once");
    }

    // ─── Ordering the mirrored tree (#236) ───────────────────────

    fn folder_row(quip_id: &str, parent: Option<&str>) -> FolderRow {
        FolderRow {
            quip_folder_id: quip_id.into(),
            owner_id: "u1".into(),
            title: format!("Folder {quip_id}"),
            parent_quip_id: parent.map(str::to_string),
            ogre_folder_id: None,
        }
    }

    /// `(quip id, the quip id of the parent it hangs under, or None for
    /// "directly under the import's destination")`, in creation order.
    fn placements(rows: &[FolderRow]) -> Vec<(&str, Option<&str>)> {
        order_folders_parent_first(rows)
            .into_iter()
            .map(|(row, parent)| {
                let parent = match parent {
                    MirrorParent::ImportRoot => None,
                    MirrorParent::Quip(p) => Some(p),
                };
                (row.quip_folder_id.as_str(), parent)
            })
            .collect()
    }

    /// Hazard 2: a child cannot be created before its parent. The rows come
    /// back from DynamoDB in `SK` order, which is the Quip id's order and
    /// says nothing about depth — so the order has to be re-derived, not
    /// assumed.
    #[test]
    fn a_nested_tree_is_ordered_parent_before_child() {
        // Depth 3, deliberately listed deepest-first so `SK` order is the
        // exact reverse of a usable one.
        let rows = vec![
            folder_row("c", Some("b")),
            folder_row("b", Some("a")),
            folder_row("a", None),
        ];
        assert_eq!(
            placements(&rows),
            vec![("a", None), ("b", Some("a")), ("c", Some("b"))],
        );
    }

    /// Hazard 3, first half: `parent_quip_id` names a folder outside the
    /// selected scope — the user picked a sub-folder as a root, or a shared
    /// folder's parent was never selected.
    ///
    /// **The rule: re-parent to the import's destination, never drop.** A
    /// dropped folder takes its documents' filing with it and the user never
    /// learns which structure went missing; a re-parented one is visibly
    /// there, at worst one level shallower than in Quip.
    #[test]
    fn a_parent_outside_the_selected_scope_re_parents_to_the_import_root() {
        let rows = vec![folder_row("child", Some("never-selected"))];
        assert_eq!(placements(&rows), vec![("child", None)]);
    }

    /// Hazard 3, second half. The inventory walk cannot record a cycle (its
    /// parentage is the BFS tree), but rows written by an older inventory,
    /// or hand-edited, or half-migrated, are not covered by that argument —
    /// and "terminates" is not a property to leave resting on an invariant
    /// enforced in another crate.
    ///
    /// Same rule as an unselected parent: the whole cycle lands under the
    /// import's destination rather than looping or vanishing.
    #[test]
    fn a_cycle_terminates_with_every_folder_under_the_import_root() {
        let rows = vec![
            folder_row("a", Some("b")),
            folder_row("b", Some("a")),
            folder_row("solo", Some("solo")),
        ];
        let placed = placements(&rows);
        assert_eq!(placed.len(), 3, "every row is still placed: {placed:?}");
        assert!(
            placed.iter().all(|(_, parent)| parent.is_none()),
            "a folder in a cycle has no placeable parent: {placed:?}",
        );
    }

    /// The ordering must be total and injective: every row placed, exactly
    /// once. A folder silently dropped here is a folder that never gets
    /// mirrored and never gets reported either.
    #[test]
    fn every_row_is_placed_exactly_once_however_broken_the_graph() {
        let rows = vec![
            folder_row("root", None),
            folder_row("kid", Some("root")),
            folder_row("orphan", Some("gone")),
            folder_row("loop_a", Some("loop_b")),
            folder_row("loop_b", Some("loop_a")),
        ];
        let placed = placements(&rows);
        let ids: BTreeSet<&str> = placed.iter().map(|(id, _)| *id).collect();
        assert_eq!(placed.len(), rows.len(), "no duplicates: {placed:?}");
        assert_eq!(ids.len(), rows.len(), "no drops: {placed:?}");
    }

    /// A placed parent always precedes the child that names it, which is the
    /// property `mirror_folder_tree` reads the parent's freshly-created
    /// OgreNotes id out of an accumulating map on.
    #[test]
    fn a_named_parent_always_precedes_its_child_in_the_order() {
        let rows = vec![
            folder_row("z", Some("m")),
            folder_row("m", Some("a")),
            folder_row("a", None),
            folder_row("b", Some("a")),
        ];
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for (id, parent) in placements(&rows) {
            if let Some(p) = parent {
                assert!(seen.contains(p), "{id} precedes its parent {p}");
            }
            seen.insert(id);
        }
    }
}
