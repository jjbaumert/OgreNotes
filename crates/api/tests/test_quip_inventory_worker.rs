// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 1 — `StartQuipImport` inventory handler integration tests.
//!
//! Drives `worker_mode::execute_start_quip_import` directly (the `pub`
//! test seam, mirroring `execute_import_docx`) against a real
//! DynamoDB-local plus a wiremock Quip server. Gated on `require_infra!`.
//!
//! Fixture tree (matches the `walk_inventory` unit fixture):
//!   root -> [thread t1, subfolder f2]
//!   f2   -> [thread t1 (shared), thread t2]
//! so inventory discovers 2 folders (root, f2) and 2 threads (t1, t2),
//! with t1 shared across both folders.

mod common;

use std::sync::Arc;

use fred::clients::RedisClient;
use fred::prelude::*;
use ogrenotes_api::worker_mode::{execute_and_finalize, execute_start_quip_import, WorkerCtx};
use ogrenotes_quip_import::QuipToken;
use ogrenotes_storage::models::import::{ImportRecord, ImportStatus};
use ogrenotes_storage::models::import_inventory::{ThreadRow, ThreadState};
use ogrenotes_worker::{Job, JobQueue};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Wiremock Quip server serving `/1/folders/` (per-id) and `/1/threads/`
/// fixtures for the `root -> [t1, f2]`, `f2 -> [t1, t2]` tree.
async fn quip_fixture_server() -> MockServer {
    let server = MockServer::start().await;

    // /1/folders/?ids=root
    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .and(query_param("ids", "root"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "root": {
                "folder": {"id": "root", "title": "Root"},
                "children": [ {"thread_id": "t1"}, {"folder_id": "f2"} ]
            }
        })))
        .mount(&server)
        .await;

    // /1/folders/?ids=f2
    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .and(query_param("ids", "f2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "f2": {
                "folder": {"id": "f2", "title": "Sub"},
                "children": [ {"thread_id": "t1"}, {"thread_id": "t2"} ]
            }
        })))
        .mount(&server)
        .await;

    // /1/threads/ — returns metadata for both threads regardless of the
    // exact `ids` ordering (the handler keys the result by thread id).
    Mock::given(method("GET"))
        .and(path("/1/threads/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "t1": {"thread": {"id": "t1", "title": "Doc A", "type": "document", "updated_usec": 111}},
            "t2": {"thread": {"id": "t2", "title": "Sheet", "type": "spreadsheet", "updated_usec": 222}}
        })))
        .mount(&server)
        .await;

    // Phase 2a widened `StartQuipImport` to run the content pass in the same
    // job (see `test_quip_content_worker.rs` for its own coverage), so an
    // inventory-only fixture is no longer a complete fixture for this job.
    // A trivial body for every thread keeps these tests focused on inventory.
    Mock::given(method("GET"))
        .and(path_regex(r"^/2/threads/.+/html$"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<p>body</p>"))
        .mount(&server)
        .await;

    server
}

/// Wiremock Quip server whose `/1/folders/` always 401s — a revoked token.
async fn quip_unauthorized_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    server
}

/// Wiremock Quip server whose `/1/folders/` always 503s — a transient
/// (rate-limit-class) error that the handler must surface as `Err` for the
/// queue to retry.
async fn quip_transient_error_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    server
}

const CLAIM_STALE_MS: i64 = 30_000; // mirror worker_mode::CLAIM_STALE_MS

fn now_ms() -> i64 {
    ogrenotes_common::time::now_usec() / 1000
}

/// A fresh Redis client + uniquely-named job stream, so the dead-letter test
/// doesn't compete with concurrent tests on the same Redis. Mirrors the helper
/// in `test_worker_mode.rs`.
async fn fresh_queue(suffix: &str) -> JobQueue {
    let config = fred::types::RedisConfig::from_url("redis://127.0.0.1:6379")
        .expect("parse REDIS_URL");
    let client = RedisClient::new(config, None, None, None);
    client.init().await.expect("connect redis");
    let stream = format!("quip-inv-test:{}:{}", suffix, nanoid::nanoid!(6));
    let client = Arc::new(client);
    let _: Result<(), _> = client.del(stream.as_str()).await;
    let _: Result<(), _> = client.del(format!("{stream}:dlq").as_str()).await;
    JobQueue::new(client, stream).await.expect("queue init")
}

/// Seed a `Scoping` import record with the given owner + selected roots,
/// returning its id.
async fn seed_scoping_import(app: &common::TestApp, owner: &str, roots: &[&str]) -> String {
    let import_id = format!("imp-{}", nanoid::nanoid!(8));
    let now = ogrenotes_common::time::now_usec();
    let record = ImportRecord {
        import_id: import_id.clone(),
        owner_id: owner.to_string(),
        status: ImportStatus::Scoping,
        phase: 0,
        quip_user_id: None,
        target_folder_id: Some("target-folder".to_string()),
        selected_roots: roots.iter().map(|s| s.to_string()).collect(),
        created_at: now,
        updated_at: now,
    };
    app.state.import_repo.create(&record).await.expect("seed import record");
    import_id
}

/// Build a `WorkerCtx` from a `TestApp`'s wired repos, pointing the
/// per-import Quip client at the given wiremock base.
fn worker_ctx_with_quip(app: &common::TestApp, quip_base: String) -> WorkerCtx {
    WorkerCtx::new(
        app.state.doc_repo.clone(),
        app.state.folder_repo.clone(),
        app.state.doc_repo.s3().clone(),
        app.state.import_repo.clone(),
        app.state.quip_token_store.clone(),
        Some(quip_base),
    )
}

#[tokio::test]
async fn inventory_walk_persists_folders_and_threads_and_total() {
    common::require_infra!();
    let server = quip_fixture_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1", &["root"]).await;
    app.state
        .quip_token_store
        .put(&import_id, &QuipToken::new("tok".into()))
        .await
        .unwrap();

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    // Threads: t1 + t2 discovered and persisted.
    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let ids: std::collections::BTreeSet<_> =
        threads.iter().map(|t| t.quip_thread_id.clone()).collect();
    assert_eq!(
        ids,
        ["t1", "t2"].iter().map(|s| s.to_string()).collect::<std::collections::BTreeSet<_>>()
    );

    // Metadata carried through from /1/threads/.
    let t1 = threads.iter().find(|t| t.quip_thread_id == "t1").unwrap();
    assert_eq!(t1.title, "Doc A");
    assert_eq!(t1.thread_type, "document");
    assert_eq!(t1.updated_usec, 111);
    assert_eq!(t1.owner_id, "owner1");
    // Phase 2a: the same job now runs the content pass, so the thread this
    // walk enqueued as Pending has already been converted by the time the
    // job returns. Inventory's own contract — that it *discovers* the thread
    // with the right metadata and folder membership — is what's asserted here.
    assert_eq!(t1.state, ThreadState::ContentDone);
    // Shared thread lists both member folders; first_folder is the root.
    assert_eq!(t1.first_folder, "root");
    let mut mf = t1.member_folders.clone();
    mf.sort();
    assert_eq!(mf, vec!["f2".to_string(), "root".to_string()]);

    // Folders: root + f2 persisted.
    let folders = app.state.import_repo.list_folders(&import_id).await.unwrap();
    let fids: std::collections::BTreeSet<_> =
        folders.iter().map(|f| f.quip_folder_id.clone()).collect();
    assert_eq!(
        fids,
        ["f2", "root"].iter().map(|s| s.to_string()).collect::<std::collections::BTreeSet<_>>()
    );

    // Phase advanced + total recorded. Phase 2a: the job continues into the
    // content pass, so it lands on phase 2 with both threads converted.
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.phase, 2);
    let (total, done) = app.state.import_repo.count_threads_by_state(&import_id).await.unwrap();
    assert_eq!((total, done), (2, 2));
}

#[tokio::test]
async fn inventory_is_idempotent_on_rerun() {
    common::require_infra!();
    let server = quip_fixture_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1", &["root"]).await;
    app.state
        .quip_token_store
        .put(&import_id, &QuipToken::new("tok".into()))
        .await
        .unwrap();

    // Pre-advance t1 to ContentDone. Because `put_thread` is
    // insert-if-absent, the inventory walk must NOT downgrade this row
    // back to Pending — the core resumability guarantee.
    app.state
        .import_repo
        .put_thread(
            &import_id,
            &ThreadRow {
                quip_thread_id: "t1".into(),
                owner_id: "owner1".into(),
                title: "Doc A".into(),
                thread_type: "document".into(),
                updated_usec: 111,
                member_folders: vec!["root".into()],
                first_folder: "root".into(),
                state: ThreadState::ContentDone,
                ogre_doc_id: Some("ogre-doc-1".into()),
            },
        )
        .await
        .unwrap();

    let ctx = worker_ctx_with_quip(&app, server.uri());

    // Two runs.
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    assert_eq!(threads.len(), 2, "no duplicate thread rows across re-runs");

    let t1 = threads.iter().find(|t| t.quip_thread_id == "t1").unwrap();
    assert_eq!(
        t1.state,
        ThreadState::ContentDone,
        "an advanced thread must not be downgraded by a re-run"
    );
    assert_eq!(t1.ogre_doc_id.as_deref(), Some("ogre-doc-1"));

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.phase, 2);
}

#[tokio::test]
async fn inventory_token_rejected_sets_status() {
    common::require_infra!();
    let server = quip_unauthorized_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1", &["root"]).await;
    app.state
        .quip_token_store
        .put(&import_id, &QuipToken::new("revoked".into()))
        .await
        .unwrap();

    let ctx = worker_ctx_with_quip(&app, server.uri());
    // A revoked token is terminal for this run: the handler returns Ok
    // (do not burn retries hammering Quip with a dead token) but flips
    // the status to TokenRejected so the UI can prompt a reconnect.
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        ImportStatus::TokenRejected,
        "a revoked token must set TokenRejected, not a generic Failed"
    );

    // The runner claim must be released even on this early (non-happy)
    // exit — a fresh instance can immediately re-claim (Ok(true) proves no
    // live lease was left behind by the clear-on-every-exit guard).
    let reclaimed = app
        .state
        .import_repo
        .claim_runner(&import_id, "fresh-after-tokenrejected", now_ms(), CLAIM_STALE_MS)
        .await
        .unwrap();
    assert!(reclaimed, "handler must clear the lease on the token-rejected path");
}

/// Regression: a crashed worker's still-fresh-looking DDB lease must not
/// strand the import when the queue redelivers the entry. With
/// `CLAIM_STALE_MS` (30s) below the reaper interval (60s), a lease whose
/// heartbeat is ~61s old is stale by redelivery time, so the handler
/// reclaims it and drives the import to completion instead of no-opping and
/// acking the job away.
#[tokio::test]
async fn inventory_reclaims_stale_lease() {
    common::require_infra!();
    let server = quip_fixture_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1", &["root"]).await;
    app.state
        .quip_token_store
        .put(&import_id, &QuipToken::new("tok".into()))
        .await
        .unwrap();

    // Simulate a crashed worker: a lease whose heartbeat is 61s old. Passing
    // an old `now_ms` sets `runner_heartbeat_ms` to that old timestamp.
    let acquired = app
        .state
        .import_repo
        .claim_runner(&import_id, "crashed-inst", now_ms() - 61_000, CLAIM_STALE_MS)
        .await
        .unwrap();
    assert!(acquired, "seed: crashed worker acquires the lease");

    // The redelivered handler (fresh instance id) must reclaim the stale
    // lease and complete — NOT no-op and get acked while the import strands.
    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.phase, 2, "reclaimed run must reach phase 2 (inventory + content)");
    let (total, _) = app.state.import_repo.count_threads_by_state(&import_id).await.unwrap();
    assert_eq!(total, 2, "reclaimed run must persist the discovered threads");
}

/// The happy path must release the runner claim on success so a subsequent
/// run (or Phase-2 handler) can re-acquire immediately rather than waiting
/// out the stale window.
#[tokio::test]
async fn inventory_clears_lease_on_success() {
    common::require_infra!();
    let server = quip_fixture_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1", &["root"]).await;
    app.state
        .quip_token_store
        .put(&import_id, &QuipToken::new("tok".into()))
        .await
        .unwrap();

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    // No live lease should remain: a fresh instance claims immediately.
    let reclaimed = app
        .state
        .import_repo
        .claim_runner(&import_id, "fresh-after-success", now_ms(), CLAIM_STALE_MS)
        .await
        .unwrap();
    assert!(reclaimed, "successful run must clear the runner claim");
}

/// A transient Quip error must surface as `Err` (so the queue retries) AND
/// the runner claim must be released on that error exit — otherwise the
/// retry (running under a different instance id) would see a live lease,
/// no-op, and get acked, stranding the import.
#[tokio::test]
async fn inventory_clears_lease_on_transient_error() {
    common::require_infra!();
    let server = quip_transient_error_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1", &["root"]).await;
    app.state
        .quip_token_store
        .put(&import_id, &QuipToken::new("tok".into()))
        .await
        .unwrap();

    let ctx = worker_ctx_with_quip(&app, server.uri());
    let result = execute_start_quip_import(&ctx, &import_id, "owner1").await;
    assert!(result.is_err(), "a transient (503) error must return Err for the queue to retry");

    // Lease released despite the Err → the retry can re-claim.
    let reclaimed = app
        .state
        .import_repo
        .claim_runner(&import_id, "fresh-after-transient", now_ms(), CLAIM_STALE_MS)
        .await
        .unwrap();
    assert!(reclaimed, "handler must clear the lease on a transient-error exit");
}

/// A SUSTAINED Quip failure must leave the `ImportRecord` in a terminal state.
/// The handler returns `Err` on each transient (503) attempt so the queue
/// retries; once `MAX_RETRIES` is exhausted the job dead-letters. Without the
/// dead-letter → `ImportStatus::Failed` write, the record stays `Running`/phase
/// 0 and the wizard's poll loop (which only stops on phase>=1 or a terminal
/// status) hangs on "Scanning…" forever. Drives the real
/// `execute_and_finalize` retry budget, mirroring
/// `test_worker_mode::execute_and_finalize_retries_to_budget_then_dead_letters`.
#[tokio::test]
async fn dead_lettered_quip_import_ends_failed() {
    common::require_infra!();
    let server = quip_transient_error_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1", &["root"]).await;
    app.state
        .quip_token_store
        .put(&import_id, &QuipToken::new("tok".into()))
        .await
        .unwrap();

    let ctx = worker_ctx_with_quip(&app, server.uri());
    let queue = fresh_queue("deadletter").await;

    let job_id = queue
        .enqueue(Job::StartQuipImport {
            import_id: import_id.clone(),
            owner_id: "owner1".to_string(),
        })
        .await
        .expect("enqueue");

    // MAX_RETRIES = 3: attempts 0,1,2 retry; attempt 3 dead-letters. Drive the
    // real finalize once per attempt.
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

    // Job is dead-lettered (gone from the main stream)...
    let next = queue.consume_next("c1", 500).await.expect("consume");
    assert!(next.is_none(), "job must be dead-lettered once the retry budget is spent");
    let _ = job_id; // job_id retained for parity with the queue-status precedent

    // ...and — the fix under test — the ImportRecord is now terminal Failed, so
    // the frontend poll loop stops instead of hanging on "Scanning…".
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        ImportStatus::Failed,
        "a dead-lettered Quip import must end Failed, not stay Running"
    );
}
