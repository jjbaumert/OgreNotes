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
use ogrenotes_storage::models::import_inventory::{FolderRow, ThreadRow, ThreadState};
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
use ogrenotes_storage::repo::import_repo::ImportRepo;
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
        quip_token_store: Arc<dyn TokenStore>,
        quip_base: Option<String>,
    ) -> Self {
        Self { doc_repo, folder_repo, s3, import_repo, quip_token_store, quip_base }
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

    let ctx = Arc::new(WorkerCtx::new(
        Arc::new(DocRepo::new(dynamo.clone(), s3.clone())),
        Arc::new(FolderRepo::new(dynamo)),
        s3,
        import_repo,
        quip_token_store,
        None,
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
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_string());
            tracing::error!(job_id, attempt, panic = %msg, "job execution panicked");
            Err(format!("job execution panicked: {msg}"))
        }
    };
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
        Job::StartQuipImport { import_id, owner_id } => {
            execute_start_quip_import(ctx, import_id, owner_id).await?;
            Ok(Some(serde_json::json!({ "importId": import_id }).to_string()))
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
        // The Quip inventory trigger stages nothing in S3; its durable
        // state is the DynamoDB manifest, cleaned up on the import's own
        // lifecycle, not here.
        Job::StartQuipImport { .. } | Job::Noop { .. } => return,
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
/// `TokenRejected` and the handler returns `Ok(())` rather than burning the
/// retry budget hammering Quip with a dead credential — the UI polls status
/// and prompts a reconnect. Transient errors return `Err` so the queue's
/// retry/reaper resumes the walk from scratch (cheap, thanks to
/// insert-if-absent).
///
/// `pub` so integration tests can drive it directly (the `execute_import_docx`
/// seam precedent) without standing up a full consumer loop.
pub async fn execute_start_quip_import(
    ctx: &WorkerCtx,
    import_id: &str,
    owner_id: &str,
) -> Result<(), String> {
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
        tracing::info!(import_id, "quip inventory: import held by a live runner; skipping");
        return Ok(());
    }

    // From here we OWN the lease. Clear it on EVERY exit — success OR error —
    // so a mid-handler failure never leaves a held claim that would make the
    // queue's retry (running under a *different* instance id) see a live
    // lease, no-op, and get acked as success while the import stays stranded
    // below phase 1. This mirrors what `mark_quip_failure` does for Quip
    // errors, now applied uniformly to DDB-error `?`-returns too.
    let result = run_inventory(ctx, import_id, owner_id, &instance).await;
    ctx.import_repo.clear_runner_claim(import_id).await.ok();
    result
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
        Err(e) => return mark_quip_failure(ctx, import_id, &e).await,
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
        Err(e) => return mark_quip_failure(ctx, import_id, &e).await,
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
    Ok(())
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
/// - transient (`RateLimited`/`Http`/`Api`/`Parse`) → return `Err` so the
///   queue's retry/reaper resumes the job. The walk restarts from scratch,
///   which insert-if-absent makes cheap and safe.
///
/// Never logs or formats the token (the `QuipError` variants never carry it).
async fn mark_quip_failure(
    ctx: &WorkerCtx,
    import_id: &str,
    err: &QuipError,
) -> Result<(), String> {
    match err {
        QuipError::Unauthorized => {
            ctx.import_repo
                .set_status(import_id, ImportStatus::TokenRejected)
                .await
                .ok();
            tracing::warn!(import_id, "quip inventory: token rejected (401/403); TokenRejected");
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
