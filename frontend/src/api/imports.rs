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
/// `status` stays `"running"` through both passes — Phase 2b is what sets
/// a terminal `"succeeded"` — so callers stop polling on `phase >= 2` or
/// on a failure `status` (`failed` / `tokenrejected` / `cancelled`).
pub async fn get_status(import_id: &str) -> Result<StatusResponse, ApiClientError> {
    client::api_get(&format!("/imports/quip/{import_id}")).await
}
