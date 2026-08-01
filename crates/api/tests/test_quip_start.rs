// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for `POST /api/v1/imports/quip/{id}/start` and
//! `GET /api/v1/imports/quip/{id}` (Quip import Phase 1 Task 4).
//!
//! Drives the real router with a real `JobQueue` producer against shared
//! Redis (`TestApp::new()` wires it — see `common/mod.rs`), so these are
//! gated on `require_infra!`. Enqueue success is asserted indirectly via
//! the `202` response: the handler enqueues before returning it and maps
//! enqueue failures to `503`, so a `202` proves the job made it onto the
//! queue.

mod common;

use hyper::Method;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::import::{ImportRecord, ImportStatus};
use ogrenotes_storage::models::import_inventory::{REPORT_MAX_NOTES_PER_KIND, ReportNote};

/// The `REPORT` row's counter keys and note kinds, as the worker writes
/// them (`worker_mode::report`, which is `pub(crate)` and so not nameable
/// from an integration test).
///
/// Re-typing them here is deliberate: `GET /imports/quip/{id}` projects
/// these keys into a response the wizard renders, so a rename is a
/// wire-visible change and ought to break a test rather than silently
/// start reading as "zero — nothing was lost".
const THREADS_IMPORTED: &str = "threads_imported";
const THREADS_SKIPPED_FORBIDDEN: &str = "threads_skipped_forbidden";
const THREADS_SKIPPED_CHAT: &str = "threads_skipped_chat";
const THREADS_FAILED: &str = "threads_failed";
const KIND_THREAD_SKIPPED: &str = "thread_skipped";
const KIND_THREAD_FAILED: &str = "thread_failed";

/// Seed a `Scoping` import record with the given owner + selected roots,
/// returning its id. Mirrors the helper in
/// `test_quip_inventory_worker.rs:101` (not shared across test binaries).
async fn seed_scoping_import(app: &common::TestApp, owner: &str, roots: &[&str]) -> String {
    let import_id = format!("imp-{}", nanoid::nanoid!(8));
    let now = now_usec();
    let record = ImportRecord {
        import_id: import_id.clone(),
        owner_id: owner.to_string(),
        status: ImportStatus::Scoping,
        phase: 0,
        quip_user_id: None,
        target_folder_id: None,
        import_folder_id: None,
        selected_roots: roots.iter().map(|s| s.to_string()).collect(),
        created_at: now,
        updated_at: now,
    };
    app.state.import_repo.create(&record).await.expect("seed import record");
    import_id
}

#[tokio::test]
async fn start_authorizes_target_folder_persists_scope_and_enqueues() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("owner1@test.com").await;
    let folder = app.create_folder(&token, "Dest", None).await;
    let import_id = seed_scoping_import(&app, &user_id, &[]).await;

    let (status, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/imports/quip/{import_id}/start"),
            Some(&token),
            Some(serde_json::json!({
                "selectedRootFolderIds": ["root"],
                "targetFolderId": folder,
            })),
        )
        .await;
    assert_eq!(status, 202, "start failed: {body}");
    assert_eq!(body["importId"], import_id);
    assert_eq!(body["status"], "running");

    let rec = app
        .state
        .import_repo
        .get(&import_id)
        .await
        .unwrap()
        .expect("import record exists");
    assert_eq!(rec.selected_roots, vec!["root".to_string()]);

    // #170 containment: start now creates a dedicated per-import folder UNDER
    // the picked target and records THAT as the effective destination, so
    // imported documents land in one deletable folder instead of flat in the
    // picked (Home) folder. `target_folder_id` is therefore the import folder,
    // not the raw `folder` the wizard sent.
    let import_folder = rec
        .import_folder_id
        .as_deref()
        .expect("first start must create and record an import folder");
    assert_ne!(
        import_folder, folder,
        "the import folder must be a NEW folder, not the picked parent",
    );
    assert_eq!(
        rec.target_folder_id.as_deref(),
        Some(import_folder),
        "the effective destination on META must be the import folder",
    );

    // The import folder is a real, listable child of the picked parent, owned
    // by the caller — so the user can see it in the sidebar and delete it to
    // undo the whole import.
    let child_folder = app
        .state
        .folder_repo
        .get(import_folder)
        .await
        .unwrap()
        .expect("the import folder exists as a Folder row");
    assert_eq!(child_folder.parent_id.as_deref(), Some(folder.as_str()));
    assert_eq!(child_folder.owner_id, user_id);
    assert!(
        child_folder.title.starts_with("Quip Import"),
        "the folder name identifies the import: {}",
        child_folder.title,
    );
    let parent_children = app
        .state
        .folder_repo
        .list_children(&folder)
        .await
        .unwrap();
    assert!(
        parent_children.iter().any(|c| c.child_id == import_folder),
        "the import folder must be linked as a child of the picked parent",
    );
    // The folder row must never carry the Quip token/secret — the security
    // spine forbids it reaching any durable row or the folder name.
    assert!(
        !child_folder.title.to_lowercase().contains("token")
            && !child_folder.title.to_lowercase().contains("secret"),
        "the import folder name must not embed a credential: {}",
        child_folder.title,
    );

    app.cleanup().await;
}

/// Idempotency guarantee (the crux of #170 containment): a re-start must NOT
/// create a second import folder. The importer is deliberately re-startable
/// (a crashed-and-reaped run replays), so clicking start twice — or the queue
/// redelivering the job — must reuse the ONE folder recorded on the first
/// start, never accumulate a fresh folder per attempt.
#[tokio::test]
async fn restart_reuses_the_same_import_folder() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("owner1@test.com").await;
    let folder = app.create_folder(&token, "Dest", None).await;
    let import_id = seed_scoping_import(&app, &user_id, &[]).await;

    let path = format!("/api/v1/imports/quip/{import_id}/start");
    let body = serde_json::json!({
        "selectedRootFolderIds": ["root"],
        "targetFolderId": folder,
    });

    let (s1, b1) = app
        .json_request(Method::POST, &path, Some(&token), Some(body.clone()))
        .await;
    assert_eq!(s1, 202, "first start failed: {b1}");
    let after_first = app
        .state
        .import_repo
        .get(&import_id)
        .await
        .unwrap()
        .expect("record exists")
        .import_folder_id
        .expect("first start records an import folder");

    let (s2, b2) = app
        .json_request(Method::POST, &path, Some(&token), Some(body))
        .await;
    assert_eq!(s2, 202, "re-start failed: {b2}");
    let after_second = app
        .state
        .import_repo
        .get(&import_id)
        .await
        .unwrap()
        .expect("record exists")
        .import_folder_id
        .expect("re-start keeps the import folder recorded");

    assert_eq!(
        after_first, after_second,
        "a re-start must reuse the same import folder, not create a second",
    );

    // The picked parent started empty, so after two starts it must hold
    // EXACTLY ONE child — the single import folder. Counting every child (not
    // just the one whose id we already know) is what catches a broken guard
    // that creates a second, differently-ided folder on the re-start.
    let children = app
        .state
        .folder_repo
        .list_children(&folder)
        .await
        .unwrap();
    assert_eq!(
        children.len(),
        1,
        "a re-start must not create a second folder under the target; children: {children:?}",
    );
    assert_eq!(
        children[0].child_id, after_first,
        "the one child must be the folder recorded on the first start",
    );

    app.cleanup().await;
}

#[tokio::test]
async fn start_rejects_unauthorized_target_folder() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let (_other_id, other_token) = app.create_user("other@test.com").await;
    let their_folder = app.create_folder(&other_token, "Theirs", None).await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    let (status, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/imports/quip/{import_id}/start"),
            Some(&owner_token),
            Some(serde_json::json!({
                "selectedRootFolderIds": ["root"],
                "targetFolderId": their_folder,
            })),
        )
        .await;
    assert_eq!(
        status, 404,
        "check_folder_access hides unauthorized folders as 404: {body}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn get_status_returns_progress_and_is_owner_gated() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let (_other_id, other_token) = app.create_user("other@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    // owner sees it:
    let (status, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/imports/quip/{import_id}"),
            Some(&owner_token),
            None,
        )
        .await;
    assert_eq!(status, 200, "get_status failed: {body}");
    assert_eq!(body["status"], "scoping");
    assert_eq!(body["phase"], 0);
    assert_eq!(body["progress"]["total"], 0);
    assert_eq!(body["progress"]["done"], 0);
    assert_eq!(body["progress"]["stage"], "scoping");

    // a different user gets 404 (no existence disclosure):
    let (status, _body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/imports/quip/{import_id}"),
            Some(&other_token),
            None,
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

// ─── Task 5: the outcome report on the status poll ─────────────

/// GET the status endpoint as `token`'s owner, asserting 200.
async fn get_report_status(
    app: &common::TestApp,
    import_id: &str,
    token: &str,
) -> serde_json::Value {
    let (status, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/imports/quip/{import_id}"),
            Some(token),
            None,
        )
        .await;
    assert_eq!(status, 200, "get_status failed: {body}");
    body
}

/// No `REPORT` row yet ⇒ `report: null`, **not** an all-zero report.
///
/// The distinction is load-bearing. Report writes are advisory by
/// construction (`worker_mode::record_report` returns nothing so a broken
/// report can never halt an import), so a perfectly successful run can
/// finish with no row at all. A wizard that read a missing row as
/// `imported: 0` would announce "Imported 0 documents" after a clean
/// 47-document run — a worse lie than the one this feature removes.
#[tokio::test]
async fn get_status_reports_null_until_the_worker_writes_a_report_row() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    let body = get_report_status(&app, &import_id, &owner_token).await;
    assert!(
        body["report"].is_null(),
        "an import with no REPORT row must report null, not zeros: {body}",
    );

    app.cleanup().await;
}

/// The endpoint surfaces the counters and the bounded note list, and the
/// two disagree on purpose: 10 000 inaccessible threads produce 10 000 in
/// the counter and 25 notes, because the storage row budgets notes per
/// kind. **The counter is what the "…and N more" sentence is computed
/// from.** A response that reported `total = notes.len()` would let the
/// wizard present a 25-item list as the complete set of losses — the
/// original bug, moved one layer up.
#[tokio::test]
async fn get_status_surfaces_true_counter_totals_alongside_the_bounded_notes() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    let repo = &app.state.import_repo;
    // Counters carry the true totals and are bumped in bulk (the worker
    // bumps by 1 per event; the `by` parameter is the same code path).
    repo.bump_report_counter(&import_id, &owner_id, THREADS_IMPORTED, 47)
        .await
        .expect("imported counter");
    repo.bump_report_counter(&import_id, &owner_id, THREADS_SKIPPED_FORBIDDEN, 10_000)
        .await
        .expect("skipped counter");
    repo.bump_report_counter(&import_id, &owner_id, THREADS_SKIPPED_CHAT, 12)
        .await
        .expect("chat counter");
    repo.bump_report_counter(&import_id, &owner_id, THREADS_FAILED, 2)
        .await
        .expect("failed counter");

    // More skip notes than the per-kind budget, so the row truncates and
    // `notes_dropped` becomes non-zero — the case the wizard must never
    // present as a complete list.
    const OVERFLOW: usize = 5;
    for i in 0..REPORT_MAX_NOTES_PER_KIND + OVERFLOW {
        repo.append_report_note(
            &import_id,
            &owner_id,
            ReportNote {
                quip_thread_id: format!("qt{i:04}"),
                kind: KIND_THREAD_SKIPPED.to_string(),
                detail: "Quip denied access to this content (HTTP 403)".to_string(),
            },
        )
        .await
        .expect("skip note");
    }
    for i in 0..2 {
        repo.append_report_note(
            &import_id,
            &owner_id,
            ReportNote {
                quip_thread_id: format!("qf{i}"),
                kind: KIND_THREAD_FAILED.to_string(),
                detail: "Quip returned HTTP 500; gave up after 3 attempts".to_string(),
            },
        )
        .await
        .expect("fail note");
    }

    let body = get_report_status(&app, &import_id, &owner_token).await;
    let report = &body["report"];
    assert!(!report.is_null(), "report must be present: {body}");

    assert_eq!(report["imported"], 47);
    assert_eq!(report["chatThreadsSkipped"], 12);
    assert_eq!(
        report["notesDropped"], OVERFLOW,
        "the row's own truncation marker must survive to the wire",
    );

    // Skips: the counter, not the list length.
    assert_eq!(
        report["skipped"]["total"], 10_000,
        "total must come from the uncapped counter: {report}",
    );
    let skip_notes = report["skipped"]["notes"]
        .as_array()
        .expect("skipped.notes array");
    assert_eq!(
        skip_notes.len(),
        REPORT_MAX_NOTES_PER_KIND,
        "the note list stops at the storage row's per-kind budget",
    );
    assert!(
        report["skipped"]["total"].as_u64().unwrap() > skip_notes.len() as u64,
        "the response must let a reader see that the list is a prefix",
    );
    assert_eq!(skip_notes[0]["quipThreadId"], "qt0000");
    assert_eq!(
        skip_notes[0]["detail"],
        "Quip denied access to this content (HTTP 403)"
    );

    // Failures are their own bucket, not folded in with skips.
    assert_eq!(report["failed"]["total"], 2);
    let fail_notes = report["failed"]["notes"].as_array().expect("failed.notes");
    assert_eq!(fail_notes.len(), 2);
    assert_eq!(fail_notes[0]["quipThreadId"], "qf0");
    assert!(
        fail_notes
            .iter()
            .all(|n| n["detail"].as_str().unwrap().contains("gave up")),
        "a failure's detail must read as 'we tried and lost', not as a skip",
    );
    assert!(
        skip_notes
            .iter()
            .all(|n| !n["detail"].as_str().unwrap().contains("gave up")),
        "a skip must not inherit failure wording",
    );

    app.cleanup().await;
}

/// The no-token guard for the *response* shape, mirroring
/// `import_record_never_carries_a_token_field` (which guards the durable
/// row) in `crates/storage/tests/test_import_repo.rs`.
///
/// The Quip token reaching the frontend is the one failure this feature
/// cannot survive, and the report is the newest surface that could leak it:
/// its `detail` strings are the only worker-authored free text the wizard
/// renders. Two independent checks — no field *named* token/secret anywhere
/// in the response (a value check alone would miss an empty or re-encoded
/// field), and the live token's own text absent from the serialized body.
///
/// Field *names* rather than the whole body, because `status` legitimately
/// serializes as `"tokenrejected"`.
#[tokio::test]
async fn report_response_never_carries_a_token_field() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    // A live token stashed for this exact import — the thing that must not
    // come back out.
    const SECRET: &str = "QUIPTOKENsentinel-must-never-be-returned-0xdeadbeef";
    app.state
        .quip_token_store
        .put(&import_id, &ogrenotes_quip_import::QuipToken::new(SECRET.to_string()))
        .await
        .expect("stash token");

    app.state
        .import_repo
        .bump_report_counter(&import_id, &owner_id, THREADS_SKIPPED_FORBIDDEN, 1)
        .await
        .expect("counter");
    app.state
        .import_repo
        .append_report_note(
            &import_id,
            &owner_id,
            ReportNote {
                quip_thread_id: "qt1".to_string(),
                kind: KIND_THREAD_SKIPPED.to_string(),
                detail: "Quip denied access to this content (HTTP 403)".to_string(),
            },
        )
        .await
        .expect("note");

    let body = get_report_status(&app, &import_id, &owner_token).await;
    let serialized = body.to_string();

    assert!(
        !serialized.contains(SECRET),
        "the stashed Quip token must never appear in the status response",
    );
    let mut offending = Vec::new();
    collect_secret_keys(&body, &mut offending);
    assert!(
        offending.is_empty(),
        "status response exposes credential-shaped fields {offending:?}: {serialized}",
    );

    app.cleanup().await;
}

/// Walk a JSON value collecting every object key whose *name* looks like a
/// credential. Recursive because the report nests two levels deep and a
/// leak added under `report.skipped.notes[]` would be invisible to a
/// top-level key scan.
fn collect_secret_keys(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, child) in map {
                let lower = k.to_ascii_lowercase();
                if lower.contains("token") || lower.contains("secret") || lower.contains("password")
                {
                    out.push(k.clone());
                }
                collect_secret_keys(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect_secret_keys(child, out);
            }
        }
        _ => {}
    }
}

/// A corrupt `REPORT` row must degrade the poll to `report: null`, never 500
/// it. Report writes are advisory by construction on the WRITE side
/// (`worker_mode::record_report` returns nothing, so a poisoned counter can
/// never halt an import); this pins the same principle on the READ side, which
/// used to propagate a decode error as `ApiError::Internal` and 500 every
/// wizard poll — taking down the read path over the exact bookkeeping the
/// write path is hardened against.
///
/// A permanently poisoned counter is a real reachable state, documented on
/// `ImportRepo::bump_report_counter`: a `counters` map value that is not a
/// number fails `report_from_item` on every subsequent read. This seeds
/// exactly that and asserts the poll still returns 200 with the rest of the
/// status intact.
#[tokio::test]
async fn a_corrupt_report_row_degrades_to_null_rather_than_500ing_the_poll() {
    common::require_infra!();
    use aws_sdk_dynamodb::types::AttributeValue;

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    // Poison the REPORT row: `counters.threads_imported` is a string where the
    // decoder requires a number, so every `get_report` for this import now
    // errors. Written straight to DynamoDB — the repo's own writers would
    // never produce this, which is the point.
    app.dynamo_client()
        .put_item()
        .table_name(&app.table_name)
        .item("PK", AttributeValue::S(format!("IMPORT#{import_id}")))
        .item("SK", AttributeValue::S("REPORT".to_string()))
        .item("owner_id", AttributeValue::S(owner_id.clone()))
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
        "precondition: the report read genuinely fails for this import",
    );

    // The poll must still succeed. `get_report_status` asserts 200 internally.
    let body = get_report_status(&app, &import_id, &owner_token).await;

    assert!(
        body["report"].is_null(),
        "a corrupt report must degrade to null, not 500 the poll: {body}",
    );
    // The rest of the status is unaffected — the report is advisory, the
    // status is not.
    assert_eq!(body["status"], "scoping", "status must still return: {body}");
    assert_eq!(body["phase"], 0, "phase must still return: {body}");
    assert!(body["progress"].is_object(), "progress must still return: {body}");

    app.cleanup().await;
}
