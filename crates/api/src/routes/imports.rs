// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Quip import — Phase 0 Task 7: `POST /api/v1/imports/quip/connect`.
//!
//! Validates a pasted Quip personal access token against the real Quip
//! Automation API, creates the durable `ImportRecord` manifest (status
//! `Scoping`, no token — see `ogrenotes_storage::models::import`), stashes
//! the token in the `TokenStore` keyed by the new import id, and returns
//! the caller's Quip profile plus their root folders so Task 8's wizard
//! can render a folder picker.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_quip_import::{QuipError, QuipToken};
use ogrenotes_storage::models::import::{ImportRecord, ImportStatus};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/quip/connect", post(connect))
}

// ─── DTOs ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
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

    let folders = state
        .quip_client
        .folders(&token, &root_ids)
        .await
        .map_err(map_quip_error)?;

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

/// Map a `QuipError` to the ApiError this route exposes. Never surfaces
/// the token (neither error variant carries it — see
/// `QuipClient::observe_and_check`) or Quip's raw status/body text; both
/// user-visible messages are static strings independent of the
/// underlying cause.
fn map_quip_error(err: QuipError) -> ApiError {
    match err {
        QuipError::Unauthorized => ApiError::BadRequest("invalid Quip token".to_string()),
        QuipError::RateLimited { .. } | QuipError::Http(_) | QuipError::Api { .. } | QuipError::Parse(_) => {
            ApiError::ServiceUnavailable("Quip API error".to_string())
        }
    }
}
