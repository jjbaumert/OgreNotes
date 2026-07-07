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
use ogrenotes_common::config::AppConfig;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
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
}

impl WorkerCtx {
    pub fn new(doc_repo: Arc<DocRepo>, folder_repo: Arc<FolderRepo>, s3: S3Client) -> Self {
        Self { doc_repo, folder_repo, s3 }
    }
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
    let ctx = Arc::new(WorkerCtx::new(
        Arc::new(DocRepo::new(dynamo.clone(), s3.clone())),
        Arc::new(FolderRepo::new(dynamo)),
        s3,
    ));
    tracing::info!("worker mode: persistence context ready");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let consumer_prefix = consumer_prefix();

    let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    for i in 0..config.worker_concurrency.max(1) {
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

/// Execute one claimed job and finalize it: ack on success (then drop the
/// staging blob), or retry / dead-letter on failure per [`MAX_RETRIES`].
/// `pub` so integration tests can drive the real retry budget and the
/// reaper's reclaim→finalize path directly.
pub async fn execute_and_finalize(queue: &JobQueue, claimed: ClaimedJob, ctx: &WorkerCtx) {
    let job_id = claimed.envelope.job_id.clone();
    let attempt = claimed.envelope.attempt;
    let result = execute(ctx, &claimed.envelope.payload).await;
    match result {
        Ok(payload) => match queue.ack(&claimed, payload).await {
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
async fn execute(ctx: &WorkerCtx, payload: &Job) -> Result<Option<String>, String> {
    match payload {
        Job::Noop { label } => {
            tracing::info!(label, "noop executed");
            Ok(Some(
                serde_json::json!({ "label": label }).to_string(),
            ))
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
            Ok(Some(serde_json::json!({ "docId": doc_id }).to_string()))
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
            Ok(Some(serde_json::json!({ "docId": doc_id }).to_string()))
        }
        #[cfg(not(feature = "pdf"))]
        Job::ImportPdf { .. } => Err("PDF import not compiled into this build".into()),
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
        Job::Noop { .. } => return,
    };
    if let Err(e) = ctx.s3.delete_object(s3_key).await {
        tracing::warn!(s3_key, error = %e, "failed to delete import staging blob");
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
    persist_imported_document(doc_repo, folder_repo, &snapshot, title, owner_id, folder).await
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
    persist_imported_document(doc_repo, folder_repo, &snapshot, title, owner_id, folder).await
}

/// Persist a freshly-parsed import as a new document: write the v=1
/// snapshot via the doc repo, then link it into its folder. Mirrors
/// the synchronous `routes::documents::create_from_text` doc-creation
/// shape; the PDF import (M-6.6) reuses this once its parser lands.
async fn persist_imported_document(
    doc_repo: &DocRepo,
    folder_repo: &FolderRepo,
    snapshot: &[u8],
    title: &str,
    owner_id: &str,
    folder_id: &str,
) -> Result<String, String> {
    use ogrenotes_common::id::new_id;
    use ogrenotes_common::time::now_usec;
    use ogrenotes_storage::models::document::DocumentMeta;
    use ogrenotes_storage::models::folder::FolderChild;
    use ogrenotes_storage::models::{ChildType, DocType};

    let doc_id = new_id();
    let now = now_usec();

    let meta = DocumentMeta {
        doc_id: doc_id.clone(),
        title: title.to_string(),
        owner_id: owner_id.to_string(),
        folder_id: Some(folder_id.to_string()),
        additional_folder_ids: Vec::new(),
        workspace_id: None,
        doc_type: DocType::Document,
        snapshot_version: 1,
        snapshot_s3_key: Some(format!("docs/{doc_id}/snapshots/1.bin")),
        is_deleted: false,
        deleted_at: None,
        link_sharing_mode: None,
        link_view_options: ogrenotes_storage::models::ViewOptions::default(),
        locked: false,
        is_template: false,
        created_at: now,
        updated_at: now,
    };
    doc_repo
        .create(&meta, snapshot)
        .await
        .map_err(|e| format!("create document: {e}"))?;

    folder_repo
        .add_child(&FolderChild {
            folder_id: folder_id.to_string(),
            child_id: doc_id.clone(),
            child_type: ChildType::Doc,
            added_at: now,
        })
        .await
        .map_err(|e| format!("link to folder: {e}"))?;

    Ok(doc_id)
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
