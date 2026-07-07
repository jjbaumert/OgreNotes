// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.4 piece C — async-job status API.
//!
//! Two endpoints over the [`ogrenotes_worker::JobProducer`] handle
//! wired into [`AppState`]:
//!
//! - `POST /api/v1/jobs` — enqueue a job, returns its [`JobId`].
//! - `GET  /api/v1/jobs/{id}` — poll the job's [`JobStatus`].
//!
//! The frontend long-polls the GET endpoint until the status reaches
//! a terminal `succeeded`/`failed` state (v1; WebSocket push is a v2
//! carry per the phase-6 plan).
//!
//! Enqueue scope: this generic endpoint only accepts the `Noop`
//! variant. The real work payloads (`ImportDocx`/`ImportPdf`) carry
//! `owner_id` + `s3_key` fields that must be derived server-side from
//! the authenticated caller and a server-issued upload key — accepting
//! them on a client-controlled body would let a caller spoof another
//! user's ownership or point the worker at an arbitrary S3 object. Those
//! jobs get their own domain routes (`POST /documents/import-job`,
//! M-6.5/6.6) that construct the payload from trusted context. So the
//! generic surface stays a Noop-only queue smoke/health endpoint plus
//! the polling half that every job kind shares.
//!
//! Authorization model (#85): the `JobId` is an unguessable nanoid the
//! caller receives from its own enqueue (or import) call. Owned jobs
//! (the import variants — `Job::Import{Docx,Pdf}` carry an `owner_id`)
//! gate the GET endpoint on caller-id matching the recorded owner; a
//! mismatch returns 404 (not 403) so a leaked id can't be used to
//! confirm existence cross-user. Ownerless jobs (currently just
//! `Noop`) remain bearer-capability — any authenticated caller can
//! poll a label echo that carries no sensitive data.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_worker::{Job, JobError, JobStatus};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(enqueue))
        .route("/{id}", get(poll))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueRequest {
    /// Arbitrary correlation label echoed back by the worker in the
    /// terminal status's `result_json`. Bounded so a client can't
    /// stuff the Redis envelope with an unbounded string.
    label: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueResponse {
    job_id: String,
}

const MAX_LABEL_LEN: usize = 256;

/// POST /api/v1/jobs — enqueue a Noop job, returns its id.
async fn enqueue(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(req): Json<EnqueueRequest>,
) -> Result<(StatusCode, Json<EnqueueResponse>), ApiError> {
    let producer = state.job_producer.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Async job queue is not available".to_string())
    })?;

    let label = req.label.trim().to_string();
    if label.is_empty() {
        return Err(ApiError::BadRequest("label cannot be empty".to_string()));
    }
    if label.len() > MAX_LABEL_LEN {
        return Err(ApiError::BadRequest(format!(
            "label too long (max {MAX_LABEL_LEN} characters)"
        )));
    }

    let job_id = producer
        .enqueue(Job::Noop { label })
        .await
        .map_err(map_job_error)?;

    Ok((StatusCode::ACCEPTED, Json(EnqueueResponse { job_id })))
}

/// GET /api/v1/jobs/{id} — poll a job's status.
///
/// Owned jobs (the import variants) are gated on the caller's user id
/// matching the recorded owner; a mismatch returns 404 (not 403) so a
/// leaked job id can't be used to confirm a job exists on another
/// user's behalf. Ownerless jobs (currently just `Noop`) stay
/// bearer-capability — any authenticated user can poll. (#85)
async fn poll(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<JobStatus>, ApiError> {
    let producer = state.job_producer.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Async job queue is not available".to_string())
    })?;

    let (status, owner) = producer.poll(&id).await.map_err(map_job_error)?;
    if let Some(o) = owner.as_deref()
        && o != auth.user_id.as_str()
    {
        return Err(ApiError::NotFound(format!("job {id} not found")));
    }
    Ok(Json(status))
}

/// Map a worker-side [`JobError`] onto the HTTP error surface. A
/// missing/expired job id is a 404; everything else is a Redis or
/// serialization fault on our side, so it's a 500 — the caller can't
/// act on the distinction.
fn map_job_error(err: JobError) -> ApiError {
    match err {
        JobError::NotFound(id) => ApiError::NotFound(format!("job {id} not found")),
        JobError::Redis(_) | JobError::Serialize(_) => ApiError::Internal(err.to_string()),
    }
}
