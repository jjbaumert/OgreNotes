// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Quip import — Phase 0 Task 7: `POST /api/v1/imports/quip/connect`.
//!
//! Validates a pasted Quip personal access token against the real Quip
//! Automation API, creates the durable `ImportRecord` manifest (status
//! `Scoping`, no token — see `ogrenotes_storage::models::import`), stashes
//! the token in the `TokenStore` keyed by the new import id, and returns
//! the caller's Quip profile plus their root folders so Task 8's wizard
//! can render a folder picker. If any step after the token is stashed
//! fails (currently: the folders fetch), the handler best-effort deletes
//! the stashed token and marks the manifest `Failed` before returning
//! the error — a live Quip token is never left stranded in the store.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_quip_import::{QuipError, QuipToken};
use ogrenotes_storage::models::AccessLevel;
use ogrenotes_storage::models::import::{ImportRecord, ImportStatus};
use ogrenotes_storage::models::import_inventory::ReportRow;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;
use crate::worker_mode::report;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/quip/connect", post(connect))
        .route("/quip/{id}/start", post(start))
        .route("/quip/{id}", get(get_status))
}

// ─── DTOs ──────────────────────────────────────────────────────

// No `Debug` derive: the plaintext token must never be printable, even by a
// future `tracing::debug!(?req)`. (The value is moved into `QuipToken` — which
// IS redacted — immediately in the handler.)
#[derive(Deserialize)]
struct ConnectRequest {
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectResponse {
    import_id: String,
    quip_profile: QuipProfileDto,
    root_folders: Vec<RootFolderDto>,
}

#[derive(Debug, Serialize)]
struct QuipProfileDto {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct RootFolderDto {
    id: String,
    title: String,
}

// ─── Handler ───────────────────────────────────────────────────

/// `POST /api/v1/imports/quip/connect` — body `{ "token": "<pat>" }`.
///
/// Flow: rate-limit -> validate the token against Quip's
/// `/1/users/current` -> create the `ImportRecord` (status `Scoping`,
/// never carries the token) -> stash the token in the `TokenStore` keyed
/// by the new import id -> fetch the caller's root folders (private +
/// shared) -> `201` with the profile and roots.
///
/// Error mapping (never echoes the token or raw Quip internals):
/// - `QuipError::Unauthorized` -> `400 Bad Request` ("invalid Quip
///   token") — deliberately NOT 401, which is reserved for OUR auth on
///   this endpoint (a bad Quip token is a bad *request*, not a failure
///   to authenticate the caller against OgreNotes).
/// - `QuipError::Forbidden` -> `400 Bad Request` ("invalid Quip token"),
///   same as `Unauthorized`. `connect` makes two Quip calls —
///   `/1/users/current` and then `folders()` over the caller's own private
///   + shared root folder ids — and both route through this mapping. A 403
///   from either still means the token itself lacks the access it needs:
///   `folders()` here is scoped to the caller's *own* roots, not to any
///   particular document, so this isn't "one document is restricted" the
///   way a per-thread 403 is later — there's no specific document in play
///   yet. It reads to the caller the same as a bad token. The per-thread/
///   blob meaning of `Forbidden` distinct from `Unauthorized` only shows
///   up later, in the content
///   pass (`worker_mode.rs`, issue #141 Task 4).
/// - `QuipError::RateLimited | Http | Api | Parse` -> `503 Service
///   Unavailable` ("Quip API error").
async fn connect(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<ConnectRequest>,
) -> Result<(StatusCode, Json<ConnectResponse>), ApiError> {
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "quip_connect",
        &user_id,
        state.config.rate_limit_quip_connect_per_min,
        60,
    )
    .await?;

    let token = QuipToken::new(req.token);

    let quip_user = state
        .quip_client
        .current_user(&token)
        .await
        .map_err(map_quip_error)?;

    let import_id = new_id();
    let now = now_usec();
    let record = ImportRecord {
        import_id: import_id.clone(),
        owner_id: user_id,
        status: ImportStatus::Scoping,
        phase: 0,
        quip_user_id: Some(quip_user.id.clone()),
        target_folder_id: None,
        selected_roots: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    state.import_repo.create(&record).await?;

    // Stash the token only after the manifest row exists, so a token-store
    // failure never leaves a "connected" import with no recoverable token —
    // the caller sees a 500 and can retry `connect`, which overwrites
    // (`TokenStore::put` is an upsert) rather than accumulating orphans.
    state
        .quip_token_store
        .put(&import_id, &token)
        .await
        .map_err(|e| {
            tracing::error!(import_id = %import_id, error = %e, "quip token store put failed");
            ApiError::Internal("failed to store Quip credential".to_string())
        })?;

    let mut root_ids = Vec::with_capacity(1 + quip_user.shared_folder_ids.len());
    root_ids.push(quip_user.private_folder_id.clone());
    root_ids.extend(quip_user.shared_folder_ids.iter().cloned());

    let folders = match state.quip_client.folders(&token, &root_ids).await {
        Ok(folders) => folders,
        Err(e) => {
            // A live Quip token must never be left stranded: best-effort
            // delete it from the store and mark the manifest row Failed
            // (rather than a dangling Scoping row) before surfacing the
            // error. Both are best-effort cleanup — their own failure
            // must not mask the original Quip error, and neither may log
            // or return the token.
            rollback_stashed_token(&state, &import_id).await;
            return Err(map_quip_error(e));
        }
    };

    let root_folders = folders
        .into_iter()
        .map(|f| RootFolderDto {
            id: f.id,
            title: f.title,
        })
        .collect();

    Ok((
        StatusCode::CREATED,
        Json(ConnectResponse {
            import_id,
            quip_profile: QuipProfileDto {
                id: quip_user.id,
                name: quip_user.name,
            },
            root_folders,
        }),
    ))
}

/// Best-effort cleanup for the "token stashed but a later step in
/// `connect` failed" path: delete the stranded token and mark the
/// manifest row `Failed` so it isn't left as a dangling `Scoping` row
/// with no recoverable token. Never logs or returns the token itself;
/// only `import_id`. Failures here are logged and swallowed — the
/// caller already has a real error to report, and this is best-effort.
async fn rollback_stashed_token(state: &AppState, import_id: &str) {
    if let Err(e) = state.quip_token_store.delete(import_id).await {
        tracing::error!(import_id = %import_id, error = %e, "quip token rollback delete failed");
    }
    if let Err(e) = state.import_repo.set_status(import_id, ImportStatus::Failed).await {
        tracing::error!(import_id = %import_id, error = %e, "quip import rollback set_status failed");
    }
}

/// Map a `QuipError` to the ApiError this route exposes. Never surfaces
/// the token (neither error variant carries it — see
/// `QuipClient::observe_and_check`) or Quip's raw status/body text; both
/// user-visible messages are static strings independent of the
/// underlying cause.
fn map_quip_error(err: QuipError) -> ApiError {
    match err {
        // `Forbidden` (403) is folded in with `Unauthorized` (401) here
        // deliberately: both Quip calls `connect` makes (`current_user`,
        // then `folders()` over the caller's own root folder ids) are
        // scoped to the caller, not to any particular document, so a 403
        // from either means the token lacks the access it needs — not
        // "one thread is restricted" — see this function's doc comment.
        QuipError::Unauthorized | QuipError::Forbidden => {
            ApiError::BadRequest("invalid Quip token".to_string())
        }
        // Routine throttling — no server log; the caller already gets a 503.
        QuipError::RateLimited { .. } => ApiError::ServiceUnavailable("Quip API error".to_string()),
        // Unexpected Quip-side failure (transport, unexpected status/body,
        // parse). Log it so an operator can tell an outage or a shape change
        // apart from routine throttling — without this, every non-auth Quip
        // failure is an indistinguishable "503, no reason" in the logs.
        // Safe to emit: none of these variants carry the token (pinned by
        // client.rs's `unauthorized_and_rate_limited_map` test).
        QuipError::Http(_) | QuipError::Api { .. } | QuipError::Parse(_) => {
            tracing::warn!(error = %err, "quip API call failed unexpectedly");
            ApiError::ServiceUnavailable("Quip API error".to_string())
        }
    }
}

// ─── Task 4: start + status ───────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartRequest {
    selected_root_folder_ids: Vec<String>,
    target_folder_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartResponse {
    import_id: String,
    status: String,
}

/// `POST /api/v1/imports/quip/{id}/start` — body
/// `{ "selectedRootFolderIds": [...], "targetFolderId": "..." }`.
///
/// Owner-gates the import row, authorizes the destination folder
/// (`check_folder_access` hides an unauthorized/missing folder as 404,
/// same as a missing/foreign import), persists the chosen scope, then
/// enqueues the token-free `Job::StartQuipImport` trigger the worker
/// (Task 3) claims and runs. Deliberately does NOT write `status: Running`
/// here — the worker sets that authoritatively when it claims the job;
/// the `"running"` in the response body is only the optimistic label the
/// wizard shows while the worker spins up.
async fn start(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(import_id): Path<String>,
    Json(req): Json<StartRequest>,
) -> Result<(StatusCode, Json<StartResponse>), ApiError> {
    state
        .import_repo
        .get(&import_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .filter(|r| r.owner_id == user_id)
        .ok_or_else(|| ApiError::NotFound("import not found".to_string()))?;

    super::folders::check_folder_access(&state, &req.target_folder_id, &user_id, AccessLevel::Edit)
        .await?;

    state
        .import_repo
        .set_scope(&import_id, &req.selected_root_folder_ids, &req.target_folder_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let producer = state
        .job_producer
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("job queue unavailable".to_string()))?;
    producer
        .enqueue(ogrenotes_worker::Job::StartQuipImport {
            import_id: import_id.clone(),
            owner_id: user_id,
        })
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("enqueue failed: {e}")))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(StartResponse {
            import_id,
            status: "running".to_string(),
        }),
    ))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    status: String,
    phase: u8,
    progress: Progress,
    /// The import's outcome report, or `null` while no `REPORT` row exists
    /// yet (the worker creates it lazily on the first counted outcome).
    ///
    /// `null` is **not** the same as "an all-zero report", and the
    /// distinction is load-bearing for the wizard: report writes are
    /// deliberately advisory (see `worker_mode::record_report` — a report
    /// must never be able to halt an import), so an import can succeed with
    /// no row at all. A client that read a missing row as
    /// `imported: 0` would announce "Imported 0 documents" after a clean
    /// 47-document run. `null` means "no breakdown available"; the wizard
    /// falls back to its pre-report wording there.
    report: Option<ReportDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: usize,
    total: usize,
    stage: String,
}

/// The user-facing projection of `ReportRow`.
///
/// Deliberately **not** the raw `counters` map plus the raw `notes` list.
/// The counter keys and note kinds are worker-internal names
/// (`worker_mode::report`); re-typing them in the frontend would be a
/// cross-target mirror with no test to hold it, and the failure mode of a
/// drifted key is silent — an unmatched key reads as zero, i.e. "nothing
/// was lost", which is exactly the bug this endpoint exists to fix. The
/// projection happens once, here, next to the constants.
///
/// Skips and failures are separate fields, not one "problems" bucket,
/// because they are different situations with different remedies: a skip
/// is "Quip refused to hand this over" (the user may be able to get
/// access); a failure is "we tried and lost" (the user cannot).
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReportDto {
    /// Documents actually written into the destination folder.
    imported: u64,
    /// Documents Quip refused to serve (HTTP 403).
    skipped: OutcomeDto,
    /// Documents the content pass tried and gave up on.
    failed: OutcomeDto,
    /// Quip chat threads, which are counted but never named: chats are an
    /// expected bulk category and are not documents, so they get their own
    /// number rather than inflating `skipped` (see
    /// `worker_mode`'s chat branch, which deliberately writes no note).
    chat_threads_skipped: u64,
    /// How many notes the `REPORT` row discarded, across **all** kinds —
    /// including kinds this DTO does not project (dropped images, truncated
    /// nesting). Reported for completeness; it is *not* the per-section
    /// truncation signal, because it cannot say which section lost notes.
    /// `OutcomeDto::total` is that signal — see its doc.
    notes_dropped: u64,
}

/// One outcome class: the true total plus a bounded sample of named threads.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct OutcomeDto {
    /// **The true total, from the uncapped counter — never `notes.len()`.**
    /// `notes` stops at the row's per-kind budget (25); this does not. A
    /// reader renders "and `total - notes.len()` more" whenever the two
    /// differ, which is the whole point: a list that silently stopped at 25
    /// would recreate, in the UI, the very silence this feature removes.
    total: u64,
    /// Named examples, bounded by the storage row's per-kind note budget.
    notes: Vec<NoteDto>,
}

/// One named thread within an [`OutcomeDto`].
///
/// `detail` is worker-authored and already sanitized (`safe_quip_reason` —
/// no raw Quip response body, no URL). It is plain text: a client renders
/// it as text, never as markup.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NoteDto {
    /// Empty when the loss was not thread-scoped (the inventory's
    /// forbidden-folder note uses an empty id — no single thread is to
    /// blame). A client shows `detail` alone in that case.
    quip_thread_id: String,
    detail: String,
}

/// Project a stored `ReportRow` into the wire shape.
///
/// Two invariants worth naming:
///
/// 1. `total` is `max(counter, notes.len())`. The counter and the note kind
///    are not in perfect one-to-one correspondence — the inventory's
///    forbidden-*folder* note is filed under the `thread_skipped` kind but
///    bumps its own counter — so a note could otherwise outnumber its
///    counter and make `total - notes.len()` underflow into "the list is
///    complete." `max` keeps the sentence honest in the direction that
///    matters: never claim completeness we cannot back.
/// 2. Counter keys and note kinds come from `worker_mode::report`, not from
///    string literals typed here.
fn project_report(row: ReportRow) -> ReportDto {
    let counter = |key: &str| row.counters.get(key).copied().unwrap_or(0);
    let notes_of = |kind: &str| -> Vec<NoteDto> {
        row.notes
            .iter()
            .filter(|n| n.kind == kind)
            .map(|n| NoteDto {
                quip_thread_id: n.quip_thread_id.clone(),
                detail: n.detail.clone(),
            })
            .collect()
    };
    let outcome = |counter_key: &str, kind: &str| {
        let notes = notes_of(kind);
        OutcomeDto {
            total: counter(counter_key).max(notes.len() as u64),
            notes,
        }
    };

    ReportDto {
        imported: counter(report::THREADS_IMPORTED),
        skipped: outcome(report::THREADS_SKIPPED_FORBIDDEN, report::KIND_THREAD_SKIPPED),
        failed: outcome(report::THREADS_FAILED, report::KIND_THREAD_FAILED),
        chat_threads_skipped: counter(report::THREADS_SKIPPED_CHAT),
        notes_dropped: row.notes_dropped,
    }
}

/// `GET /api/v1/imports/quip/{id}` — owner-gated status + progress poll
/// for the wizard. A different user (or a nonexistent id) gets 404, never
/// 403 — same existence-hiding convention as `start`.
///
/// Also carries the import's outcome report (counters + the bounded note
/// list) so the wizard's completion state can say what the run *lost*, not
/// just what it finished. The report read is a single consistent `get_item`
/// on one small row — `ImportRepo::get_report`'s own doc comment budgets
/// for exactly this, one read per wizard poll.
///
/// The report never carries the Quip token: `ReportRow` has no token field,
/// its `detail` strings are built by `worker_mode::safe_quip_reason` (which
/// refuses Quip's raw response bodies), and
/// `report_response_never_carries_a_token_field` pins the response shape.
async fn get_status(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(import_id): Path<String>,
) -> Result<Json<StatusResponse>, ApiError> {
    let record = state
        .import_repo
        .get(&import_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .filter(|r| r.owner_id == user_id)
        .ok_or_else(|| ApiError::NotFound("import not found".to_string()))?;

    let (total, done) = state
        .import_repo
        .count_threads_by_state(&import_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // The report is ADVISORY on the read side too. A corrupt or un-decodable
    // `REPORT` row — a permanently poisoned counter is a real reachable state,
    // documented on `ImportRepo::bump_report_counter` — must not 500 the poll
    // the wizard runs every second: that would take down the read side over
    // the exact bookkeeping the write side is hardened never to fail an import
    // over. A failed read degrades to `report: None`, which the wizard already
    // renders (its pre-report wording), and the rest of the status — status,
    // phase, progress — returns normally. The error is logged, never
    // surfaced; it carries no token (`ReportRow` has no token field) and its
    // text is a DynamoDB decode message, not user or Quip content.
    let report = match state.import_repo.get_report(&import_id).await {
        Ok(row) => row.map(project_report),
        Err(e) => {
            tracing::warn!(
                import_id,
                error = %e,
                "quip status: report read failed; returning status without a report",
            );
            None
        }
    };

    let stage = match record.phase {
        0 => "scoping",
        1 => "inventory",
        _ => "content",
    };

    Ok(Json(StatusResponse {
        status: serde_json::to_string(&record.status)
            .unwrap()
            .trim_matches('"')
            .to_string(),
        phase: record.phase,
        progress: Progress {
            done,
            total,
            stage: stage.to_string(),
        },
        report,
    }))
}
