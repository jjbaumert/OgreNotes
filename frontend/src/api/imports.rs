// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// Quip import — Phase 0 client for `POST /imports/quip/connect`. The
// wizard (components/quip_import) sends the pasted Quip personal
// access token once; the backend exchanges it for a QuipClient,
// probes `current_user` + `folders`, and persists an `ImportRecord`
// (no token field — see crates/storage's `ImportRepo`, by design).
// The token itself never round-trips back to the client after this
// call; the response carries only the profile + root-folder listing
// the scope step (Phase 1) will build on.

use serde::{Deserialize, Serialize};

use crate::api::client::{self, ApiClientError};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectRequest<'a> {
    token: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuipProfile {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootFolder {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResponse {
    pub import_id: String,
    pub quip_profile: QuipProfile,
    pub root_folders: Vec<RootFolder>,
}

/// Exchange a pasted Quip personal access token for a connected
/// import session. The token is sent once over this call and never
/// stored client-side beyond the request body — the wizard clears its
/// token signal as soon as this resolves (success or failure).
pub async fn connect(token: &str) -> Result<ConnectResponse, ApiClientError> {
    client::api_post("/imports/quip/connect", &ConnectRequest { token }).await
}

// ─── Phase 1: start + status poll ──────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartRequest<'a> {
    selected_root_folder_ids: &'a [String],
    target_folder_id: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResponse {
    pub import_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub stage: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub status: String,
    pub phase: u8,
    pub progress: Progress,
    /// The folder this import's documents land in — where "Open folder"
    /// takes the user (#174) — or `None` when the server named no
    /// destination.
    ///
    /// `None` has two causes and one handling: an import that was never
    /// started (no destination chosen), or a server older than #174 that
    /// does not send the field at all. `#[serde(default)]` is what makes
    /// the second case decode rather than error mid-rolling-deploy, and the
    /// wizard falls back to Home either way — a missing destination must
    /// never be guessed at.
    #[serde(default)]
    pub destination_folder_id: Option<String>,
    /// The run's outcome report, or `None` when the server has no `REPORT`
    /// row for this import.
    ///
    /// `None` is not "an all-zero report". Report writes are advisory
    /// server-side (they must never be able to halt an import), so a
    /// perfectly successful run can finish without one. Treating `None` as
    /// `imported: 0` would make the wizard announce "Imported 0 documents"
    /// after a clean 47-document run, so the wizard falls back to its
    /// pre-report wording instead. `#[serde(default)]` additionally keeps a
    /// client newer than its server decoding.
    #[serde(default)]
    pub report: Option<ImportReport>,
}

/// The import's outcome report: what landed, and what did not.
///
/// Mirrors `routes::imports::ReportDto`. Skips and failures are separate
/// fields, not one "problems" list, because they mean different things to
/// the person reading them — see [`ImportReport::skipped`] /
/// [`ImportReport::failed`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Documents actually written into the destination folder.
    #[serde(default)]
    pub imported: u64,
    /// Documents Quip refused to serve (HTTP 403). Actionable: the user may
    /// be able to get access and re-run.
    #[serde(default)]
    pub skipped: Outcome,
    /// Documents the importer tried repeatedly and gave up on. Not
    /// actionable in the same way — retrying already happened.
    #[serde(default)]
    pub failed: Outcome,
    /// Quip chat threads. Counted, never named: chats are not documents.
    #[serde(default)]
    pub chat_threads_skipped: u64,
    /// Row-global count of discarded notes, spanning kinds this wizard does
    /// not render (dropped images, truncated nesting). **Deliberately not
    /// used for the "and N more" line** — it cannot say which section lost
    /// notes, and attributing an image-note drop to the document list would
    /// be a new lie in place of the old one. [`Outcome::hidden`] is the
    /// per-section truncation signal.
    #[serde(default)]
    pub notes_dropped: u64,
}

/// One outcome class: the true total plus a bounded sample of named threads.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// The true total, from the server's uncapped counters. **Not**
    /// `notes.len()`, which stops at the storage row's per-kind budget.
    #[serde(default)]
    pub total: u64,
    /// Named examples — a bounded prefix, not the whole set.
    #[serde(default)]
    pub notes: Vec<ReportNote>,
}

impl Outcome {
    /// How many threads are in [`Self::total`] but absent from
    /// [`Self::notes`] — the "…and N more" number. Saturating, so a server
    /// that ever sent more notes than its counter yields `0` rather than
    /// wrapping to a nonsense total.
    pub fn hidden(&self) -> u64 {
        self.total.saturating_sub(self.notes.len() as u64)
    }
}

/// One named loss. `detail` is server-authored, already stripped of Quip's
/// raw response body and any URL, and is **plain text** — it is rendered
/// through a text node, never as markup.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportNote {
    /// Empty when the loss was not thread-scoped (e.g. a whole folder the
    /// inventory could not read). Rendered as `detail` alone in that case.
    #[serde(default)]
    pub quip_thread_id: String,
    #[serde(default)]
    pub detail: String,
}

/// Kick off the actual import run for a connected session: persist the
/// chosen root-folder scope + destination, and enqueue the token-free
/// worker trigger (`Job::StartQuipImport`) server-side. No token travels
/// with this call — it never left the server after `connect`.
pub async fn start(
    import_id: &str,
    selected_root_folder_ids: &[String],
    target_folder_id: &str,
) -> Result<StartResponse, ApiClientError> {
    client::api_post(
        &format!("/imports/quip/{import_id}/start"),
        &StartRequest {
            selected_root_folder_ids,
            target_folder_id,
        },
    )
    .await
}

/// Poll the current phase/progress of an in-flight import. During
/// inventory (Phase 1), `progress.total` climbs as Quip threads are
/// discovered while `progress.done` stays 0; `phase` flips to `1` once
/// the inventory walk is complete. During the content pass, `progress.done`
/// climbs toward `progress.total` for free (threads move
/// `Pending -> ContentDone`); `phase` flips to `2` once content is done.
/// `status` stays `"running"` through both passes and flips to a terminal
/// `"succeeded"` when the content pass completes (written just after
/// `phase = 2`, so a poll can legitimately observe `phase == 2` while
/// `status` is still `"running"`). Callers therefore stop polling on
/// `phase >= 2` — not on `status` — or on a failure `status`
/// (`failed` / `tokenrejected` / `cancelled`).
///
/// Every poll also carries [`StatusResponse::report`] — the run's
/// counters plus a bounded sample of the threads it could not import — so
/// the completion state can say what was *lost*, not only what finished.
/// See [`ImportReport`] for why `None` is not "an all-zero report".
pub async fn get_status(import_id: &str) -> Result<StatusResponse, ApiClientError> {
    client::api_get(&format!("/imports/quip/{import_id}")).await
}
