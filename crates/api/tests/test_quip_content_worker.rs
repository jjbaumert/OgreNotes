// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 2a — the `StartQuipImport` **content pass** integration tests.
//!
//! Drives `worker_mode::execute_start_quip_import` (the same `pub` seam the
//! Phase 1 inventory tests use) against a real DynamoDB-local + MinIO plus a
//! wiremock Quip server that now also serves `/2/threads/{id}/html` and
//! `/1/blob/{thread}/{blob}`. One job runs inventory *then* content, so these
//! tests exercise the wiring, not just the per-thread function.
//!
//! Fixture tree:
//!   root -> [thread t1, subfolder f2]
//!   f2   -> [thread t1 (shared), thread t2, thread tc]
//! with t1 = document (image + intra-Quip link), t2 = spreadsheet,
//! tc = chat (must be skipped without ever fetching its HTML).

mod common;

use aws_sdk_dynamodb::types::AttributeValue;
use ogrenotes_api::worker_mode::{
    build_folder_mapping, execute_start_quip_import, import_one_thread, WorkerCtx,
};
use ogrenotes_quip_import::{QuipClient, QuipToken};
use ogrenotes_storage::models::import::{ImportRecord, ImportStatus};
use ogrenotes_storage::models::import_inventory::{ThreadRow, ThreadState};
use ogrenotes_storage::models::DocType;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// t1's body: a section-anchored heading + paragraph, a Quip blob image,
/// and a link to the in-scope thread t2 (which becomes an UNRESOLVED# row
/// for Phase 2b to back-patch).
const T1_HTML: &str = r#"<h1 id="sec-1">Doc A</h1>
<p id="sec-2">Hello <b>world</b></p>
<img src="/blob/t1/b9" alt="pic.png">
<p>See <a href="https://acme.quip.com/t2/Sheet">the sheet</a></p>"#;

const T2_HTML: &str = r#"<p>numbers</p>"#;

/// Stand-in blob bytes. Content doesn't matter — only that exactly these
/// bytes land in S3 under the document's blob prefix.
const BLOB_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nnot-really-a-png";

/// Wiremock Quip server serving the Phase-1 inventory endpoints plus the
/// Phase-2a content endpoints.
async fn quip_content_server() -> MockServer {
    let server = MockServer::start().await;

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

    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .and(query_param("ids", "f2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "f2": {
                "folder": {"id": "f2", "title": "Sub"},
                "children": [ {"thread_id": "t1"}, {"thread_id": "t2"}, {"thread_id": "tc"} ]
            }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/1/threads/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "t1": {"thread": {"id": "t1", "title": "Doc A", "type": "document", "updated_usec": 111}},
            "t2": {"thread": {"id": "t2", "title": "Sheet", "type": "spreadsheet", "updated_usec": 222}},
            "tc": {"thread": {"id": "tc", "title": "Watercooler", "type": "chat", "updated_usec": 333}}
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/2/threads/t1/html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(T1_HTML))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/2/threads/t2/html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(T2_HTML))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/1/blob/t1/b9"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BLOB_BYTES.to_vec()))
        .mount(&server)
        .await;

    server
}

/// Seed a `Scoping` import record scoped to `root`, targeting `target-folder`.
async fn seed_scoping_import(app: &common::TestApp, owner: &str) -> String {
    let import_id = format!("imp-{}", nanoid::nanoid!(8));
    let now = ogrenotes_common::time::now_usec();
    let record = ImportRecord {
        import_id: import_id.clone(),
        owner_id: owner.to_string(),
        status: ImportStatus::Scoping,
        phase: 0,
        quip_user_id: None,
        target_folder_id: Some("target-folder".to_string()),
        selected_roots: vec!["root".to_string()],
        created_at: now,
        updated_at: now,
    };
    app.state.import_repo.create(&record).await.expect("seed import record");
    app.state
        .quip_token_store
        .put(&import_id, &QuipToken::new("tok".into()))
        .await
        .expect("seed token");
    import_id
}

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

/// How many times the mock served a given path.
async fn hits(server: &MockServer, suffix: &str) -> usize {
    server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .iter()
        .filter(|r| r.url.path().ends_with(suffix))
        .count()
}

/// The `ogre_doc_id` recorded on a thread's manifest row.
async fn doc_id_for(app: &common::TestApp, import_id: &str, thread: &str) -> Option<String> {
    let threads = app.state.import_repo.list_threads(import_id).await.unwrap();
    threads
        .into_iter()
        .find(|t| t.quip_thread_id == thread)
        .and_then(|t| t.ogre_doc_id)
}

#[tokio::test]
async fn content_pass_creates_documents_with_quip_timestamps_and_folders() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    // Every non-chat thread advanced to ContentDone with a document id.
    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let t1 = threads.iter().find(|t| t.quip_thread_id == "t1").unwrap();
    assert_eq!(t1.state, ThreadState::ContentDone);
    let t1_doc_id = t1.ogre_doc_id.clone().expect("t1 has an ogre doc id");

    let meta = app.state.doc_repo.get(&t1_doc_id).await.unwrap().expect("t1 document exists");
    assert_eq!(meta.title, "Doc A");
    assert_eq!(meta.owner_id, "owner1");
    assert_eq!(meta.doc_type, DocType::Document);

    // Quip's own timestamps, NOT `now`. `updated_usec` = 111 for t1.
    assert_eq!(meta.created_at, 111, "created_at must be the Quip updated_usec");
    assert_eq!(meta.updated_at, 111, "updated_at must be the Quip updated_usec");

    // Phase 1 leaves `ogre_folder_id` unset on every FOLDER# row, so the
    // mapping falls back to the import's target folder for all threads
    // (documented Phase-2a limitation: no mirrored folder tree yet).
    assert_eq!(meta.folder_id.as_deref(), Some("target-folder"));
    assert!(
        !meta.additional_folder_ids.contains(&"target-folder".to_string()),
        "the primary folder must not be duplicated into additional_folder_ids"
    );

    // The doc is really linked into the folder — `additional_folder_ids`
    // alone does not create the CHILD# rows.
    let children = app.state.folder_repo.list_children("target-folder").await.unwrap();
    let child_ids: std::collections::BTreeSet<_> =
        children.iter().map(|c| c.child_id.clone()).collect();
    assert!(child_ids.contains(&t1_doc_id), "t1's doc is linked into the target folder");

    // The raw HTML was staged to S3 under the import's prefix.
    let staged = app
        .state
        .doc_repo
        .s3()
        .get_object(&format!("imports/{import_id}/threads/t1.html"))
        .await
        .expect("staged html exists");
    assert_eq!(String::from_utf8(staged).unwrap(), T1_HTML);

    // Section map recorded for the two anchored blocks, in document order.
    let sections = app.state.import_repo.get_secmap(&import_id, "t1").await.unwrap();
    let ids: Vec<&str> = sections.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(ids, vec!["sec-1", "sec-2"]);
    assert!(sections.iter().all(|(_, block)| !block.is_empty()), "{sections:?}");

    // The import advanced to phase 2.
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.phase, 2, "content pass must advance the import to phase 2");
}

#[tokio::test]
async fn content_pass_is_resumable_and_never_duplicates() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();
    let first_doc_id = doc_id_for(&app, &import_id, "t1").await.expect("t1 imported");
    assert_eq!(hits(&server, "/2/threads/t1/html").await, 1);

    // Second run of the SAME job: inventory re-walks (cheap, idempotent) but
    // the content pass must skip every ContentDone thread *before* fetching.
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    assert_eq!(
        hits(&server, "/2/threads/t1/html").await,
        1,
        "a re-run must not refetch an already-ContentDone thread's HTML"
    );
    assert_eq!(
        hits(&server, "/1/blob/t1/b9").await,
        1,
        "a re-run must not refetch an already-ContentDone thread's blobs"
    );
    assert_eq!(
        doc_id_for(&app, &import_id, "t1").await.as_deref(),
        Some(first_doc_id.as_str()),
        "a re-run must not mint a second document for the same thread"
    );

    // Exactly two documents exist for the owner (t1 + t2); the chat made none.
    let docs = app.state.doc_repo.query_docs_by_owner("owner1").await.unwrap();
    assert_eq!(docs.len(), 2, "one document per non-chat thread, no duplicates: {docs:?}");
}

#[tokio::test]
async fn images_are_sideloaded_to_s3_and_src_becomes_a_blob_reference() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let doc_id = doc_id_for(&app, &import_id, "t1").await.expect("t1 imported");

    // The persisted snapshot's Image.src is a durable blob reference, not
    // the Quip-relative path the walker recorded.
    let snapshot = app.state.doc_repo.load_snapshot(&doc_id).await.unwrap().expect("snapshot");
    let doc = ogrenotes_collab::snapshot::deserialize(&snapshot).expect("decode snapshot");
    let refs = ogrenotes_collab::blob_ref::collect_blob_refs(doc.inner());
    assert_eq!(refs.len(), 1, "exactly one blob reference: {refs:?}");
    let (blob_id, key) = &refs[0];
    assert_eq!(blob_id, "b9", "the Quip blob id is preserved");
    assert!(
        key.starts_with(&format!("blobs/{doc_id}/b9/")),
        "the key must sit under this document's blob prefix: {key}"
    );

    // ...and the bytes really landed there.
    let bytes = app.state.doc_repo.s3().get_object(key).await.expect("blob object exists");
    assert_eq!(bytes, BLOB_BYTES, "the side-loaded bytes are Quip's");
}

#[tokio::test]
async fn chat_threads_are_skipped_and_spreadsheets_become_spreadsheet_docs() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();

    let chat = threads.iter().find(|t| t.quip_thread_id == "tc").unwrap();
    assert_eq!(chat.state, ThreadState::Skipped, "a chat thread is skipped, not imported");
    assert!(chat.ogre_doc_id.is_none(), "a skipped chat makes no document");
    assert_eq!(
        hits(&server, "/2/threads/tc/html").await,
        0,
        "a chat thread's HTML must never be fetched"
    );

    let sheet = threads.iter().find(|t| t.quip_thread_id == "t2").unwrap();
    assert_eq!(sheet.state, ThreadState::ContentDone);
    let sheet_doc_id = sheet.ogre_doc_id.clone().expect("t2 has an ogre doc id");
    let meta = app.state.doc_repo.get(&sheet_doc_id).await.unwrap().expect("t2 document");
    assert_eq!(meta.doc_type, DocType::Spreadsheet, "a Quip spreadsheet imports as a Spreadsheet");
    assert_eq!(meta.title, "Sheet");
    assert_eq!(meta.created_at, 222, "Quip's own updated_usec, not now");
}

#[tokio::test]
async fn intra_quip_links_are_recorded_unresolved_for_phase_2b() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let unresolved = app.state.import_repo.list_unresolved(&import_id).await.unwrap();
    let t1 = unresolved
        .iter()
        .find(|u| u.source_quip_thread_id == "t1")
        .expect("t1's link recorded for back-patching");
    assert_eq!(t1.owner_id, "owner1");
    assert_eq!(t1.links.len(), 1, "{:?}", t1.links);
    assert_eq!(t1.links[0].target_quip_thread_id, "t2");
    assert!(
        !t1.links[0].source_block_id.is_empty(),
        "the source block must be named so Phase 2b can find the chip"
    );

    // t2 has no outbound Quip links, so it gets no row at all.
    assert!(
        !unresolved.iter().any(|u| u.source_quip_thread_id == "t2"),
        "a link-free thread must not write an empty UNRESOLVED# row"
    );
}

/// Regression (I3): a finished import must reach a terminal
/// `ImportStatus::Succeeded`.
///
/// Nothing used to set it — `run_content_pass` bumped `phase` to 2 and
/// returned, so a completed import read `Running` forever. The wizard papered
/// over that by keying on `phase >= 2`, but it left "finished" and "stranded
/// mid-run" as the *same* record state, which is why no recovery sweep over
/// `Running` imports could be written.
#[tokio::test]
async fn completed_content_pass_ends_succeeded() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.phase, 2, "precondition: the content pass ran to completion");
    assert_eq!(
        rec.status,
        ImportStatus::Succeeded,
        "a completed import must be terminally Succeeded, not indefinitely Running",
    );

    // The wizard's terminal-FAILURE set is {failed, tokenrejected, cancelled};
    // `succeeded` must stay out of it, or a successful import would render as
    // an error.
    assert!(
        !matches!(
            rec.status,
            ImportStatus::Failed | ImportStatus::TokenRejected | ImportStatus::Cancelled
        ),
        "succeeded must not collide with the wizard's terminal-failure statuses",
    );
}

/// Regression (I1): an ordinary transient error *after* the document is
/// created must not duplicate the user's document.
///
/// Steps 8/9/10 — `put_secmap`, `put_unresolved`, `set_thread_content_done` —
/// each return `Transient` on failure and abort the pass while the thread is
/// still `Pending`. The retry used to mint a **fresh** `doc_id`, so a single
/// DynamoDB throttle deterministically produced a second document (up to four
/// across `MAX_RETRIES`), violating the plan's "never create a duplicate
/// document for one thread" invariant.
///
/// The retry's inputs are reconstructed exactly as DynamoDB holds them after
/// such a failure: the document exists, its id is reserved on the `THREAD#`
/// row, and the row is still `Pending` because the checkpoint never landed.
/// Driving `import_one_thread` (the documented per-thread test seam) with that
/// row is precisely what the queue's retry does.
#[tokio::test]
async fn retry_after_a_post_create_failure_creates_no_second_document() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    // Attempt 1 gets t1 all the way to a real document.
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();
    let doc_id = doc_id_for(&app, &import_id, "t1").await.expect("t1 imported");
    let before = app.state.doc_repo.query_docs_by_owner("owner1").await.unwrap();
    assert_eq!(before.len(), 2, "precondition: one doc each for t1 and t2");

    // Rewind to the state a step-8/9/10 failure leaves behind: everything up
    // to and including `DocRepo::create` is durable, the `ContentDone`
    // checkpoint is not.
    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let t1 = threads.iter().find(|t| t.quip_thread_id == "t1").unwrap();
    // NOTE: this row's `ogre_doc_id` was written by attempt 1's *checkpoint*,
    // not by step 6's reservation — so this test pins only the RECONCILE half
    // of the fix. `reserve_before_the_first_durable_write_survives_a_failed_attempt`
    // below covers the reservation half, which is the part a real step-8
    // failure depends on.
    assert_eq!(t1.ogre_doc_id.as_deref(), Some(doc_id.as_str()));
    let retry_row = ThreadRow { state: ThreadState::Pending, ..t1.clone() };

    let record = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    let folders = build_folder_mapping(&ctx, &import_id, &record).await.unwrap();
    let client = QuipClient::new(Some(server.uri()));
    let token = QuipToken::new("tok".into());
    import_one_thread(&ctx, &import_id, "owner1", &client, &token, &retry_row, &folders)
        .await
        .expect("the retry must complete, not fail on the already-created document");

    // THE ASSERTION: exactly one document for this thread, still the same one.
    let after = app.state.doc_repo.query_docs_by_owner("owner1").await.unwrap();
    assert_eq!(
        after.len(),
        2,
        "a retry must adopt its own earlier document, not mint a second: {after:?}",
    );
    assert_eq!(
        doc_id_for(&app, &import_id, "t1").await.as_deref(),
        Some(doc_id.as_str()),
        "the manifest must still name the original document",
    );

    // ...and the retry finished the tail it had failed on.
    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let t1 = threads.iter().find(|t| t.quip_thread_id == "t1").unwrap();
    assert_eq!(t1.state, ThreadState::ContentDone, "the retry must land the checkpoint");

    // The adopted document is intact — same title, same Quip timestamps, and a
    // readable v1 snapshot (the retry re-asserts it rather than assuming the
    // first attempt's S3 write landed).
    let meta = app.state.doc_repo.get(&doc_id).await.unwrap().expect("document survives");
    assert_eq!(meta.title, "Doc A");
    assert_eq!(meta.created_at, 111, "Quip provenance preserved across the retry");
    let snapshot = app.state.doc_repo.load_snapshot(&doc_id).await.unwrap();
    assert!(snapshot.is_some_and(|s| !s.is_empty()), "the v1 snapshot must be readable");
}

/// Wiremock Quip server identical to [`quip_content_server`] except that the
/// FIRST fetch of t1's blob 401s. That aborts t1's import at `sideload_images`
/// — after step 6 has settled the document id, before any document exists —
/// which is the shape of every real post-reservation failure. Every later
/// request succeeds, so the same server serves the retry.
async fn quip_server_failing_the_first_blob_fetch() -> MockServer {
    let server = quip_content_server().await;
    // Higher priority (lower number) than the 200 already mounted, and usable
    // exactly once; after that wiremock falls through to the 200.
    Mock::given(method("GET"))
        .and(path("/1/blob/t1/b9"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    server
}

/// Regression (I1, the reservation half): the document id is settled on the
/// `THREAD#` row **before** anything durable is written under it.
///
/// This is the half a real step-8/9/10 failure depends on. Without it, an
/// attempt that dies after `DocRepo::create` leaves a `Pending` row with no id
/// on it, the retry mints a fresh one, and the user gets two documents — and
/// crucially, a test that seeds the row from a *successful* attempt can't tell
/// the difference, because `set_thread_content_done` writes `ogre_doc_id` too.
///
/// So this drives a genuine mid-thread failure (a 401 on the image blob, which
/// lands between the reservation and the persist) and checks the manifest in
/// that intermediate state: an id present, `Pending` still, and no document
/// under it yet. Then it lets the retry run and confirms it adopts that exact
/// id.
#[tokio::test]
async fn reserve_before_the_first_durable_write_survives_a_failed_attempt() {
    common::require_infra!();
    let server = quip_server_failing_the_first_blob_fetch().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    // Attempt 1: t1 dies on its image blob. (A 401 is terminal for the run, so
    // the pass stops at t1 — which is exactly the state we want to inspect.)
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let t1 = threads.iter().find(|t| t.quip_thread_id == "t1").unwrap();
    let reserved = t1
        .ogre_doc_id
        .clone()
        .expect("the doc id must be RESERVED on the row before any durable write");
    assert_eq!(
        t1.state,
        ThreadState::Pending,
        "the thread must still be Pending — a reservation is not progress",
    );
    assert!(
        app.state.doc_repo.get(&reserved).await.unwrap().is_none(),
        "nothing may exist under the reserved id yet; that is what makes it a reservation",
    );
    let after_failure = app.state.doc_repo.query_docs_by_owner("owner1").await.unwrap();
    assert!(
        after_failure.is_empty(),
        "the failed attempt must have created no document at all: {after_failure:?}",
    );

    // Attempt 2 (the queue's retry): the blob now serves, and the thread must
    // be imported under the id reserved by attempt 1.
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    assert_eq!(
        doc_id_for(&app, &import_id, "t1").await.as_deref(),
        Some(reserved.as_str()),
        "the retry must import under the RESERVED id, not a freshly minted one",
    );
    let docs = app.state.doc_repo.query_docs_by_owner("owner1").await.unwrap();
    assert_eq!(docs.len(), 2, "one document per non-chat thread, no duplicates: {docs:?}");
    assert!(
        docs.iter().any(|d| d.doc_id == reserved && d.title == "Doc A"),
        "t1's document is the reserved one: {docs:?}",
    );
}

// ─── Per-thread failure disposition (#141 / #142) ────────────────

/// The queue's real budget for one `StartQuipImport` entry: the first
/// attempt plus `worker_mode::MAX_RETRIES` (3) retries. A per-thread give-up
/// that needed more runs than this would dead-letter the import instead of
/// finishing it, which is #142 renamed rather than fixed — so every test that
/// exercises the give-up path drives the job through *this* helper rather
/// than calling the handler an unbounded number of times.
const QUEUE_RUNS: usize = 4;

/// Drive the import the way the queue does: run it until a run doesn't fail,
/// giving up after `QUEUE_RUNS`. Returns how many runs it took.
async fn run_like_the_queue(ctx: &WorkerCtx, import_id: &str, owner: &str) -> usize {
    for run in 1..=QUEUE_RUNS {
        if execute_start_quip_import(ctx, import_id, owner).await.is_ok() {
            return run;
        }
    }
    panic!("the import never completed inside the queue's {QUEUE_RUNS}-run budget: it would have dead-lettered");
}

/// [`quip_content_server`] with t1's HTML fetch overridden to `status`
/// forever. Higher priority (lower number) than the 200 mounted underneath.
async fn quip_server_with_thread_html_status(thread: &str, status: u16) -> MockServer {
    let server = quip_content_server().await;
    Mock::given(method("GET"))
        .and(path(format!("/2/threads/{thread}/html")))
        .respond_with(ResponseTemplate::new(status))
        .with_priority(1)
        .mount(&server)
        .await;
    server
}

async fn thread_row(app: &common::TestApp, import_id: &str, thread: &str) -> ThreadRow {
    app.state
        .import_repo
        .list_threads(import_id)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.quip_thread_id == thread)
        .unwrap_or_else(|| panic!("no THREAD# row for {thread}"))
}

/// Regression (#141): a 403 on ONE thread skips that thread and lets the rest
/// of the import finish.
///
/// This is the bug's whole shape. A single access-restricted Quip document
/// used to map to a run-terminal `TokenRejected`: the import halted and told
/// the user their token had expired, when the token was fine. Reconnecting a
/// fresh one didn't help — the re-run reached the same thread and halted
/// again — so the import was permanently wedged behind a misleading
/// diagnosis.
#[tokio::test]
async fn a_forbidden_thread_is_skipped_and_the_other_threads_still_import() {
    common::require_infra!();
    let server = quip_server_with_thread_html_status("t1", 403).await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    let runs = run_like_the_queue(&ctx, &import_id, "owner1").await;
    assert_eq!(runs, 1, "a 403 is a decided outcome; it must not cost the job a retry");

    // The inaccessible thread is skipped BY NAME, with a reason.
    let t1 = thread_row(&app, &import_id, "t1").await;
    assert_eq!(t1.state, ThreadState::Skipped, "a 403 thread is skipped, not fatal");
    let reason = t1.reason.clone().unwrap_or_default();
    assert!(reason.contains("403"), "the reason must say why: {reason:?}");

    // ...and everything else really imported. This is the half that used to
    // be missing entirely.
    let t2 = thread_row(&app, &import_id, "t2").await;
    assert_eq!(t2.state, ThreadState::ContentDone, "the other threads must still import");
    let sheet_doc_id = t2.ogre_doc_id.clone().expect("t2 has a document");
    let meta = app.state.doc_repo.get(&sheet_doc_id).await.unwrap().expect("t2's document exists");
    assert_eq!(meta.title, "Sheet");

    // The import is a SUCCESS with a report line, not a token failure.
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        ImportStatus::Succeeded,
        "one inaccessible document must not fail the import",
    );
    assert_ne!(
        rec.status,
        ImportStatus::TokenRejected,
        "the credential was valid; claiming otherwise is the #141 misdiagnosis",
    );
    assert_eq!(rec.phase, 2, "the content pass must have run to completion");
}

/// Regression (#142): a thread that fails deterministically is given up on —
/// marked `Failed` — and the pass completes for everyone else.
///
/// A Quip 5xx on one thread used to abort the whole content pass on every
/// run, so the import produced "nothing after t0042" and eventually
/// dead-lettered. The give-up has to land INSIDE the queue's retry budget:
/// `run_like_the_queue` fails the test if it doesn't.
#[tokio::test]
async fn a_thread_that_always_fails_is_marked_failed_and_the_pass_completes() {
    common::require_infra!();
    let server = quip_server_with_thread_html_status("t1", 500).await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    let runs = run_like_the_queue(&ctx, &import_id, "owner1").await;
    assert!(
        runs <= QUEUE_RUNS,
        "the give-up must happen before the job dead-letters (took {runs} runs)",
    );

    let t1 = thread_row(&app, &import_id, "t1").await;
    assert_eq!(
        t1.state,
        ThreadState::Failed,
        "a thread the pass tried and lost is Failed, not Skipped and not Pending",
    );
    assert_eq!(t1.attempts, 3, "the attempt counter is what decides the give-up");
    let reason = t1.reason.clone().unwrap_or_default();
    assert!(reason.contains("500"), "the reason names the failure: {reason:?}");
    assert!(reason.contains("gave up"), "the reason says it gave up: {reason:?}");

    // The other threads are unaffected — this is the whole point.
    let t2 = thread_row(&app, &import_id, "t2").await;
    assert_eq!(t2.state, ThreadState::ContentDone);
    assert!(
        app.state.doc_repo.get(&t2.ogre_doc_id.clone().unwrap()).await.unwrap().is_some(),
        "the healthy thread's document must exist",
    );

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.phase, 2, "the pass must reach the end of the manifest");
    assert_eq!(
        rec.status,
        ImportStatus::Succeeded,
        "one dead thread must not fail the import",
    );
}

/// Regression: a 401 STILL halts the whole run. This is the risk #141's fix
/// creates — loosening 403 must not loosen 401 with it.
///
/// A dead credential is genuinely run-terminal: every remaining thread would
/// fail identically, so continuing would burn the retry budget hammering Quip
/// with a revoked token and would mark every remaining thread `Failed` over a
/// problem a reconnect fixes in one click.
#[tokio::test]
async fn a_401_on_a_thread_still_halts_the_whole_run() {
    common::require_infra!();
    let server = quip_server_with_thread_html_status("t1", 401).await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        ImportStatus::TokenRejected,
        "a 401 must still flip the import to TokenRejected so the UI prompts a reconnect",
    );
    assert_ne!(rec.phase, 2, "a halted run must not claim the content pass completed");

    // The pass stopped AT t1 — it did not walk on marking threads Skipped or
    // Failed, and it did not import anything past it.
    let t1 = thread_row(&app, &import_id, "t1").await;
    assert_eq!(t1.state, ThreadState::Pending, "a 401 thread stays Pending; it is not the thread's fault");
    let t2 = thread_row(&app, &import_id, "t2").await;
    assert_eq!(t2.state, ThreadState::Pending, "the run must stop, not continue to the next thread");
    assert_eq!(
        hits(&server, "/2/threads/t2/html").await,
        0,
        "no further Quip call may be made with a credential known to be dead",
    );
    let docs = app.state.doc_repo.query_docs_by_owner("owner1").await.unwrap();
    assert!(docs.is_empty(), "a halted run creates no documents: {docs:?}");
}

/// The REPORT row is what turns "999 documents" into "999 documents and a
/// report line": it must name the threads that were skipped and failed, with
/// reasons, and carry the true totals in its counters.
#[tokio::test]
async fn the_report_names_the_skipped_and_failed_threads_with_reasons() {
    common::require_infra!();
    // t1 is inaccessible (403 → skip); t2 is broken (500 → fail after N).
    //
    // t2's error BODY is hostile on purpose: `QuipError::Api` carries the raw
    // response verbatim, so anything that stringifies the error into a
    // durable `reason` / `detail` — rather than going through
    // `safe_quip_reason` — writes a credential and a user's address into
    // DynamoDB and then into the frontend's report. This body is what makes
    // the leak assertions at the bottom load-bearing rather than decorative.
    let server = quip_server_with_thread_html_status("t1", 403).await;
    Mock::given(method("GET"))
        .and(path("/2/threads/t2/html"))
        .respond_with(ResponseTemplate::new(500).set_body_string(
            r#"{"error":"tok-SEEKRET rejected for ada@example.com"}"#,
        ))
        .with_priority(1)
        .mount(&server)
        .await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    run_like_the_queue(&ctx, &import_id, "owner1").await;

    let report = app
        .state
        .import_repo
        .get_report(&import_id)
        .await
        .expect("report read")
        .expect("an import that lost threads must have a REPORT row");
    assert_eq!(report.owner_id, "owner1");

    assert_eq!(report.counters.get("threads_skipped_forbidden"), Some(&1));
    assert_eq!(report.counters.get("threads_failed"), Some(&1));
    assert_eq!(report.counters.get("threads_skipped_chat"), Some(&1), "tc is a chat");
    assert_eq!(report.notes_dropped, 0, "nothing was over budget here: {report:?}");

    let skipped = report
        .notes
        .iter()
        .find(|n| n.quip_thread_id == "t1")
        .expect("the skipped thread must be named: {report:?}");
    assert_eq!(skipped.kind, "thread_skipped");
    assert!(skipped.detail.contains("403"), "{skipped:?}");

    let failed = report
        .notes
        .iter()
        .find(|n| n.quip_thread_id == "t2")
        .expect("the failed thread must be named");
    assert_eq!(failed.kind, "thread_failed");
    assert!(failed.detail.contains("500"), "{failed:?}");

    // Chats are counted but deliberately NOT named — a chat-heavy import
    // would otherwise spend the whole note budget on them and name none of
    // the documents it actually lost.
    assert!(
        !report.notes.iter().any(|n| n.quip_thread_id == "tc"),
        "chat skips are a counter, not a note: {report:?}",
    );

    // Nothing Quip echoed back into its error body reaches a durable,
    // user-visible string — not the report's notes, and not the `THREAD#`
    // row's `reason`, which the wizard renders.
    let t2_reason = thread_row(&app, &import_id, "t2").await.reason.unwrap_or_default();
    let durable_strings: Vec<String> = report
        .notes
        .iter()
        .map(|n| n.detail.clone())
        .chain(std::iter::once(t2_reason))
        .collect();
    for s in &durable_strings {
        assert!(!s.contains("SEEKRET"), "a credential must never reach durable state: {s:?}");
        assert!(!s.contains('@'), "no address may reach durable state: {s:?}");
        assert!(
            !s.contains("rejected for"),
            "Quip's raw response body must not be stringified into durable state: {s:?}",
        );
    }
}

/// A report write that can never succeed must not affect the import.
///
/// The `REPORT` row is a plain read-modify-write over a decoded `ReportRow`,
/// so a row whose `counters` map holds a non-numeric value fails
/// `report_from_item` on *every* subsequent read — `bump_report_counter` and
/// `append_report_note` are then permanently broken for that import. That is a
/// real reachable state, not a mock: this test writes exactly such a row and
/// then runs an ordinary, entirely healthy import over it. An import that died
/// because it could not write a note about a dying import is the worst
/// outcome available.
#[tokio::test]
async fn a_permanently_poisoned_report_row_does_not_affect_the_import() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    // Poison it: `counters.threads_imported` is a string where the decoder
    // requires a number.
    app.dynamo_client()
        .put_item()
        .table_name(&app.table_name)
        .item("PK", AttributeValue::S(format!("IMPORT#{import_id}")))
        .item("SK", AttributeValue::S("REPORT".to_string()))
        .item("owner_id", AttributeValue::S("owner1".to_string()))
        .item(
            "counters",
            AttributeValue::M(std::collections::HashMap::from([(
                "threads_imported".to_string(),
                AttributeValue::S("not-a-number".to_string()),
            )])),
        )
        .send()
        .await
        .expect("seed a poisoned REPORT row");
    assert!(
        app.state.import_repo.get_report(&import_id).await.is_err(),
        "precondition: every report read for this import now fails",
    );

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1")
        .await
        .expect("a broken report must not fail the import");

    // The import is completely unaffected: same outcome as the happy path.
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Succeeded);
    assert_eq!(rec.phase, 2);
    let docs = app.state.doc_repo.query_docs_by_owner("owner1").await.unwrap();
    assert_eq!(docs.len(), 2, "both documents imported: {docs:?}");
    assert_eq!(thread_row(&app, &import_id, "t1").await.state, ThreadState::ContentDone);
    assert_eq!(thread_row(&app, &import_id, "t2").await.state, ThreadState::ContentDone);

    // And the row is still poisoned — the import never repaired or overwrote
    // it, which is what makes this "permanently" broken rather than "broken
    // once".
    assert!(
        app.state.import_repo.get_report(&import_id).await.is_err(),
        "the poisoned row is still poisoned; the import simply never depended on it",
    );
}

/// The arithmetic that makes #142's fix a fix rather than a rename: *several*
/// deterministically-failing threads must all resolve inside the queue's
/// retry budget, not just one.
///
/// This is what forces the pass to keep walking the manifest after an
/// under-budget thread failure instead of returning `Err` on the spot. An
/// abort-on-first-bad-thread pass advances exactly ONE thread's attempt
/// counter per run, so two bad threads need `2 * MAX_THREAD_ATTEMPTS - 1` = 5
/// runs — one more than the queue gives — and the import dead-letters with
/// the second thread still `Pending`. Continuing charges an attempt to every
/// bad thread in a single run, so any number of them resolve in
/// `MAX_THREAD_ATTEMPTS` runs.
#[tokio::test]
async fn several_failing_threads_all_resolve_inside_the_queues_retry_budget() {
    common::require_infra!();
    let server = quip_server_with_thread_html_status("t1", 500).await;
    Mock::given(method("GET"))
        .and(path("/2/threads/t2/html"))
        .respond_with(ResponseTemplate::new(500))
        .with_priority(1)
        .mount(&server)
        .await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    // Fails the test — with the dead-letter spelled out — if the give-up
    // needs more runs than the queue has.
    let runs = run_like_the_queue(&ctx, &import_id, "owner1").await;
    assert_eq!(
        runs, 3,
        "both threads must be charged an attempt PER RUN, so both give up on run \
         MAX_THREAD_ATTEMPTS; taking longer means the pass is still aborting at the \
         first bad thread",
    );

    for thread in ["t1", "t2"] {
        let row = thread_row(&app, &import_id, thread).await;
        assert_eq!(row.state, ThreadState::Failed, "{thread} must be given up on");
        assert_eq!(row.attempts, 3, "{thread} must have been charged one attempt per run");
    }
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Succeeded);
    assert_eq!(rec.phase, 2);
}

/// Mirror of `worker_mode::MAX_CONSECUTIVE_THREAD_FAILURES` (same convention
/// as `CLAIM_STALE_MS` in the inventory suite — the const is private).
const MAX_CONSECUTIVE_THREAD_FAILURES: usize = 5;

/// Seed an extra `Pending` document thread straight onto the manifest.
///
/// `run_content_pass` walks `list_threads`, and `put_thread` is
/// insert-if-absent with no delete anywhere in the repo, so a pre-seeded row
/// survives the inventory re-walk at the top of every run. That is what makes
/// a manifest larger than the wiremock fixture's folder tree possible without
/// new plumbing.
async fn seed_extra_thread(app: &common::TestApp, import_id: &str, quip_thread_id: &str) {
    app.state
        .import_repo
        .put_thread(
            import_id,
            &ThreadRow {
                quip_thread_id: quip_thread_id.to_string(),
                owner_id: "owner1".to_string(),
                title: quip_thread_id.to_string(),
                thread_type: "document".to_string(),
                updated_usec: 999,
                member_folders: vec!["root".to_string()],
                first_folder: "root".to_string(),
                state: ThreadState::Pending,
                ogre_doc_id: None,
                reason: None,
                attempts: 0,
            },
        )
        .await
        .expect("seed extra thread row");
}

/// The circuit breaker: back-to-back thread failures read as a broken *Quip*,
/// not as broken threads, and stop the pass.
///
/// This is the guard that makes "keep walking after a thread failure" safe.
/// Continuing is right when the bad threads are scattered (#142's shape), and
/// catastrophic when *everything* is failing: without the breaker a Quip-wide
/// 5xx outage would charge an attempt to every thread in the manifest, and
/// three such runs would mark an entire import `Failed` thread by thread over
/// an outage that resolved itself in an hour.
///
/// The load-bearing assertion is on the thread *past* the breaking point:
/// `attempts == 0` and zero HTTP hits prove the walk actually STOPPED, rather
/// than merely happening to end.
#[tokio::test]
async fn consecutive_thread_failures_stop_the_pass_instead_of_condemning_the_manifest() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    // t3..t8 have no `/2/threads/{id}/html` mock, so wiremock's default 404
    // becomes `QuipError::Api { status: 404 }` -> `Transient`: six failing
    // threads, one more than the breaker's threshold. Sorted order puts them
    // after t1/t2 and before tc.
    let failing: Vec<String> = (3..=8).map(|i| format!("t{i}")).collect();
    assert_eq!(
        failing.len(),
        MAX_CONSECUTIVE_THREAD_FAILURES + 1,
        "the fixture must hold exactly one thread PAST the breaking point",
    );
    for t in &failing {
        seed_extra_thread(&app, &import_id, t).await;
    }

    let ctx = worker_ctx_with_quip(&app, server.uri());
    let err = execute_start_quip_import(&ctx, &import_id, "owner1")
        .await
        .expect_err("tripping the breaker must fail the job so the queue retries with backoff");
    assert!(err.contains("consecutive"), "the error must name the reason: {err:?}");

    // The first MAX_CONSECUTIVE_THREAD_FAILURES failures were each charged an
    // attempt, and then the pass stopped.
    for t in &failing[..MAX_CONSECUTIVE_THREAD_FAILURES] {
        let row = thread_row(&app, &import_id, t).await;
        assert_eq!(row.attempts, 1, "{t} must have been charged exactly one attempt");
        assert_eq!(row.state, ThreadState::Pending, "{t} is retryable, not given up on");
    }

    // THE ASSERTION: the thread past the breaking point was never touched.
    // Without the breaker the pass would have walked it (and every thread
    // after it) charging attempts the whole way.
    let t8 = thread_row(&app, &import_id, "t8").await;
    assert_eq!(t8.attempts, 0, "the pass must STOP, not run out of threads");
    assert_eq!(t8.state, ThreadState::Pending);
    assert_eq!(
        hits(&server, "/2/threads/t8/html").await,
        0,
        "no Quip call may be spent past the breaking point",
    );
    // tc sorts after t8, so it is untouched too — a second, independent
    // witness that the loop exited early rather than completing.
    assert_eq!(
        thread_row(&app, &import_id, "tc").await.state,
        ThreadState::Pending,
        "the chat thread sorts last and must not have been reached",
    );

    // Progress made before the breaker tripped is still durable.
    for t in ["t1", "t2"] {
        assert_eq!(
            thread_row(&app, &import_id, t).await.state,
            ThreadState::ContentDone,
            "{t} imported before the failures started and must stay imported",
        );
    }
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_ne!(rec.phase, 2, "an aborted pass must not claim completion");
    assert_ne!(
        rec.status,
        ImportStatus::Succeeded,
        "an aborted pass must not claim success",
    );
}
