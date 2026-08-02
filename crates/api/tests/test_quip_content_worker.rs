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
    build_folder_mapping, execute_start_quip_import, import_one_thread, PersonDirectory, WorkerCtx,
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

/// Wrap a bare HTML body in the JSON envelope `GET /2/threads/{id}/html`
/// actually returns (#169): `{ "html": "...", "response_metadata":
/// { "next_cursor": "" } }`. A single-page (empty-cursor) response.
fn html_envelope(html: &str) -> serde_json::Value {
    serde_json::json!({ "html": html, "response_metadata": { "next_cursor": "" } })
}

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

    // The real `/2/threads/{id}/html` returns a JSON envelope, not bare HTML
    // (#169). The fixture serves that exact shape so the mock matches reality;
    // serving bare HTML here is what let the garbling bug ship.
    Mock::given(method("GET"))
        .and(path("/2/threads/t1/html"))
        .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(T1_HTML)))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/2/threads/t2/html"))
        .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(T2_HTML)))
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
        import_folder_id: None,
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
        app.state.user_repo.clone(),
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

/// Every S3 key currently under `prefix`, read straight from the bucket.
///
/// Deliberately a real `list_objects_v2` rather than a spy on the delete call:
/// "a delete was issued" is exactly the assertion that passes against code
/// that deletes the wrong prefix, and #196 is a data-*retention* bug, so the
/// only assertion that means anything is what the bucket still holds.
async fn keys_under(app: &common::TestApp, prefix: &str) -> Vec<String> {
    app.s3_client()
        .list_objects_v2()
        .bucket(&app.bucket)
        .prefix(prefix)
        .send()
        .await
        .expect("list objects")
        .contents()
        .iter()
        .filter_map(|o| o.key().map(str::to_string))
        .collect()
}

/// The keys staged under one import's thread-staging prefix.
async fn staged_keys(app: &common::TestApp, import_id: &str) -> Vec<String> {
    keys_under(app, &format!("imports/{import_id}/threads/")).await
}

/// `true` when S3 answers `HEAD` with a 404 for `key` — the object is really
/// gone, not merely absent from a cached listing.
async fn head_is_404(app: &common::TestApp, key: &str) -> bool {
    match app.s3_client().head_object().bucket(&app.bucket).key(key).send().await {
        Ok(_) => false,
        Err(e) => e.into_service_error().is_not_found(),
    }
}

/// Put one object into the test bucket under an arbitrary key.
async fn seed_object(app: &common::TestApp, key: &str, body: &[u8]) {
    app.state
        .doc_repo
        .s3()
        .put_object(key, body.to_vec())
        .await
        .unwrap_or_else(|e| panic!("seed {key}: {e}"));
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

    // The raw HTML was staged to S3 under the import's prefix *during* the
    // run, and swept when the import went terminal (#196 — the staged object
    // is the user's full document text, so it must not outlive the import).
    // This assertion used to read "staged html exists"; the mid-run staging it
    // covered now lives in `a_retryable_run_keeps_its_staging_for_the_retry`,
    // and the sweep itself in `a_succeeded_import_deletes_its_staged_html`.
    assert!(
        staged_keys(&app, &import_id).await.is_empty(),
        "a succeeded import must leave no staged thread HTML behind",
    );

    // Section map recorded for the two anchored blocks, in document order.
    let sections = app.state.import_repo.get_secmap(&import_id, "t1").await.unwrap();
    let ids: Vec<&str> = sections.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(ids, vec!["sec-1", "sec-2"]);
    assert!(sections.iter().all(|(_, block)| !block.is_empty()), "{sections:?}");

    // The import advanced to phase 2.
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.phase, 2, "content pass must advance the import to phase 2");
}

/// Rich body for the #169 block-structure test: an `<h1>`, a `<p>`, and a
/// `<ul><li>` — three distinct block kinds. Fed through the JSON envelope so
/// the whole content pass (client parse → walker → snapshot) is exercised.
const T1_RICH_HTML: &str = concat!(
    "<h1 id=\"sec-1\">Heading One</h1>",
    "<p id=\"sec-2\">A paragraph of prose.</p>",
    "<ul><li>first item</li><li>second item</li></ul>",
);

/// [`quip_content_server`] with t1's HTML fetch overridden to a rich JSON
/// envelope (heading + paragraph + list). Higher priority (lower number) than
/// the 200 already mounted underneath.
async fn quip_server_with_rich_t1_envelope() -> MockServer {
    let server = quip_content_server().await;
    Mock::given(method("GET"))
        .and(path("/2/threads/t1/html"))
        .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(T1_RICH_HTML)))
        .with_priority(1)
        .mount(&server)
        .await;
    server
}

/// #169 — the regression this whole fix exists for. Serving the realistic JSON
/// envelope, the imported document must have REAL block structure — a heading,
/// a paragraph, and a list — not one text node holding the JSON wrapper.
///
/// This is the assertion that would have caught the bug: a byte-count or
/// "not empty" check passes just as happily on a document whose entire body is
/// the literal string `{"html":"<h1 ...","response_metadata":...}`. Before the
/// fix, `thread_html` returned that JSON verbatim, every `<` was escaped, and
/// html5ever collapsed the document into a single text node.
#[tokio::test]
async fn content_pass_parses_the_json_envelope_into_real_block_structure() {
    common::require_infra!();
    let server = quip_server_with_rich_t1_envelope().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let doc_id = doc_id_for(&app, &import_id, "t1").await.expect("t1 imported");
    let snapshot = app
        .state
        .doc_repo
        .load_snapshot(&doc_id)
        .await
        .unwrap()
        .expect("snapshot exists");
    let doc = ogrenotes_collab::snapshot::deserialize(&snapshot).expect("decode snapshot");

    // THE ASSERTION: three top-level blocks (heading, paragraph, list), not
    // the single text node the JSON-garbling bug produced.
    {
        use yrs::types::xml::XmlFragment;
        use yrs::Transact;
        let txn = doc.inner().transact();
        let frag =
            ogrenotes_collab::document::get_content_fragment(&txn).expect("content fragment");
        assert_eq!(
            frag.len(&txn),
            3,
            "heading + paragraph + list are three real blocks, not one JSON text node",
        );
    }

    let html = ogrenotes_collab::export::to_html(doc.inner());
    assert!(html.contains("<h1"), "a real heading block must survive: {html}");
    assert!(html.contains("Heading One"), "the heading text must survive: {html}");
    assert!(
        html.contains("<ul") && html.contains("<li"),
        "a real list must survive: {html}",
    );
    // The JSON envelope's own keys must never appear as document text — their
    // presence is exactly the #169 garbling (the wrapper became the body).
    assert!(
        !html.contains("response_metadata") && !html.contains("next_cursor"),
        "the JSON wrapper must not have leaked into the document body: {html}",
    );
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

// ─── #233 Gap 2 — the spreadsheet path, end to end ────────────────
//
// `T2_HTML` above is `<p>numbers</p>`. It proves a spreadsheet *thread*
// becomes a `DocType::Spreadsheet` *document*, but it contains no table, so
// the one thing #230 added to this worker — reading Quip's `thread_type` and
// handing `QuipThreadKind` to the walker at the single call site in
// `worker_mode::import_one_thread` — is never exercised against grid markup.
// If that argument were dropped, or passed as `Document`, every imported
// spreadsheet would silently take its grid chrome as data and every existing
// test here would still pass.
//
// The collab crate pins the walker in isolation
// (`quip_corpus::corpus_spreadsheet_grid_chrome_is_not_imported_as_data`).
// What follows pins the *wiring*: Quip's JSON envelope → `thread_html` →
// staging → `from_quip_html_as` → snapshot → DynamoDB, and back out again.

/// The real `QGYAAAjicgG` spreadsheet body — Quip's own 31 × 17 grid,
/// 30 × 16 of which is data.
///
/// Reached from this crate by relative path off `CARGO_MANIFEST_DIR`, the
/// pattern the repo already uses to read across a crate boundary in a test:
/// `crates/highlight/tests/css_palette_sync.rs` embeds
/// `frontend/style/main.css` the same way, and `routes::ws`'s schema-duality
/// test reads `frontend/src/collab/ws_client.rs` with the same `concat!`.
/// `include_str!` rather than a runtime read on purpose — if the fixture is
/// moved or renamed this crate stops *compiling*, instead of one test
/// panicking at run time on a path nobody has looked at in a year.
///
/// **Do not replace this with hand-written markup.** Nine fidelity bugs in
/// this importer (#169, #173, #175, #176, #184, #187, #189, #230, and the
/// nested-list case) shared one root cause: fixtures encoding HTML Quip never
/// emits, so the tests passed while real documents broke. This file is the
/// bytes the worker actually staged from Quip, with only the prose scrubbed.
const T2_GRID_HTML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../collab/tests/fixtures/quip/corpus/QGYAAAjicgG.html"
));

/// [`quip_content_server`] with the spreadsheet thread's HTML fetch overridden
/// to the real grid fixture. Higher priority (lower number) than the 200
/// already mounted underneath, so no other test's expectations move.
async fn quip_server_with_grid_t2_envelope() -> MockServer {
    let server = quip_content_server().await;
    Mock::given(method("GET"))
        .and(path("/2/threads/t2/html"))
        .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(T2_GRID_HTML)))
        .with_priority(1)
        .mount(&server)
        .await;
    server
}

/// All text under `el`, descendants included, in document order.
fn element_text<T: yrs::ReadTxn>(txn: &T, el: &yrs::XmlElementRef) -> String {
    use yrs::types::xml::{XmlFragment, XmlOut};
    use yrs::{Any, Out, Text};

    let mut body = String::new();
    for i in 0..el.len(txn) {
        match el.get(txn, i) {
            Some(XmlOut::Text(text)) => {
                for delta in text.diff(txn, yrs::types::text::YChange::identity) {
                    if let Out::Any(Any::String(s)) = &delta.insert {
                        body.push_str(s.as_ref());
                    }
                }
            }
            Some(XmlOut::Element(child)) => body.push_str(&element_text(txn, &child)),
            _ => {}
        }
    }
    body
}

fn find_table<T: yrs::ReadTxn>(txn: &T, el: &yrs::XmlElementRef) -> Option<yrs::XmlElementRef> {
    use yrs::types::xml::{XmlFragment, XmlOut};

    if el.tag().as_ref() == "table" {
        return Some(el.clone());
    }
    for i in 0..el.len(txn) {
        if let Some(XmlOut::Element(child)) = el.get(txn, i)
            && let Some(found) = find_table(txn, &child)
        {
            return Some(found);
        }
    }
    None
}

/// The stored document's first `table`, row-major, as `(is_header_cell, text)`.
///
/// The census-style assertions elsewhere in this file count blocks; counting
/// cannot say *where* a value landed, and position is the whole of #230.
fn first_table_grid(doc: &yrs::Doc) -> Vec<Vec<(bool, String)>> {
    use yrs::types::xml::{XmlFragment, XmlOut};
    use yrs::Transact;

    let txn = doc.transact();
    let Some(frag) = ogrenotes_collab::document::get_content_fragment(&txn) else {
        return Vec::new();
    };
    let mut found = None;
    for i in 0..frag.len(&txn) {
        let Some(XmlOut::Element(el)) = frag.get(&txn, i) else { continue };
        if let Some(t) = find_table(&txn, &el) {
            found = Some(t);
            break;
        }
    }
    let Some(table) = found else { return Vec::new() };
    let mut grid = Vec::new();
    for r in 0..table.len(&txn) {
        let Some(XmlOut::Element(row)) = table.get(&txn, r) else { continue };
        let mut cells = Vec::new();
        for c in 0..row.len(&txn) {
            let Some(XmlOut::Element(cell)) = row.get(&txn, c) else { continue };
            cells.push((cell.tag().as_ref() == "table_header", element_text(&txn, &cell)));
        }
        grid.push(cells);
    }
    grid
}

/// Column `index` of a grid, top to bottom.
fn column(grid: &[Vec<(bool, String)>], index: usize) -> Vec<&str> {
    grid.iter().filter_map(|r| r.get(index)).map(|(_, t)| t.as_str()).collect()
}

/// **#230 / #233.** A Quip spreadsheet imported by the *worker* lands
/// unshifted — the column-letter header row and the `1..N` row-number gutter
/// are chrome, and neither becomes data.
///
/// Every assertion below is written to separate two outcomes rather than to
/// confirm the import merely succeeded, because both outcomes produce a
/// document with a table in it:
///
/// | | `QuipThreadKind::Spreadsheet` (correct) | `Document` (regressed) |
/// |---|---|---|
/// | rows × columns | 30 × 16 | 31 × 17 |
/// | header cells | 0 | 17 |
/// | row 1, columns A–D | `a1 b1 c1 d1` | the column letters |
/// | column A | data | `1`…`30` |
/// | column D's filled rows | 1, 6, 8–11 | 2, 7, 9–12 |
///
/// So if the kind stopped being threaded through `import_one_thread`, or were
/// passed as `Document`, this test goes red on the very first assertion and
/// on four more after it. (Verified by mutation: passing `Document` at that
/// call site fails `grid.len()` with `31 != 30`.)
#[tokio::test]
async fn content_pass_imports_a_spreadsheet_grid_unshifted() {
    common::require_infra!();
    let server = quip_server_with_grid_t2_envelope().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    // Precondition: this is the spreadsheet path. Quip typed t2
    // `"spreadsheet"`, and the worker agreed — without this the grid
    // assertions below could pass for the wrong reason.
    let sheet_doc_id = doc_id_for(&app, &import_id, "t2").await.expect("t2 imported");
    let meta = app.state.doc_repo.get(&sheet_doc_id).await.unwrap().expect("t2 document");
    assert_eq!(
        meta.doc_type,
        DocType::Spreadsheet,
        "precondition: the worker read Quip's thread_type as a spreadsheet",
    );

    let snapshot = app
        .state
        .doc_repo
        .load_snapshot(&sheet_doc_id)
        .await
        .unwrap()
        .expect("the sheet's snapshot must be persisted");
    let doc = ogrenotes_collab::snapshot::deserialize(&snapshot).expect("decode snapshot");
    let grid = first_table_grid(doc.inner());

    // The source table is 31 × 17. The extra row is the column-letter
    // `<thead>`; the extra column is the row-number gutter. Both are the
    // grid's own rulers, not the user's data.
    assert_eq!(
        grid.len(),
        30,
        "body rows only — the column-letter row is chrome, not a 31st row of data",
    );
    assert_eq!(
        grid.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![16; 30],
        "data columns only — the row-number gutter is chrome, not a 17th column",
    );

    // Every `<th>` in this document was a column letter, so with the header
    // row gone there is no header cell left anywhere.
    assert!(
        grid.iter().flatten().all(|(header, _)| !header),
        "a stripped sheet keeps no header cell",
    );

    // The top-left data cell. The fixture is content-scrubbed — each word run
    // became filler of the identical length — so Quip's `a1 b1 c1 d1` reads as
    // four 2-character strings. Their *values* are meaningless; that they are
    // the first four cells of the first row, rather than cells 2-5 of row 2,
    // is the assertion.
    assert_eq!(
        grid[0][..4].iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
        vec!["as", "as", "no", "it"],
        "row 1, columns A-D — Quip's a1..d1, unshifted",
    );

    // The shift was diagonal, so pin both axes. Under the bug column A held
    // 30 row numbers.
    let col_a = column(&grid, 0);
    assert!(
        !col_a.iter().all(|t| t.bytes().all(|b| b.is_ascii_digit())),
        "column A is the sheet's own first column, not the row-number ruler: {col_a:?}",
    );

    // The other axis, down column D — the one column of this sheet with
    // content spread through it. Quip has a value at D1 and D6 and a run of
    // four numbers at D8-D11; every other cell is a U+200B-only spacer. Under
    // the bug all of it read one row lower and one column right.
    let col_d = column(&grid, 3);
    let occupied: Vec<usize> = col_d
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.trim_matches('\u{200b}').is_empty())
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(occupied, vec![1, 6, 8, 9, 10, 11], "column D's filled rows, 1-based: {col_d:?}");
}

/// The input side of the test above, stated so a `30` there can never be read
/// as "the fixture only ever had 30 rows".
///
/// If someone ever trims the chrome out of the fixture itself, the end-to-end
/// assertions would still pass while testing nothing — this fails instead.
#[test]
fn the_grid_fixture_really_carries_the_chrome_being_stripped() {
    assert_eq!(
        T2_GRID_HTML.matches("<tr").count(),
        31,
        "31 source rows: the column-letter <thead> row plus 30 body rows",
    );
    assert!(
        T2_GRID_HTML.contains("<thead>"),
        "the column-letter header row must be present in the input",
    );
}

/// [`quip_content_server`] with the *document* thread's HTML overridden to the
/// same grid fixture. t1 is typed `"document"` by the mock `/1/threads/`
/// response, so this is byte-identical markup down the other branch.
async fn quip_server_with_grid_t1_envelope() -> MockServer {
    let server = quip_content_server().await;
    Mock::given(method("GET"))
        .and(path("/2/threads/t1/html"))
        .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(T2_GRID_HTML)))
        .with_priority(1)
        .mount(&server)
        .await;
    server
}

/// **The worker-level negative control.** The same bytes down a `document`
/// thread keep their header row and leading column.
///
/// Without this, `content_pass_imports_a_spreadsheet_grid_unshifted` passes
/// just as happily against a worker that hardcodes
/// `QuipThreadKind::Spreadsheet` and never reads `thread_type` at all — the
/// two tests together say the worker *discriminates*, not merely that it
/// strips. It is also the worker-side statement of why #230 could not use a
/// structural discriminator: 16 real prose tables in `document` threads carry
/// chrome byte-identical to this, and their `<th>` headings must survive.
///
/// The collab crate pins the same pair on the walker
/// (`the_same_grid_markup_imported_as_a_document_keeps_its_header_row`);
/// this pins that the worker hands it the right side of that pair.
#[tokio::test]
async fn the_same_grid_markup_in_a_document_thread_keeps_its_header_row() {
    common::require_infra!();
    let server = quip_server_with_grid_t1_envelope().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let doc_id = doc_id_for(&app, &import_id, "t1").await.expect("t1 imported");
    let meta = app.state.doc_repo.get(&doc_id).await.unwrap().expect("t1 document");
    assert_eq!(
        meta.doc_type,
        DocType::Document,
        "precondition: t1 is a document thread, whatever its body contains",
    );

    let snapshot = app.state.doc_repo.load_snapshot(&doc_id).await.unwrap().expect("snapshot");
    let doc = ogrenotes_collab::snapshot::deserialize(&snapshot).expect("decode snapshot");
    let grid = first_table_grid(doc.inner());

    assert_eq!(grid.len(), 31, "the header row is content in a document thread");
    assert_eq!(
        grid.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![17; 31],
        "17 cells per row — nothing is stripped off a document thread",
    );
    assert_eq!(
        grid[0].iter().filter(|(header, _)| *header).count(),
        17,
        "the whole first row survives as header cells",
    );
    assert!(
        column(&grid, 0)[1..].iter().all(|t| t.bytes().all(|b| b.is_ascii_digit())),
        "the leading column survives as content: {:?}",
        column(&grid, 0),
    );
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
    import_one_thread(
        &ctx,
        &import_id,
        "owner1",
        &client,
        &token,
        &retry_row,
        &folders,
        &mut PersonDirectory::default(),
    )
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

// ─── #155: a recoverable blob failure must not checkpoint the thread ─
//
// Step 10 marks a thread `ContentDone` unconditionally, and a `ContentDone`
// thread is skipped on every later run with zero Quip calls — the content
// pass's whole resumability guarantee. So dropping an image `src` and then
// checkpointing makes the loss **permanent**: re-running the import cannot
// recover it. The thread must instead stay retryable whenever a later attempt
// could still fetch the blob, and checkpoint only when it genuinely could not.

/// [`quip_content_server`] with t1's blob fetch overridden to `status`
/// forever. Higher priority (lower number) than the 200 mounted underneath.
async fn quip_server_with_blob_status(status: u16) -> MockServer {
    let server = quip_content_server().await;
    Mock::given(method("GET"))
        .and(path("/1/blob/t1/b9"))
        .respond_with(ResponseTemplate::new(status))
        .with_priority(1)
        .mount(&server)
        .await;
    server
}

/// Regression (#155): a **rate-limited** blob leaves the thread retryable.
///
/// This is the bug's headline shape. A 503 is Quip saying "not now", not "not
/// ever"; the pre-fix code dropped the `src`, kept the alt text and marked the
/// thread `ContentDone`, so the very next run skipped it and the document was
/// image-less forever. During a rate-limit storm that silently persists a whole
/// migration's worth of pictureless documents while reporting success.
///
/// Throttling is never one thread's fault, so it is a `RunFailure`: the pass
/// aborts, the queue's backoff does the waiting, and the thread is charged
/// **no** attempt — the same disposition a rate-limited `/2/threads/{id}/html`
/// already had.
///
/// Mutation check: restore the blanket `Err(e) => { drop the src; continue }`
/// arm in `sideload_images` and every assertion below goes red — the run
/// succeeds and t1 is `ContentDone` with no image.
#[tokio::test]
async fn a_rate_limited_blob_leaves_the_thread_retryable() {
    common::require_infra!();
    let server = quip_server_with_blob_status(503).await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    let outcome = execute_start_quip_import(&ctx, &import_id, "owner1").await;
    assert!(
        outcome.is_err(),
        "the pass must report itself incomplete so the queue retries it",
    );

    let t1 = thread_row(&app, &import_id, "t1").await;
    assert_eq!(
        t1.state,
        ThreadState::Pending,
        "a rate-limited image must leave the thread retryable, never ContentDone",
    );
    assert_eq!(
        t1.attempts, 0,
        "throttling is not the thread's fault, so it costs the thread no attempt",
    );
    // The doc id is reserved before the first durable write, so it exists —
    // what must not exist is a snapshot checkpointed without the image.
    let reserved = t1.ogre_doc_id.clone().expect("a doc id is reserved up front");
    assert!(
        app.state.doc_repo.load_snapshot(&reserved).await.unwrap().is_none(),
        "no document may be persisted without an image a retry could still fetch",
    );

    // ...and the recovery really works: serve the blob and re-run.
    server.reset().await;
    let healthy = quip_content_server().await;
    let ctx = worker_ctx_with_quip(&app, healthy.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let t1 = thread_row(&app, &import_id, "t1").await;
    assert_eq!(t1.state, ThreadState::ContentDone, "the retry finishes the thread");
    let snapshot = app.state.doc_repo.load_snapshot(&reserved).await.unwrap().expect("snapshot");
    let doc = ogrenotes_collab::snapshot::deserialize(&snapshot).expect("decode snapshot");
    assert_eq!(
        ogrenotes_collab::blob_ref::collect_blob_refs(doc.inner()).len(),
        1,
        "the image the first run could not fetch is present after the retry",
    );
}

/// The #142 attempt bound really does apply to the new blob path.
///
/// A Quip 5xx on a blob is recoverable, so it propagates — but propagating
/// without a bound would be an unbounded retry, which is the failure #142
/// exists to prevent. It routes through `ThreadImportError::Transient`, so the
/// per-thread counter charges it, the thread is `Failed` after
/// `MAX_THREAD_ATTEMPTS`, and the import still *completes* inside the queue's
/// budget with the healthy threads imported.
///
/// This is also the negative control for "a thread with no images is
/// unaffected": t2 has no image and imports normally throughout.
#[tokio::test]
async fn a_blob_that_always_5xxs_is_bounded_by_the_threads_attempt_budget() {
    common::require_infra!();
    let server = quip_server_with_blob_status(500).await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    let runs = run_like_the_queue(&ctx, &import_id, "owner1").await;
    assert!(
        runs <= QUEUE_RUNS,
        "the give-up must happen before the job dead-letters (took {runs} runs)",
    );

    let t1 = thread_row(&app, &import_id, "t1").await;
    assert_eq!(t1.attempts, 3, "the existing per-thread counter bounds the new path");
    assert_eq!(t1.state, ThreadState::Failed, "and the give-up is reached, not an endless retry");
    let reason = t1.reason.clone().unwrap_or_default();
    assert!(reason.contains("gave up"), "the reason says it gave up: {reason:?}");

    // A thread with no images is untouched by any of this.
    let t2 = thread_row(&app, &import_id, "t2").await;
    assert_eq!(t2.state, ThreadState::ContentDone, "an image-free thread imports normally");
    assert!(
        app.state.doc_repo.get(&t2.ogre_doc_id.clone().unwrap()).await.unwrap().is_some(),
        "the healthy thread's document must exist",
    );

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Succeeded, "one dead thread must not fail the import");
    let report = app.state.import_repo.get_report(&import_id).await.unwrap().expect("report");
    let failed = report
        .notes
        .iter()
        .find(|n| n.quip_thread_id == "t1")
        .expect("the failed thread must be named in the report");
    assert_eq!(failed.kind, "thread_failed", "no new note kind: the #142 kind already says it");
}

/// A **permanent** blob failure still drops the image and still checkpoints —
/// and must, because the alternative is strictly worse for the user.
///
/// A 404 blob is gone. Retrying it three times and then marking the thread
/// `Failed` would turn a document that imported fine-but-imageless into a
/// document that did not import at all. So the pre-#155 policy is exactly right
/// for this class and is deliberately kept: drop the `src`, keep the alt text,
/// name the loss on the report, and finish the thread. One run, no retries.
///
/// This is the negative control against the fix over-reaching: an unconditional
/// propagate would loop this thread to `Failed` and lose the document.
#[tokio::test]
async fn a_permanently_missing_blob_still_checkpoints_the_thread_in_one_run() {
    common::require_infra!();
    let server = quip_server_with_blob_status(404).await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    execute_start_quip_import(&ctx, &import_id, "owner1")
        .await
        .expect("a blob that will never exist must not fail the run");

    let t1 = thread_row(&app, &import_id, "t1").await;
    assert_eq!(
        t1.state,
        ThreadState::ContentDone,
        "the document is worth more than the picture it lost",
    );
    assert_eq!(t1.attempts, 0, "a decided loss costs no attempt: there is nothing to retry");
    assert_eq!(
        hits(&server, "/1/blob/t1/b9").await,
        1,
        "asked once and decided; a permanent failure must not be re-bought",
    );

    // The document exists, keeps its text, and has no dangling blob reference.
    let doc_id = t1.ogre_doc_id.clone().expect("t1 imported");
    let snapshot = app.state.doc_repo.load_snapshot(&doc_id).await.unwrap().expect("snapshot");
    let doc = ogrenotes_collab::snapshot::deserialize(&snapshot).expect("decode snapshot");
    assert!(
        ogrenotes_collab::blob_ref::collect_blob_refs(doc.inner()).is_empty(),
        "the src is dropped rather than left pointing at nothing",
    );

    // ...and the loss is named on the report under the kind that already
    // exists for it. No new note kind is spent on #155.
    let report = app.state.import_repo.get_report(&import_id).await.unwrap().expect("report");
    assert_eq!(report.counters.get("images_dropped"), Some(&1));
    let note = report
        .notes
        .iter()
        .find(|n| n.kind == "image_dropped")
        .expect("the dropped image must be named");
    assert_eq!(note.quip_thread_id, "t1");
    assert!(note.detail.contains("b9"), "the note names the blob: {note:?}");
}

// ─── silent content loss: live apps (#191) and formulas (#192) ───────
//
// These two are unlike every other loss this suite covers. A dropped image
// is a fetch that failed; a truncated nesting is a bound we chose. These are
// content that IS in the export, that the importer does not carry, and that
// the imported document gives the reader no way to notice: a Kanban board
// arrives as its column headings with no cards, a live spreadsheet as a grid
// of frozen numbers. The report note is the entire signal.

/// t2's body, carrying both losses at once.
///
/// The two `<td>` cells are **verbatim** from
/// `crates/collab/tests/fixtures/quip/corpus/QGYAAAjicgG.html` — the real
/// `<span formula=…>`-inside-`<td>` spelling, its text being the value Quip
/// last computed, and Quip's `<br/>` cell terminator.
///
/// The live-app block is **synthetic** and says so: no corpus fixture carries
/// one (`quip_corpus.rs` records that as a known coverage gap) and the
/// audit's Kanban thread was never checked in. Its *payload*, however, is
/// deliberately hostile — a card title spelling both a credential and an
/// address — because that payload is document content and the assertions
/// below are what stop it reaching a durable, user-visible note. A `detail`
/// that quoted what was lost would leak exactly the thing the loss is about.
const T2_LOSSY_HTML: &str = concat!(
    "<div data-live-app-id='kanban' data-live-app-payload='",
    r#"{"cards":[{"title":"tok-SEEKRET rejected for ada@example.com"}]}"#,
    "'><table><tr><th>To do</th><th>Done</th></tr></table></div>",
    "<table><tr>",
    "<td id='temp:s:temp:C:QGY02ded512fb3c4b019236db16b_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd' style=''>",
    "<span id='temp:s:temp:C:QGY02ded512fb3c4b019236db16b_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd' formula='=D8*D9'>3</span>\n\n<br/></td>",
    "<td id='temp:s:temp:C:QGY332cf016b08748b09637afc75_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd' style=''>",
    "<span id='temp:s:temp:C:QGY332cf016b08748b09637afc75_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd' formula='=SUM(D8:D10)'>4</span>\n\n<br/></td>",
    "</tr></table>",
);

/// [`quip_content_server`] with t2 serving [`T2_LOSSY_HTML`].
async fn quip_server_with_a_lossy_spreadsheet() -> MockServer {
    let server = quip_content_server().await;
    Mock::given(method("GET"))
        .and(path("/2/threads/t2/html"))
        .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(T2_LOSSY_HTML)))
        .with_priority(1)
        .mount(&server)
        .await;
    server
}

/// #191 + #192: a document whose live app and formulas were dropped still
/// imports, and the report says what it lost.
///
/// The import succeeding is half the contract and the easier half to get
/// wrong in the other direction — a loss that failed the thread would be a
/// far worse bug than the silence it replaced.
#[tokio::test]
async fn a_dropped_live_app_and_dropped_formulas_are_named_in_the_report() {
    common::require_infra!();
    let server = quip_server_with_a_lossy_spreadsheet().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    run_like_the_queue(&ctx, &import_id, "owner1").await;

    // The import is otherwise completely ordinary.
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Succeeded, "a named loss is not a failure");
    assert_eq!(thread_row(&app, &import_id, "t2").await.state, ThreadState::ContentDone);
    assert!(doc_id_for(&app, &import_id, "t2").await.is_some(), "the document still exists");

    let report = app
        .state
        .import_repo
        .get_report(&import_id)
        .await
        .expect("report read")
        .expect("an import that lost content must have a REPORT row");

    // Counters count *things*, not occasions — one note, two formulas.
    assert_eq!(
        report.counters.get("spreadsheet_formulas_dropped"),
        Some(&2),
        "both formulas counted: {report:?}",
    );
    assert_eq!(
        report.counters.get("live_apps_dropped"),
        Some(&1),
        "one live-app block: {report:?}",
    );
    assert_eq!(report.notes_dropped, 0, "nothing was over budget here: {report:?}");

    let formulas = report
        .notes
        .iter()
        .find(|n| n.kind == "formulas_dropped")
        .unwrap_or_else(|| panic!("the dropped formulas must be named: {report:?}"));
    assert_eq!(formulas.quip_thread_id, "t2", "the note names the document that lost them");
    assert!(formulas.detail.contains('2'), "the count is in the detail: {formulas:?}");

    let live_app = report
        .notes
        .iter()
        .find(|n| n.kind == "live_app_dropped")
        .unwrap_or_else(|| panic!("the dropped live app must be named: {report:?}"));
    assert_eq!(live_app.quip_thread_id, "t2");

    // One note per document per kind — a sheet with 300 formulas must spend
    // one of this kind's 25 notes, not 300.
    assert_eq!(
        report.notes.iter().filter(|n| n.kind == "formulas_dropped").count(),
        1,
        "one note per document, with the count in the detail: {report:?}",
    );
}

/// A note's `detail` is durable and user-visible. It may name **what** was
/// lost and **where**; it may never carry what the lost thing contained.
///
/// Asserted over both the rendered note and the raw DynamoDB item, because
/// the two are different surfaces and only one of them is what a future
/// export, support dump, or backup actually reads.
#[tokio::test]
async fn no_lost_content_reaches_a_report_note_or_the_raw_report_row() {
    common::require_infra!();
    let server = quip_server_with_a_lossy_spreadsheet().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    run_like_the_queue(&ctx, &import_id, "owner1").await;

    let report = app.state.import_repo.get_report(&import_id).await.unwrap().unwrap();
    let mut durable: Vec<String> = report.notes.iter().map(|n| n.detail.clone()).collect();
    assert!(!durable.is_empty(), "precondition: notes were written at all: {report:?}");

    // The raw item, exactly as it sits in DynamoDB — a `detail` that leaked
    // through some path the decoded view smooths over would still be here.
    let raw = app
        .dynamo_client()
        .get_item()
        .table_name(&app.table_name)
        .key("PK", AttributeValue::S(format!("IMPORT#{import_id}")))
        .key("SK", AttributeValue::S("REPORT".to_string()))
        .send()
        .await
        .expect("read the raw REPORT item")
        .item
        .expect("the REPORT row exists");
    durable.push(format!("{raw:?}"));

    for s in &durable {
        assert!(!s.contains("SEEKRET"), "a credential must never reach durable state: {s:?}");
        assert!(!s.contains('@'), "no address may reach durable state: {s:?}");
        assert!(
            !s.contains("cards"),
            "the live-app payload must not be quoted into the report: {s:?}",
        );
        assert!(
            !s.contains("D8") && !s.contains("SUM("),
            "a formula is document content and must not be quoted into the report: {s:?}",
        );
    }
}

/// The advisory rule, exercised on the new path specifically: a report row
/// that can never be written must not change what the import does with a
/// document that lost content.
///
/// `a_permanently_poisoned_report_row_does_not_affect_the_import` covers the
/// healthy document. This covers the one that has something to report — the
/// case where a caller who treated `record_report`'s outcome as meaningful
/// would fail a thread over its own bookkeeping.
#[tokio::test]
async fn a_broken_report_row_does_not_change_the_outcome_of_a_lossy_import() {
    common::require_infra!();
    let server = quip_server_with_a_lossy_spreadsheet().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    app.dynamo_client()
        .put_item()
        .table_name(&app.table_name)
        .item("PK", AttributeValue::S(format!("IMPORT#{import_id}")))
        .item("SK", AttributeValue::S("REPORT".to_string()))
        .item("owner_id", AttributeValue::S("owner1".to_string()))
        .item(
            "counters",
            AttributeValue::M(std::collections::HashMap::from([(
                "spreadsheet_formulas_dropped".to_string(),
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
        .expect("an unwritable report must not fail the import");

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Succeeded);
    assert_eq!(rec.phase, 2);
    let docs = app.state.doc_repo.query_docs_by_owner("owner1").await.unwrap();
    assert_eq!(docs.len(), 2, "both documents imported: {docs:?}");
    assert_eq!(thread_row(&app, &import_id, "t2").await.state, ThreadState::ContentDone);
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

/// #142 residual: MORE than `MAX_CONSECUTIVE_THREAD_FAILURES` sort-adjacent
/// deterministically-failing threads must NOT dead-letter the import.
///
/// The circuit breaker trips at 5 consecutive failures. The pre-fix breaker
/// counted *every* failure, so with 6+ adjacent bad threads it re-tripped at
/// the same offset on every run and returned `Err` before the walk ever
/// reached the threads past the trip point — those never got a first attempt,
/// never climbed to `Failed`, and the job dead-lettered with them still
/// `Pending`. That is #142 in miniature (bounded to N > 5 rather than N > 1,
/// but the same bug).
///
/// The fix counts only a thread's *first* failure toward the breaker, so a
/// known-bad thread stops re-arming it and the walk advances past the cluster
/// across runs. Seven adjacent failures (comfortably past the threshold of 5)
/// must now all reach `Failed` and the import must complete.
///
/// Seven, not more, because sort order is lexicographic: t1, t2, t3 … t9 stay
/// adjacent, but t10 would sort between t1 and t2. Seven is enough to prove
/// the property (> 5); the resolvable bound is 2× the threshold, documented on
/// `run_content_pass`.
#[tokio::test]
async fn more_than_five_adjacent_deterministic_failures_do_not_dead_letter() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    // t3..t9: seven adjacent threads with no `/2/threads/{id}/html` mock, so
    // each 404s -> Api{404} -> Transient, deterministically, every run.
    let failing: Vec<String> = (3..=9).map(|i| format!("t{i}")).collect();
    assert!(
        failing.len() > MAX_CONSECUTIVE_THREAD_FAILURES,
        "the fixture must exceed the breaker threshold to exercise the residual",
    );
    for t in &failing {
        seed_extra_thread(&app, &import_id, t).await;
    }

    let ctx = worker_ctx_with_quip(&app, server.uri());
    // Panics (naming the dead-letter) if the import doesn't complete inside the
    // queue's real 4-run budget — which is exactly what the pre-fix breaker
    // did here.
    let runs = run_like_the_queue(&ctx, &import_id, "owner1").await;
    assert!(runs <= QUEUE_RUNS, "must complete within the job budget (took {runs} runs)");

    // Every one of the seven is marked Failed — the pass reached and gave up
    // on all of them, none left stranded Pending.
    for t in &failing {
        let row = thread_row(&app, &import_id, t).await;
        assert_eq!(
            row.state,
            ThreadState::Failed,
            "{t} must be given up on, not stranded Pending (the dead-letter symptom)",
        );
        assert_eq!(row.attempts, 3, "{t} must have exhausted its per-thread budget");
    }

    // The healthy threads still imported — the whole point of not
    // dead-lettering.
    for t in ["t1", "t2"] {
        assert_eq!(thread_row(&app, &import_id, t).await.state, ThreadState::ContentDone);
    }
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.phase, 2, "the pass must run to completion");
    assert_eq!(
        rec.status,
        ImportStatus::Succeeded,
        "a cluster of bad threads must not fail the whole import",
    );
}

/// The property the first-attempt-only breaker had to NOT break: a *sustained*
/// Quip outage — one spanning multiple runs, so the leading threads are
/// already on attempt 2+ — must still trip the breaker every run and keep the
/// walk bounded, rather than walking the whole manifest charging attempts.
///
/// This is the exact regression the reviewer flagged as the risk of counting
/// only first failures: if attempt-2+ failures no longer arm the breaker, does
/// run 2 of an outage still stop early? It does — because a tripping breaker
/// never lets the walk get ahead of its leading edge, so on run 2 the deep
/// threads are still on their *first* failure and re-arm it. The known-bad
/// leading threads don't, but they aren't what an outage's blast radius is
/// made of; the fresh leading edge is.
///
/// Twelve adjacent bad threads (u03..u14, zero-padded so they sort adjacently
/// and after the fixture's t-threads). Run 1 trips after 5 fresh failures
/// (u03..u07); run 2's leading five (u03..u07) are now attempt-2 and do NOT
/// arm the breaker, yet run 2 still trips — on the next five fresh threads
/// (u08..u12) — leaving u13/u14 untouched. That is containment surviving into
/// a sustained outage.
#[tokio::test]
async fn a_sustained_outage_still_trips_the_breaker_on_the_second_run() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let bad: Vec<String> = (3..=14).map(|i| format!("u{i:02}")).collect(); // u03..u14
    for t in &bad {
        seed_extra_thread(&app, &import_id, t).await;
    }
    let ctx = worker_ctx_with_quip(&app, server.uri());

    // Run 1: trips on the first five fresh failures.
    execute_start_quip_import(&ctx, &import_id, "owner1")
        .await
        .expect_err("run 1 must trip the breaker");
    for t in &bad[..MAX_CONSECUTIVE_THREAD_FAILURES] {
        assert_eq!(thread_row(&app, &import_id, t).await.attempts, 1, "{t} charged in run 1");
    }
    assert_eq!(
        thread_row(&app, &import_id, "u08").await.attempts,
        0,
        "run 1 must have stopped before the sixth bad thread",
    );

    // Run 2: the leading five are now attempt-2 (known-bad, do NOT arm the
    // breaker), yet the run must STILL trip — on the next five fresh threads.
    execute_start_quip_import(&ctx, &import_id, "owner1")
        .await
        .expect_err("run 2 must still trip during a sustained outage, not walk the whole manifest");

    // The already-charged leading threads advanced (attempt 2), proving they
    // were re-walked but did not, by themselves, trip the breaker.
    for t in &bad[..MAX_CONSECUTIVE_THREAD_FAILURES] {
        assert_eq!(thread_row(&app, &import_id, t).await.attempts, 2, "{t} re-charged in run 2");
    }
    // The next five fresh threads got their first attempt and tripped it.
    for t in &bad[MAX_CONSECUTIVE_THREAD_FAILURES..2 * MAX_CONSECUTIVE_THREAD_FAILURES] {
        assert_eq!(thread_row(&app, &import_id, t).await.attempts, 1, "{t} first-charged in run 2");
    }
    // THE ASSERTION: the walk stayed bounded — the twelfth thread was never
    // reached in either run, so run 2 did not walk the whole manifest.
    assert_eq!(
        thread_row(&app, &import_id, "u14").await.attempts,
        0,
        "a sustained outage must not walk the whole manifest — u14 must be untouched",
    );
    assert_eq!(
        hits(&server, "/2/threads/u14/html").await,
        0,
        "no Quip call may be spent on threads past the breaking point",
    );
}

// ─── person mentions: a Quip @person becomes a real OgreNotes mention ───
//
// The markup below is copied verbatim from a real staged `/2` thread body.
// That matters more here than anywhere else in this file: a person mention
// and a folder link reach the walker as anchors at *identically shaped* Quip
// URLs, and the only thing separating them is the `<control>` wrapper. A
// hand-simplified fixture is precisely the tool that cannot notice when the
// wrapper stops surviving the sanitizer — which is the bug these tests pin.

/// One document carrying all three shapes: a `<control>`-wrapped person
/// mention, a **bare** folder link, and an empty `<control>` (a Quip date,
/// which the client renders and the export therefore does not carry).
const TM_HTML: &str = concat!(
    r#"<p id="sec-m">Assign tasks by mentioning someone: "#,
    r#"<control data-remapped="true" id="SSfACAGTvYT">"#,
    r#"<a href="https://quip.com/XYJAEA0Sgev">Joel</a></control>.</p>"#,
    r#"<p>When you're done, check out your folder: "#,
    r#"<a href="https://quip.com/JAdAOAxYGcQ">Family</a></p>"#,
    r#"<p>Complete by <control data-remapped="true" id="SSfACAsTxeJ"></control>.</p>"#,
);

/// The Quip person's id, as it appears in the mention anchor's href.
const QUIP_PERSON_ID: &str = "XYJAEA0Sgev";

/// The email Quip reports for that person. Whether an OgreNotes account
/// exists for it is the single variable between the two tests below — and
/// this string must never appear in anything the import writes.
const QUIP_PERSON_EMAIL: &str = "joel.quip.person@example.com";

/// A wiremock Quip serving one folder with one mention-bearing thread, plus
/// the `/1/users/` batch lookup answering with the person's real profile.
async fn quip_mention_server() -> MockServer {
    quip_mention_server_with_users(ResponseTemplate::new(200).set_body_json(
        serde_json::json!({ QUIP_PERSON_ID: {"name": "Joel", "emails": [QUIP_PERSON_EMAIL]} }),
    ))
    .await
}

/// [`quip_mention_server`] with the `/1/users/` response under the caller's
/// control. Wiremock resolves mocks in **mount order**, so a later mock
/// cannot override an earlier one — the response has to be chosen here.
async fn quip_mention_server_with_users(users_response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .and(query_param("ids", "mroot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "mroot": {
                "folder": {"id": "mroot", "title": "Root"},
                "children": [ {"thread_id": "tm"} ]
            }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/1/threads/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tm": {"thread": {"id": "tm", "title": "Mentions", "type": "document", "updated_usec": 999}}
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/2/threads/tm/html"))
        .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(TM_HTML)))
        .mount(&server)
        .await;

    // The person lookup: Quip keys the batch response by the requested id.
    Mock::given(method("GET"))
        .and(path("/1/users/"))
        .respond_with(users_response)
        .mount(&server)
        .await;

    server
}

/// [`seed_scoping_import`] scoped to the mention fixture's root folder.
async fn seed_mention_import(app: &common::TestApp, owner: &str) -> String {
    let import_id = format!("imp-{}", nanoid::nanoid!(8));
    let now = ogrenotes_common::time::now_usec();
    let record = ImportRecord {
        import_id: import_id.clone(),
        owner_id: owner.to_string(),
        status: ImportStatus::Scoping,
        phase: 0,
        quip_user_id: None,
        target_folder_id: Some("target-folder".to_string()),
        import_folder_id: None,
        selected_roots: vec!["mroot".to_string()],
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

/// The stored document, rendered back to HTML — the shape a reader sees.
async fn imported_html(app: &common::TestApp, doc_id: &str) -> String {
    let snapshot = app.state.doc_repo.load_snapshot(doc_id).await.unwrap().expect("snapshot");
    let doc = ogrenotes_collab::snapshot::deserialize(&snapshot).expect("decode snapshot");
    ogrenotes_collab::export::to_html(doc.inner())
}

#[tokio::test]
async fn a_person_mention_matching_an_ogrenotes_email_becomes_a_real_mention() {
    common::require_infra!();
    let server = quip_mention_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    // The OgreNotes account whose email is the one Quip reports.
    let (matched_user_id, _) = app.create_user(QUIP_PERSON_EMAIL).await;
    let import_id = seed_mention_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let doc_id = doc_id_for(&app, &import_id, "tm").await.expect("tm imported");
    let html = imported_html(&app, &doc_id).await;

    // THE ASSERTION: a real mention of the matched OgreNotes user.
    assert!(
        html.contains(&format!("data-user-id=\"{matched_user_id}\"")),
        "the mention must reference the matched OgreNotes user: {html}",
    );
    assert!(html.contains("class=\"mention\""), "rendered as a mention chip: {html}");
    assert!(html.contains("Joel"), "the person's name survives: {html}");
    assert!(
        !html.contains(QUIP_PERSON_ID),
        "the Quip person id must not survive into the document: {html}",
    );

    // The bare folder link in the same document is untouched — still the
    // ordinary intra-Quip document-link placeholder.
    assert!(html.contains("doc-mention"), "the folder link is still a doc link: {html}");
    let unresolved = app.state.import_repo.list_unresolved(&import_id).await.unwrap();
    let links: Vec<String> = unresolved
        .iter()
        .flat_map(|u| u.links.iter().map(|l| l.target_quip_thread_id.clone()))
        .collect();
    assert_eq!(
        links,
        vec!["JAdAOAxYGcQ".to_string()],
        "only the bare anchor may be recorded as a pending document link",
    );

    // The empty `<control>` left its sentence alone.
    assert!(html.contains("Complete by ."), "{html}");

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Succeeded);
}

#[tokio::test]
async fn an_unmatched_person_mention_degrades_to_the_name_and_the_import_succeeds() {
    common::require_infra!();
    let server = quip_mention_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    // No OgreNotes account carries `QUIP_PERSON_EMAIL` — but one carries the
    // same *display name*. Matching is exact-email-only precisely so this
    // person is NOT mistaken for the Quip "Joel": a mention attributed to
    // the wrong human is worse than an unresolved one, which is why the
    // design gates fuzzy identity behind a Phase-3 confirm step.
    let (decoy_user_id, _) =
        app.create_user_with_name("joel.someone.else@example.com", "Joel").await;
    let import_id = seed_mention_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let tm = threads.iter().find(|t| t.quip_thread_id == "tm").unwrap();
    assert_eq!(tm.state, ThreadState::ContentDone, "an unresolvable mention never fails a thread");

    let doc_id = tm.ogre_doc_id.clone().expect("tm imported");
    let html = imported_html(&app, &doc_id).await;

    // THE ASSERTION: the person's NAME, not a mention of nobody and not a
    // "Missing document" chip.
    assert!(html.contains("@Joel"), "the person's name survives as text: {html}");
    assert!(
        !html.contains("class=\"mention\""),
        "no mention chip may be emitted without a real user: {html}",
    );
    assert!(!html.contains("data-user-id"), "{html}");
    assert!(!html.contains(QUIP_PERSON_ID), "{html}");
    assert!(
        !html.contains(&decoy_user_id),
        "a same-name account with a different email must NEVER be matched: {html}",
    );
    // The only doc-mention in the document is the *folder link*; the person
    // must not have become one.
    assert_eq!(html.matches("doc-mention").count(), 1, "{html}");
    assert!(html.contains("Family"), "the folder link is untouched: {html}");

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Succeeded, "the import still succeeds");
}

/// **Security spine.** Resolving a mention is the first thing in the import
/// worker that handles an email address. It may reach `UserRepo` and nothing
/// else: not the stored document, not the report, not a thread's reason.
///
/// Run over both outcomes — matched and unmatched — because they take
/// different code paths through the resolver, and the unmatched one is the
/// one that writes a fallback string.
#[tokio::test]
async fn no_email_reaches_the_document_the_report_or_a_thread_reason() {
    common::require_infra!();
    for matched in [true, false] {
        let server = quip_mention_server().await;
        let app = common::TestApp::new_with_quip_base(server.uri()).await;
        if matched {
            app.create_user(QUIP_PERSON_EMAIL).await;
        }
        let import_id = seed_mention_import(&app, "owner1").await;

        let ctx = worker_ctx_with_quip(&app, server.uri());
        execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

        let doc_id = doc_id_for(&app, &import_id, "tm").await.expect("tm imported");

        // 1. The document itself — both the rendered form and the raw CRDT
        //    bytes, so an email hiding in an attribute is caught too.
        let html = imported_html(&app, &doc_id).await;
        assert!(!html.contains(QUIP_PERSON_EMAIL), "matched={matched}: {html}");
        let snapshot = app.state.doc_repo.load_snapshot(&doc_id).await.unwrap().unwrap();
        assert!(
            !String::from_utf8_lossy(&snapshot).contains(QUIP_PERSON_EMAIL),
            "matched={matched}: an email must not be stored in the snapshot",
        );

        // 2. The import report — counters and every note's detail.
        if let Some(report) = app.state.import_repo.get_report(&import_id).await.unwrap() {
            for note in &report.notes {
                assert!(
                    !note.detail.contains(QUIP_PERSON_EMAIL)
                        && !note.kind.contains(QUIP_PERSON_EMAIL),
                    "matched={matched}: an email reached a report note: {note:?}",
                );
            }
        }

        // 3. Every thread row's user-visible reason.
        for thread in app.state.import_repo.list_threads(&import_id).await.unwrap() {
            assert!(
                !thread.reason.unwrap_or_default().contains(QUIP_PERSON_EMAIL),
                "matched={matched}: an email reached a thread reason",
            );
        }
    }
}

/// The rate-limit property: Quip allows 50 requests/minute per token, so the
/// same person mentioned across many documents must cost **one** lookup for
/// the whole import, not one per thread — and a batch of people must cost one
/// request, not one each.
#[tokio::test]
async fn person_lookups_are_cached_across_threads_and_batched_within_one() {
    common::require_infra!();
    let server = MockServer::start().await;
    // Three threads; the same two people mentioned in each.
    let body = concat!(
        r#"<p><control><a href="https://quip.com/PERSONA">Ann</a></control> and "#,
        r#"<control><a href="https://quip.com/PERSONB">Bob</a></control></p>"#,
    );
    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .and(query_param("ids", "mroot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "mroot": {
                "folder": {"id": "mroot", "title": "Root"},
                "children": [ {"thread_id": "x1"}, {"thread_id": "x2"}, {"thread_id": "x3"} ]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/1/threads/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "x1": {"thread": {"id": "x1", "title": "A", "type": "document", "updated_usec": 1}},
            "x2": {"thread": {"id": "x2", "title": "B", "type": "document", "updated_usec": 2}},
            "x3": {"thread": {"id": "x3", "title": "C", "type": "document", "updated_usec": 3}}
        })))
        .mount(&server)
        .await;
    for t in ["x1", "x2", "x3"] {
        Mock::given(method("GET"))
            .and(path(format!("/2/threads/{t}/html")))
            .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(body)))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/1/users/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "PERSONA": {"name": "Ann", "emails": ["ann.quip@example.com"]},
            "PERSONB": {"name": "Bob", "emails": ["bob.quip@example.com"]}
        })))
        .mount(&server)
        .await;

    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_mention_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    // THE ASSERTION: six mentions over three documents, one Quip request.
    assert_eq!(
        hits(&server, "/1/users/").await,
        1,
        "two distinct people across three threads must cost exactly one batched lookup",
    );
}

/// End-to-end: a `<control>`-wrapped chip Quip does not know as a person is
/// a **document link**, and the pending-link row Phase 2b back-patches from
/// is written for it.
///
/// This is the corpus's majority case. `<control>` wraps folder and thread
/// chips as well as people — in the 56 staged documents there are four
/// wrapped `quip.com` anchors, only one of which is a person, and **zero**
/// bare ones. Classifying all of them as people would degrade the other
/// three to plain `@Title` text and destroy their back-patch, which is the
/// mirror of the bug this feature fixes. Quip's own "no such user" answer is
/// what separates them, and it costs no extra request.
///
/// Both chips below are verbatim from `SSfAAALs7fy`, wrapper included.
///
/// Mutation check: map the no-profile case to `PersonFact::NoAccount` and
/// this goes red — the doc-mention count drops to zero, no unresolved row is
/// written, and the export reads `@Family`.
#[tokio::test]
async fn a_control_wrapped_non_person_stays_a_back_patchable_document_link() {
    common::require_infra!();
    const BODY: &str = concat!(
        r#"<p id="sec-m">Assign tasks by mentioning someone: "#,
        r#"<control data-remapped="true" id="SSfACAGTvYT">"#,
        r#"<a href="https://quip.com/XYJAEA0Sgev">Joel</a></control>.</p>"#,
        r#"<p>When you're done, check out your folder: "#,
        r#"<control data-remapped="true" id="SSfACA1I4lV">"#,
        r#"<a href="https://quip.com/JAdAOAxYGcQ">Family</a></control></p>"#,
    );
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .and(query_param("ids", "mroot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "mroot": {
                "folder": {"id": "mroot", "title": "Root"},
                "children": [ {"thread_id": "tm"} ]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/1/threads/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tm": {"thread": {"id": "tm", "title": "Mentions", "type": "document", "updated_usec": 9}}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/2/threads/tm/html"))
        .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(BODY)))
        .mount(&server)
        .await;
    // Quip answers for the person and says nothing about the folder id —
    // exactly how the real API reports "that is not a user".
    Mock::given(method("GET"))
        .and(path("/1/users/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            QUIP_PERSON_ID: {"name": "Joel", "emails": [QUIP_PERSON_EMAIL]}
        })))
        .mount(&server)
        .await;

    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let (matched_user_id, _) = app.create_user(QUIP_PERSON_EMAIL).await;
    let import_id = seed_mention_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let doc_id = doc_id_for(&app, &import_id, "tm").await.expect("tm imported");
    let html = imported_html(&app, &doc_id).await;

    // THE ASSERTION: the folder chip is a document link, not degraded text.
    assert_eq!(html.matches("doc-mention").count(), 1, "the folder chip is a doc link: {html}");
    assert!(html.contains("Family"), "{html}");
    assert!(!html.contains("@Family"), "it must not degrade to plain text: {html}");

    // And Phase 2b has something to back-patch.
    let unresolved = app.state.import_repo.list_unresolved(&import_id).await.unwrap();
    let links: Vec<String> = unresolved
        .iter()
        .flat_map(|u| u.links.iter().map(|l| l.target_quip_thread_id.clone()))
        .collect();
    assert_eq!(
        links,
        vec!["JAdAOAxYGcQ".to_string()],
        "the wrapped folder link must be recorded for the back-patch",
    );

    // The real person in the same document is still a real mention — the two
    // outcomes are distinct, on markup that is byte-identical in shape.
    assert!(
        html.contains(&format!("data-user-id=\"{matched_user_id}\"")),
        "the person is still resolved: {html}",
    );
    assert!(!html.contains(QUIP_PERSON_ID), "no Quip person id survives: {html}");
}

/// A repeated **storage** failure in the identity lookup must never mark the
/// thread `Failed`.
///
/// The dispositions on `ThreadImportError` are explicit that a DynamoDB or S3
/// failure is "not attributable to this thread at all", and deliberately not
/// charged to the thread's attempt budget, "which would otherwise let a
/// storage blip condemn an innocent thread". Routing every undecided lookup
/// to a thread-charged `Transient` would break exactly that: `MAX_THREAD_ATTEMPTS`
/// runs of a DynamoDB outage would mark a perfectly good document `Failed`
/// and lose it, over something that was never about the document.
///
/// The `UserRepo` here points at a table that does not exist, so every
/// `get_by_email` errors — the shape of a real outage, held for longer than
/// the thread's whole budget.
///
/// Mutation check: map `LookupFault::Storage` to `transient(...)` and this
/// goes red — attempts climb and the thread ends `Failed`.
#[tokio::test]
async fn a_repeated_identity_store_failure_never_marks_the_thread_failed() {
    common::require_infra!();
    let server = quip_mention_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    app.create_user(QUIP_PERSON_EMAIL).await;
    let import_id = seed_mention_import(&app, "owner1").await;

    // A UserRepo bound to a table that does not exist: every read errors.
    let broken_users = std::sync::Arc::new(ogrenotes_storage::repo::user_repo::UserRepo::new(
        ogrenotes_storage::dynamo::DynamoClient::new(
            app.dynamo_client().clone(),
            "ogrenotes-no-such-table".to_string(),
        ),
    ));
    let ctx = WorkerCtx::new(
        app.state.doc_repo.clone(),
        app.state.folder_repo.clone(),
        app.state.doc_repo.s3().clone(),
        app.state.import_repo.clone(),
        broken_users,
        app.state.quip_token_store.clone(),
        Some(server.uri()),
    );

    // Run the pass more times than the thread's own attempt budget.
    // Deliberately no per-iteration assertion: the state after the outage is
    // what matters, and asserting it last keeps the failure message pointed
    // at the property rather than at an intermediate.
    let mut last_failed = false;
    for _ in 0..5 {
        last_failed = execute_start_quip_import(&ctx, &import_id, "owner1").await.is_err();
    }

    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let tm = threads.iter().find(|t| t.quip_thread_id == "tm").unwrap();
    assert_ne!(
        tm.state,
        ThreadState::Failed,
        "a storage outage must never condemn a thread (attempts={})",
        tm.attempts,
    );
    assert_eq!(tm.state, ThreadState::Pending, "it stays retryable");
    assert_eq!(tm.attempts, 0, "and is not charged a single attempt");
    assert!(last_failed, "the job keeps failing so the queue keeps retrying it");
}

/// A **transient** person-lookup failure must not be checkpointed.
///
/// Step 10 marks a thread `ContentDone` unconditionally and a re-run skips a
/// `ContentDone` thread with zero Quip calls, so checkpointing here would make
/// a seconds-long Quip 5xx permanently degrade every mention it touched —
/// issue #155's pattern, and worse than #155 because this path writes no
/// report note, so the loss would be undiscoverable. The thread must instead
/// stay `Pending` under its existing attempt budget.
///
/// The account DOES exist, which is what makes this a *failure* test rather
/// than a no-match test: the only reason the mention is unresolvable is that
/// Quip erred.
///
/// Mutation check: make `PersonDirectory::resolve` report `undecided: 0`
/// unconditionally (or drop the `undecided > 0` bail in step 6b) and the
/// thread checkpoints `ContentDone` with `@Joel` baked in — every assertion
/// below goes red.
#[tokio::test]
async fn a_transient_person_lookup_failure_does_not_checkpoint_the_thread() {
    common::require_infra!();
    let server = quip_mention_server_with_users(ResponseTemplate::new(500)).await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    app.create_user(QUIP_PERSON_EMAIL).await;
    let import_id = seed_mention_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    let outcome = execute_start_quip_import(&ctx, &import_id, "owner1").await;
    assert!(outcome.is_err(), "the pass reports itself incomplete so the queue retries it");

    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let tm = threads.iter().find(|t| t.quip_thread_id == "tm").unwrap();
    assert_eq!(
        tm.state,
        ThreadState::Pending,
        "a recoverable lookup failure must leave the thread retryable, not checkpointed",
    );
    assert_eq!(tm.attempts, 1, "charged to the thread's own attempt budget");
    // The doc id is *reserved* before the first durable write, so it exists;
    // what must not exist is a snapshot carrying the degraded placeholders.
    let reserved = tm.ogre_doc_id.clone().expect("a doc id is reserved up front");
    assert!(
        app.state.doc_repo.load_snapshot(&reserved).await.unwrap().is_none(),
        "no document may be persisted with mentions that a retry could still resolve",
    );
}

/// A person Quip **decides** we cannot match still degrades permanently and
/// silently — that is correct, and the retry above must not swallow it.
///
/// Quip answers 200 with a profile whose email belongs to nobody in
/// OgreNotes: no retry could widen that, so the thread checkpoints.
#[tokio::test]
async fn a_decided_no_match_still_checkpoints_the_thread() {
    common::require_infra!();
    let server = quip_mention_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_mention_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let tm = threads.iter().find(|t| t.quip_thread_id == "tm").unwrap();
    assert_eq!(tm.state, ThreadState::ContentDone, "a decided no-match never fails the thread");
    let html = imported_html(&app, &tm.ogre_doc_id.clone().unwrap()).await;
    assert!(html.contains("@Joel"), "degraded to the name: {html}");
    assert!(!html.contains("class=\"mention\""), "{html}");
}

/// A **permanently** wrong `/1/users/` endpoint costs one request for the
/// whole run, not one per mention-bearing thread.
///
/// The `?ids=` batch shape is an openly documented assumption (Quip's own
/// reference client spells batch calls as form posts), so "the endpoint is
/// simply wrong" is a live possibility — and re-asking once per thread would
/// spend a 1 000-thread import's entire 50 req/min budget on a request that
/// can never succeed. A 4xx is a *decision*: degrade every mention once and
/// stop asking. The import still succeeds.
///
/// Mutation check: drop the `is_permanent_lookup_failure` arm and this test
/// sees three `/1/users/` hits (one per thread) instead of one — and, because
/// an uncached failure is undecided, all three threads stay `Pending` and the
/// import never succeeds.
#[tokio::test]
async fn a_permanently_wrong_user_endpoint_is_asked_once_per_run_not_once_per_thread() {
    common::require_infra!();
    let server = MockServer::start().await;
    let body = concat!(
        r#"<p><control><a href="https://quip.com/PERSONA">Ann</a></control> and "#,
        r#"<control><a href="https://quip.com/PERSONB">Bob</a></control></p>"#,
    );
    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .and(query_param("ids", "mroot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "mroot": {
                "folder": {"id": "mroot", "title": "Root"},
                "children": [ {"thread_id": "x1"}, {"thread_id": "x2"}, {"thread_id": "x3"} ]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/1/threads/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "x1": {"thread": {"id": "x1", "title": "A", "type": "document", "updated_usec": 1}},
            "x2": {"thread": {"id": "x2", "title": "B", "type": "document", "updated_usec": 2}},
            "x3": {"thread": {"id": "x3", "title": "C", "type": "document", "updated_usec": 3}}
        })))
        .mount(&server)
        .await;
    for t in ["x1", "x2", "x3"] {
        Mock::given(method("GET"))
            .and(path(format!("/2/threads/{t}/html")))
            .respond_with(ResponseTemplate::new(200).set_body_json(html_envelope(body)))
            .mount(&server)
            .await;
    }
    // The endpoint shape is wrong. It will be wrong every time.
    Mock::given(method("GET"))
        .and(path("/1/users/"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_mention_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    // THE ASSERTION: one doomed request for the run, not one per thread.
    assert_eq!(
        hits(&server, "/1/users/").await,
        1,
        "a permanently-failing lookup endpoint must be asked exactly once per run",
    );

    // And the loss is bounded: every thread still imports, with names.
    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    assert_eq!(threads.len(), 3);
    for t in &threads {
        assert_eq!(t.state, ThreadState::ContentDone, "thread {} imported", t.quip_thread_id);
        let html = imported_html(&app, &t.ogre_doc_id.clone().unwrap()).await;
        assert!(html.contains("@Ann") && html.contains("@Bob"), "degraded to names: {html}");
        assert!(!html.contains("class=\"mention\""), "{html}");
    }

    // And the loss is DISCOVERABLE. A run-wide degradation behind a
    // `tracing::warn!` alone is invisible to the person who ran the import —
    // the same undiscoverability that makes a silent `ContentDone`
    // checkpoint wrong. It must reach the report the user reads.
    let report = app.state.import_repo.get_report(&import_id).await.unwrap().expect("a report");
    assert!(
        report.counters.get("threads_mentions_degraded").copied().unwrap_or(0) > 0,
        "the degraded mentions must be counted: {:?}",
        report.counters,
    );
    let note = report
        .notes
        .iter()
        .find(|n| n.kind == "mentions_degraded")
        .expect("a note naming the systemic cause");
    assert!(
        note.detail.contains("plain text"),
        "the note must say what the user actually lost: {:?}",
        note.detail,
    );
    assert_eq!(
        report.notes.iter().filter(|n| n.kind == "mentions_degraded").count(),
        1,
        "the run-wide cause is named once, not once per thread",
    );

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Succeeded, "the import still succeeds");
}

// ─── #196: staged thread HTML must not outlive the import ─────────

/// The staged objects under `imports/{import_id}/threads/` are the user's
/// **full document text** — on the test stack that demonstrably included
/// brokerage holdings with account numbers and correspondence naming a real
/// individual. Nothing deleted them, and deleting the imported OgreNotes
/// documents does not reach them (they are keyed by *import* id, not doc id),
/// so every import retained a second copy of everything, forever, even when it
/// succeeded.
///
/// The sweep hangs off the import's **terminal status**, never off the job's
/// finalization: see `a_retryable_run_keeps_its_staging_for_the_retry` for the
/// negative that protects the in-flight run.
#[tokio::test]
async fn a_succeeded_import_deletes_its_staged_html() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Succeeded, "precondition: the import is terminal");

    // The objects are GONE, asserted against the bucket itself — both by
    // listing the prefix and by asking S3 for each key by name.
    let left = staged_keys(&app, &import_id).await;
    assert!(left.is_empty(), "the staging prefix must be empty after a succeeded import: {left:?}");
    for thread in ["t1", "t2"] {
        let key = format!("imports/{import_id}/threads/{thread}.html");
        assert!(head_is_404(&app, &key).await, "{key} must be a 404, not merely unlisted");
    }

    // What the user actually asked for survives: the documents and the images
    // side-loaded out of Quip into the document's own blob prefix. A sweep
    // that took those with it would be a far worse bug than the one it fixes.
    let t1_doc_id = doc_id_for(&app, &import_id, "t1").await.expect("t1 imported");
    assert!(app.state.doc_repo.get(&t1_doc_id).await.unwrap().is_some(), "the document survives");
    assert!(
        !keys_under(&app, &format!("blobs/{t1_doc_id}/")).await.is_empty(),
        "the side-loaded image must survive the staging sweep",
    );
}

/// **The load-bearing negative.** A run that ends retryable must leave every
/// staged object in place.
///
/// A mid-run sweep would delete the in-flight run's diagnostic material — the
/// one copy of the raw HTML that does not cost a round trip against Quip's
/// rate budget to obtain — and it would do so at exactly the moment something
/// has gone wrong and that material is worth most. The queue is still going to
/// re-run this import: it is not terminal, and only terminal imports may be
/// swept.
#[tokio::test]
async fn a_retryable_run_keeps_its_staging_for_the_retry() {
    common::require_infra!();
    // t1 imports and stages; t2 500s, which is transient and under its attempt
    // budget, so the pass walks the whole manifest and then returns Err for the
    // queue to retry.
    let server = quip_server_with_thread_html_status("t2", 500).await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    let ctx = worker_ctx_with_quip(&app, server.uri());
    let outcome = execute_start_quip_import(&ctx, &import_id, "owner1").await;
    assert!(outcome.is_err(), "precondition: this run is retryable, not terminal: {outcome:?}");

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        ImportStatus::Running,
        "precondition: a retryable run leaves the import non-terminal",
    );

    // THE ASSERTION: the staged HTML is still there, byte-for-byte.
    let staged = app
        .state
        .doc_repo
        .s3()
        .get_object(&format!("imports/{import_id}/threads/t1.html"))
        .await
        .expect("a retryable run must not delete its staged thread HTML");
    assert_eq!(
        String::from_utf8(staged).unwrap(),
        T1_HTML,
        "the staged HTML must survive intact, not be replaced or truncated",
    );
}

/// The other terminal state. An import that ends `Failed` retains exactly as
/// much of the user's document text as one that ends `Succeeded`, so it is
/// swept on the same terms.
///
/// Driven through the real sequence that produces it: a first run stages t1 and
/// ends retryable, then a selected root becomes unreadable (403), which is
/// terminal-as-`Failed` (the credential is valid, so a reconnect would not
/// help) rather than terminal-as-`TokenRejected`.
#[tokio::test]
async fn a_failed_import_deletes_its_staged_html() {
    common::require_infra!();
    let server = quip_server_with_thread_html_status("t2", 500).await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;
    let ctx = worker_ctx_with_quip(&app, server.uri());

    // Run 1: t1 is imported and staged; the run ends retryable.
    assert!(execute_start_quip_import(&ctx, &import_id, "owner1").await.is_err());
    assert_eq!(
        staged_keys(&app, &import_id).await.len(),
        1,
        "precondition: run 1 staged t1's HTML and kept it",
    );

    // Run 2: the selected root is no longer readable.
    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .respond_with(ResponseTemplate::new(403))
        .with_priority(1)
        .mount(&server)
        .await;
    execute_start_quip_import(&ctx, &import_id, "owner1")
        .await
        .expect("a 403 on a selected root is terminal for the import, not an error to retry");

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.status, ImportStatus::Failed, "precondition: the import is terminally Failed");

    let left = staged_keys(&app, &import_id).await;
    assert!(left.is_empty(), "a failed import must not retain the text it staged: {left:?}");
    let key = format!("imports/{import_id}/threads/t1.html");
    assert!(head_is_404(&app, &key).await, "{key} must be a 404");
}

/// Blast-radius fence. The sweep is scoped to
/// `imports/{import_id}/threads/` — the narrowest prefix that covers the
/// staging and nothing else — and this pins each way a wider prefix could
/// reach past it:
///
/// * `imports/` alone would take every other import and every DOCX/PDF upload
///   (`imports/{user_id}/{id}.{ext}` — a different shape under the same root);
/// * `imports/{import_id}` **without the trailing slash** would also match a
///   different import whose id merely starts with this one's;
/// * `imports/{import_id}/` would take anything else this import ever keys
///   under its own id.
#[tokio::test]
async fn the_staging_sweep_reaches_no_other_import_or_user() {
    common::require_infra!();
    let server = quip_content_server().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    // Four decoys, each one prefix-adjacent to the sweep in a different way.
    let other_import = "imports/imp-someone-else/threads/t1.html".to_string();
    let id_prefix_twin = format!("imports/{import_id}-twin/threads/t1.html");
    let docx_staging = "imports/owner2/some-upload.docx".to_string();
    let same_import_other_shape = format!("imports/{import_id}/manifest.json");
    let decoys = [&other_import, &id_prefix_twin, &docx_staging, &same_import_other_shape];
    for key in decoys {
        seed_object(&app, key, b"do not touch").await;
    }

    let ctx = worker_ctx_with_quip(&app, server.uri());
    execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();
    assert!(
        staged_keys(&app, &import_id).await.is_empty(),
        "precondition: this import's own staging was swept",
    );

    for key in decoys {
        let body = app
            .state
            .doc_repo
            .s3()
            .get_object(key)
            .await
            .unwrap_or_else(|e| panic!("{key} must survive the sweep, but: {e}"));
        assert_eq!(body, b"do not touch", "{key} must survive byte-for-byte");
    }
}

/// [`quip_content_server`] with every thread typed `chat`, so the pass reaches
/// the end of the manifest — and its terminal `Succeeded` — without ever
/// touching S3 itself. That isolation is what lets the next test point the
/// worker's S3 client at a bucket that does not exist and be sure the *only*
/// operation that fails is the staging sweep.
async fn quip_server_with_only_chat_threads() -> MockServer {
    let server = quip_content_server().await;
    Mock::given(method("GET"))
        .and(path("/1/threads/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "t1": {"thread": {"id": "t1", "title": "Chat A", "type": "chat", "updated_usec": 111}},
            "t2": {"thread": {"id": "t2", "title": "Chat B", "type": "chat", "updated_usec": 222}},
            "tc": {"thread": {"id": "tc", "title": "Chat C", "type": "chat", "updated_usec": 333}}
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    server
}

/// The sweep is **advisory**: it must never fail, retry, or alter an import.
///
/// The same discipline `record_report` follows — an import that could not
/// write a note about itself must not die of it, and an import that succeeded
/// must not be reported as failed because a cleanup could not reach S3. If the
/// sweep's error propagated, this run would return `Err`, the queue would
/// retry a *finished* import, and after four such runs it would dead-letter and
/// overwrite the user's `Succeeded` with `Failed`.
#[tokio::test]
async fn a_staging_sweep_failure_does_not_change_the_imports_outcome() {
    common::require_infra!();
    let server = quip_server_with_only_chat_threads().await;
    let app = common::TestApp::new_with_quip_base(server.uri()).await;
    let import_id = seed_scoping_import(&app, "owner1").await;

    // The worker's own S3 client points at a bucket that does not exist, so
    // the sweep's `list_objects_v2` fails outright. `doc_repo` keeps the real
    // bucket; nothing else in a chat-only pass uses `ctx.s3`.
    let ctx = WorkerCtx::new(
        app.state.doc_repo.clone(),
        app.state.folder_repo.clone(),
        ogrenotes_storage::s3::S3Client::new(
            app.s3_client().clone(),
            format!("no-such-bucket-{}", nanoid::nanoid!(8).to_lowercase()),
        ),
        app.state.import_repo.clone(),
        app.state.user_repo.clone(),
        app.state.quip_token_store.clone(),
        Some(server.uri()),
    );

    execute_start_quip_import(&ctx, &import_id, "owner1")
        .await
        .expect("a sweep that cannot reach S3 must not fail the import");

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        ImportStatus::Succeeded,
        "the import's outcome must be decided by the import, not by its cleanup",
    );
    assert_eq!(rec.phase, 2, "the pass still completed");
}
