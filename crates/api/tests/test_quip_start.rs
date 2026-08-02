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
const IMAGES_DROPPED: &str = "images_dropped";
const THREADS_TRUNCATED: &str = "threads_deep_nesting_truncated";
const THREADS_MENTIONS_DEGRADED: &str = "threads_mentions_degraded";
const LIVE_APPS_DROPPED: &str = "live_apps_dropped";
/// The counter's key is *not* its note kind's name: the kind is
/// `formulas_dropped`, the counter is `spreadsheet_formulas_dropped`
/// (`worker_mode::report`). Re-typing both here is what keeps a projection
/// that mixed them up from reading as a silent zero.
const FORMULAS_DROPPED: &str = "spreadsheet_formulas_dropped";
const KIND_THREAD_SKIPPED: &str = "thread_skipped";
const KIND_THREAD_FAILED: &str = "thread_failed";
const KIND_IMAGE_DROPPED: &str = "image_dropped";
const KIND_CONTENT_TRUNCATED: &str = "content_truncated";
const KIND_MENTIONS_DEGRADED: &str = "mentions_degraded";
const KIND_LIVE_APP_DROPPED: &str = "live_app_dropped";
const KIND_FORMULAS_DROPPED: &str = "formulas_dropped";

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

/// #174: the status poll carries the folder the documents land in, so the
/// wizard's "Open folder" button can go there instead of dumping the user on
/// Home to hunt for it.
///
/// The value is the *effective* destination — since #172 that is the
/// dedicated per-import folder, not the parent the wizard sent. Asserting
/// against the parent as well is the point: a response that echoed
/// `targetFolderId` as the user picked it would send the user one level too
/// high, which is the original bug with an extra step.
///
/// Doubles as the no-token guard for a *populated* destination field
/// (`report_response_never_carries_a_token_field` covers the same walk with
/// the field null), because a live token is stashed for this import.
#[tokio::test]
async fn get_status_carries_the_destination_folder_of_a_started_import() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("owner1@test.com").await;
    let parent = app.create_folder(&token, "Dest", None).await;
    let import_id = seed_scoping_import(&app, &user_id, &[]).await;

    const SECRET: &str = "QUIPTOKENsentinel-must-never-be-returned-0xfeedface";
    app.state
        .quip_token_store
        .put(&import_id, &ogrenotes_quip_import::QuipToken::new(SECRET.to_string()))
        .await
        .expect("stash token");

    let (status, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/imports/quip/{import_id}/start"),
            Some(&token),
            Some(serde_json::json!({
                "selectedRootFolderIds": ["root"],
                "targetFolderId": parent,
            })),
        )
        .await;
    assert_eq!(status, 202, "start failed: {body}");

    let import_folder = app
        .state
        .import_repo
        .get(&import_id)
        .await
        .unwrap()
        .expect("record exists")
        .import_folder_id
        .expect("start records an import folder");

    let body = get_report_status(&app, &import_id, &token).await;
    assert_eq!(
        body["destinationFolderId"], import_folder,
        "the status must name the folder the documents land in: {body}",
    );
    assert_ne!(
        body["destinationFolderId"], serde_json::Value::String(parent),
        "the destination is the dedicated import folder, not the picked parent: {body}",
    );

    // The new field must not have opened a credential path (same two checks
    // as `report_response_never_carries_a_token_field`, here with the
    // destination actually populated).
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

/// An import that was never started has no destination at all, and the
/// response says so with `null` rather than inventing one. The wizard's
/// button falls back to Home on `null`, so a wrong guess here would be a
/// silent misnavigation instead of a visible absence.
#[tokio::test]
async fn get_status_reports_a_null_destination_before_the_import_is_started() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    let body = get_report_status(&app, &import_id, &owner_token).await;
    assert!(
        body["destinationFolderId"].is_null(),
        "a Scoping import has no destination yet: {body}",
    );

    app.cleanup().await;
}

/// An import started *before* #172 has no `import_folder_id` — but its
/// documents did land somewhere: the parent the user picked, recorded as
/// `target_folder_id`. The response projects that, so the button still opens
/// a real folder for those runs.
///
/// This is the test that pins the choice of field: projecting
/// `import_folder_id` instead would report `null` here.
#[tokio::test]
async fn get_status_falls_back_to_the_picked_parent_for_a_pre_import_folder_run() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    // Exactly the shape a pre-#172 start left behind: a recorded destination
    // (the parent the user picked) and no import folder. `set_scope` is the
    // write that start used then, and still uses now — it never touches
    // `import_folder_id`.
    app.state
        .import_repo
        .set_scope(&import_id, &["root".to_string()], "legacy-destination")
        .await
        .expect("record the legacy destination");
    let record = app
        .state
        .import_repo
        .get(&import_id)
        .await
        .unwrap()
        .expect("record exists");
    assert!(
        record.import_folder_id.is_none(),
        "the fixture must be a pre-#172 import: no import folder",
    );

    let body = get_report_status(&app, &import_id, &owner_token).await;
    assert_eq!(
        body["destinationFolderId"], "legacy-destination",
        "a pre-#172 import must still report the folder its documents went to: {body}",
    );

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

/// #208 — **the deliverable.** Every kind the worker records reaches the
/// wire with its own counter and its own notes.
///
/// Before this, `image_dropped` / `content_truncated` / `mentions_degraded`
/// — and, from #214, `live_app_dropped` / `formulas_dropped` — were written
/// durably and correctly and then projected nowhere: a user saw them only as
/// an increment to `notesDropped`, a bare number with no explanation
/// attached. Writing the note is the cheap floor; this is the half that lets
/// someone stand on it.
///
/// Each kind gets a *distinct* counter value and a distinct note so a
/// projection that crossed two kinds' wires — the likeliest way to get this
/// wrong — fails rather than passing on symmetry.
#[tokio::test]
async fn get_status_surfaces_every_recorded_loss_kind() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    let repo = &app.state.import_repo;
    repo.bump_report_counter(&import_id, &owner_id, IMAGES_DROPPED, 8)
        .await
        .expect("images counter");
    repo.bump_report_counter(&import_id, &owner_id, THREADS_TRUNCATED, 3)
        .await
        .expect("truncation counter");
    repo.bump_report_counter(&import_id, &owner_id, THREADS_MENTIONS_DEGRADED, 5)
        .await
        .expect("mentions counter");
    repo.bump_report_counter(&import_id, &owner_id, LIVE_APPS_DROPPED, 2)
        .await
        .expect("live apps counter");
    repo.bump_report_counter(&import_id, &owner_id, FORMULAS_DROPPED, 300)
        .await
        .expect("formulas counter");

    for (kind, id, detail) in [
        (KIND_IMAGE_DROPPED, "qi1", "image blob-9: it could not be stored"),
        (KIND_CONTENT_TRUNCATED, "qc1", "nesting deeper than 32 levels was flattened"),
        (KIND_MENTIONS_DEGRADED, "qm1", "the Quip person-lookup endpoint rejected this import"),
        (KIND_LIVE_APP_DROPPED, "ql1", "2 embedded Quip live app(s) could not be converted"),
        (KIND_FORMULAS_DROPPED, "qs1", "300 spreadsheet formula(s) were not imported"),
    ] {
        repo.append_report_note(
            &import_id,
            &owner_id,
            ReportNote {
                quip_thread_id: id.to_string(),
                kind: kind.to_string(),
                detail: detail.to_string(),
            },
        )
        .await
        .expect("note");
    }

    let body = get_report_status(&app, &import_id, &owner_token).await;
    let report = &body["report"];

    for (field, total, id, detail_fragment) in [
        ("imagesDropped", 8, "qi1", "could not be stored"),
        ("contentTruncated", 3, "qc1", "flattened"),
        ("mentionsDegraded", 5, "qm1", "person-lookup endpoint"),
        ("liveAppsDropped", 2, "ql1", "live app(s)"),
        // 300 formulas, one note: the counter counts formulas, the note
        // counts documents. A projection that read the total off the note
        // list would report 1.
        ("spreadsheetFormulasDropped", 300, "qs1", "formula(s)"),
    ] {
        let section = &report[field];
        assert!(!section.is_null(), "{field} must be projected: {body}");
        assert_eq!(
            section["total"], total,
            "{field}.total must come from its own uncapped counter: {report}",
        );
        let notes = section["notes"].as_array().expect("notes array");
        assert_eq!(notes.len(), 1, "{field} must carry exactly its own notes");
        assert_eq!(notes[0]["quipThreadId"], id, "{field} projected another kind's note");
        assert!(
            notes[0]["detail"].as_str().unwrap().contains(detail_fragment),
            "{field} detail must be the one the worker authored: {section}",
        );
    }

    app.cleanup().await;
}

/// A kind that never occurred is `null`, not a zero row.
///
/// "Zero images were dropped" is not news, and a section drawn for it pushes
/// the sections that *are* news off the screen. `null` is also what lets the
/// client draw nothing without re-deriving emptiness from two fields.
#[tokio::test]
async fn a_loss_kind_that_never_occurred_is_null_not_a_zero_row() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    // A clean 47-document run: a REPORT row exists, and nothing was lost.
    app.state
        .import_repo
        .bump_report_counter(&import_id, &owner_id, THREADS_IMPORTED, 47)
        .await
        .expect("imported counter");

    let body = get_report_status(&app, &import_id, &owner_token).await;
    let report = &body["report"];
    assert_eq!(report["imported"], 47, "precondition: the row exists: {body}");
    for field in [
        "imagesDropped",
        "contentTruncated",
        "mentionsDegraded",
        "liveAppsDropped",
        "spreadsheetFormulasDropped",
    ] {
        assert!(
            report[field].is_null(),
            "{field} must be null on a run that never hit it, not a zero section: {report}",
        );
    }

    app.cleanup().await;
}

/// #208's truncation case, on a within-document kind: the counter is the
/// total, the note list is a sample, and the response must let the reader
/// see the difference.
///
/// This is the same property `get_status_surfaces_true_counter_totals_...`
/// pins for skips, re-pinned here because the new sections are a *new* place
/// to get it wrong — a projection that reported `total = notes.len()` would
/// tell a user who lost 4 000 images that 25 images were lost, which is the
/// original silence with a smaller number on it.
///
/// `images_dropped` counts images rather than documents, so its counter runs
/// past the 25-note budget far sooner than the thread-scoped kinds do; this
/// is the kind most likely to be truncated in a real run.
#[tokio::test]
async fn a_truncated_within_document_kind_still_reports_its_true_total() {
    common::require_infra!();

    let app = common::TestApp::new().await;
    let (owner_id, owner_token) = app.create_user("owner1@test.com").await;
    let import_id = seed_scoping_import(&app, &owner_id, &[]).await;

    let repo = &app.state.import_repo;
    const TRUE_TOTAL: u64 = 4_000;
    repo.bump_report_counter(&import_id, &owner_id, IMAGES_DROPPED, TRUE_TOTAL)
        .await
        .expect("images counter");
    const OVERFLOW: usize = 7;
    for i in 0..REPORT_MAX_NOTES_PER_KIND + OVERFLOW {
        repo.append_report_note(
            &import_id,
            &owner_id,
            ReportNote {
                quip_thread_id: format!("qi{i:04}"),
                kind: KIND_IMAGE_DROPPED.to_string(),
                detail: "image blob-9: Quip denied access (HTTP 403)".to_string(),
            },
        )
        .await
        .expect("image note");
    }

    let body = get_report_status(&app, &import_id, &owner_token).await;
    let images = &body["report"]["imagesDropped"];
    assert_eq!(
        images["total"], TRUE_TOTAL,
        "the total must be the uncapped counter, not the note list's length: {images}",
    );
    let notes = images["notes"].as_array().expect("imagesDropped.notes");
    assert_eq!(
        notes.len(),
        REPORT_MAX_NOTES_PER_KIND,
        "the note list stops at the storage row's per-kind budget",
    );
    // The remainder the client renders as "…and N more". Derived here the
    // same way the client derives it, so a response that made the subtraction
    // read as zero — i.e. "the list is complete" — fails here first.
    assert_eq!(
        images["total"].as_u64().unwrap() - notes.len() as u64,
        TRUE_TOTAL - REPORT_MAX_NOTES_PER_KIND as u64,
        "the unnamed remainder must be recoverable from the response: {images}",
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
///
/// **Every projected kind is populated here (#208), not just the two the
/// guard was written for.** The recursive key walk only inspects what the
/// response actually contains, so a response whose new sections were all
/// `null` would walk past the very fields the new code added — the guard
/// would stay green while covering nothing. Each kind gets a counter and a
/// note so every section is materialized before the walk runs.
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

    let repo = &app.state.import_repo;
    for (counter, kind) in [
        (THREADS_SKIPPED_FORBIDDEN, KIND_THREAD_SKIPPED),
        (THREADS_FAILED, KIND_THREAD_FAILED),
        (IMAGES_DROPPED, KIND_IMAGE_DROPPED),
        (THREADS_TRUNCATED, KIND_CONTENT_TRUNCATED),
        (THREADS_MENTIONS_DEGRADED, KIND_MENTIONS_DEGRADED),
        (LIVE_APPS_DROPPED, KIND_LIVE_APP_DROPPED),
        (FORMULAS_DROPPED, KIND_FORMULAS_DROPPED),
    ] {
        repo.bump_report_counter(&import_id, &owner_id, counter, 1)
            .await
            .expect("counter");
        repo.append_report_note(
            &import_id,
            &owner_id,
            ReportNote {
                quip_thread_id: "qt1".to_string(),
                kind: kind.to_string(),
                detail: "Quip denied access to this content (HTTP 403)".to_string(),
            },
        )
        .await
        .expect("note");
    }

    let body = get_report_status(&app, &import_id, &owner_token).await;
    let report = &body["report"];
    // Precondition for the walk below: every section the guard is meant to
    // cover is actually present in the body being walked.
    for section in [
        "skipped",
        "failed",
        "imagesDropped",
        "contentTruncated",
        "mentionsDegraded",
        "liveAppsDropped",
        "spreadsheetFormulasDropped",
    ] {
        assert!(
            report[section]["notes"].as_array().is_some_and(|n| !n.is_empty()),
            "the guard must walk a populated {section}: {body}",
        );
    }
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
