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
