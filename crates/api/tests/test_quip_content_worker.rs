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

use ogrenotes_api::worker_mode::{execute_start_quip_import, WorkerCtx};
use ogrenotes_quip_import::QuipToken;
use ogrenotes_storage::models::import::{ImportRecord, ImportStatus};
use ogrenotes_storage::models::import_inventory::ThreadState;
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
