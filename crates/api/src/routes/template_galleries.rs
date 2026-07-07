// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #142 Phase 4 — admin CRUD for workspace-scoped Company template
//! galleries.
//!
//! Routes (all under `/api/v1/admin` — the router is merged with
//! `admin::router()` at wire time):
//!
//!   POST   /api/v1/admin/workspaces/:ws_id/template-galleries
//!   GET    /api/v1/admin/workspaces/:ws_id/template-galleries
//!   GET    /api/v1/admin/workspaces/:ws_id/template-galleries/:gallery_id
//!   PATCH  /api/v1/admin/workspaces/:ws_id/template-galleries/:gallery_id
//!   DELETE /api/v1/admin/workspaces/:ws_id/template-galleries/:gallery_id
//!
//! Auth: caller must be the workspace owner or have `WorkspaceRole::Admin`
//! on the target workspace (`require_workspace_admin` from `routes/workspaces`).
//!
//! Membership shape: a gallery holds a Vec of doc ids. The docs
//! themselves live wherever they were created — the gallery only tracks
//! grouping. `list_templates` fetches metadata per doc id and drops any
//! the caller can't view, so a stale membership doesn't leak content.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::template_gallery::{
    TemplateGallery, MAX_GALLERY_DOC_IDS, MAX_GALLERY_NAME_LEN,
};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::routes::audit::record_security_event;
use crate::routes::workspaces::require_workspace_admin;
use crate::state::AppState;

/// Routes are relative to the `/api/v1/admin` prefix — merged into
/// `admin::router()` at wire time by `routes::mod::api_router`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/workspaces/{ws_id}/template-galleries",
            post(create_gallery).get(list_galleries),
        )
        .route(
            "/workspaces/{ws_id}/template-galleries/{gallery_id}",
            get(get_gallery).patch(update_gallery).delete(delete_gallery),
        )
}

// ─── DTOs ──────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateGalleryRequest {
    name: String,
    #[serde(default)]
    doc_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateGalleryRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    doc_ids: Option<Vec<String>>,
    /// Explicit consent for a destructive membership wipe. `docIds: []`
    /// PATCHes are rejected unless this is `Some(true)` — protects
    /// against a client that ossifies the schema and sends `[]` as a
    /// default for an unset field, silently clearing the gallery.
    #[serde(default)]
    clear_membership: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GalleryResponse {
    id: String,
    workspace_id: String,
    name: String,
    doc_ids: Vec<String>,
    created_by: String,
    created_at: i64,
    updated_at: i64,
}

impl From<TemplateGallery> for GalleryResponse {
    fn from(g: TemplateGallery) -> Self {
        Self {
            id: g.gallery_id,
            workspace_id: g.workspace_id,
            name: g.name,
            doc_ids: g.doc_ids,
            created_by: g.created_by,
            created_at: g.created_at,
            updated_at: g.updated_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GalleryListResponse {
    galleries: Vec<GalleryResponse>,
}

// ─── Validators ────────────────────────────────────────────────

/// Enforce the name length cap + reject empty/whitespace-only names.
/// Empty names would produce blank picker section headers; the length
/// cap keeps admins from pathologically stretching the row layout.
fn validate_name(name: &str) -> Result<(), ApiError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest("Gallery name cannot be empty".into()));
    }
    if name.chars().count() > MAX_GALLERY_NAME_LEN {
        return Err(ApiError::BadRequest(format!(
            "Gallery name is too long (max {MAX_GALLERY_NAME_LEN} characters)",
        )));
    }
    Ok(())
}

/// Enforce the per-gallery doc-count cap and dedupe while preserving
/// first-occurrence order. Returns the canonical Vec that should be
/// persisted AND echoed back in the response — otherwise the
/// create/update response body diverges from the row a subsequent GET
/// returns (repo also dedupes as a defense-in-depth belt).
fn canonicalize_doc_ids(doc_ids: Vec<String>) -> Result<Vec<String>, ApiError> {
    if doc_ids.len() > MAX_GALLERY_DOC_IDS {
        return Err(ApiError::BadRequest(format!(
            "Gallery cannot hold more than {MAX_GALLERY_DOC_IDS} templates",
        )));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(doc_ids.len());
    for id in doc_ids {
        if seen.insert(id.clone()) {
            out.push(id);
        }
    }
    Ok(out)
}

/// Verify each doc id resolves to a live document. Runs the reads
/// concurrently — the caller already capped input at
/// `MAX_GALLERY_DOC_IDS`, so this is a bounded fan-out. Returns 400
/// listing the offending ids so a curl-driven typo or a paste from a
/// mock list surfaces at write time instead of silently disappearing
/// from the picker at read time.
async fn require_doc_ids_exist(state: &AppState, doc_ids: &[String]) -> Result<(), ApiError> {
    if doc_ids.is_empty() {
        return Ok(());
    }
    let fetches = doc_ids.iter().cloned().map(|id| {
        let repo = state.doc_repo.clone();
        async move {
            let exists = repo.get(&id).await.ok().flatten().is_some();
            (id, exists)
        }
    });
    let missing: Vec<String> = futures_util::future::join_all(fetches)
        .await
        .into_iter()
        .filter_map(|(id, ok)| if ok { None } else { Some(id) })
        .collect();
    if !missing.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "unknown doc ids: {}",
            missing.join(", ")
        )));
    }
    Ok(())
}

// ─── Handlers ──────────────────────────────────────────────────

async fn create_gallery(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(ws_id): Path<String>,
    Json(req): Json<CreateGalleryRequest>,
) -> Result<(StatusCode, Json<GalleryResponse>), ApiError> {
    let _ws = require_workspace_admin(&state, &ws_id, &user_id).await?;
    validate_name(&req.name)?;
    let doc_ids = canonicalize_doc_ids(req.doc_ids)?;
    require_doc_ids_exist(&state, &doc_ids).await?;

    let now = now_usec();
    let gallery = TemplateGallery {
        workspace_id: ws_id.clone(),
        gallery_id: new_id(),
        name: req.name.trim().to_string(),
        doc_ids,
        created_by: user_id.clone(),
        created_at: now,
        updated_at: now,
    };
    state
        .template_gallery_repo
        .put(&gallery)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Gallery mutations are admin curation of workspace-visible shared
    // state. AdminAudit's key shape (PK = USER#<target_user_id>) doesn't
    // fit a workspace-scoped mutation — same reason set_mfa_required in
    // routes/workspaces.rs falls back to structured tracing rather than
    // AdminAudit. SecurityAudit's 90-day retention is a compromise until
    // the unified audit table (M-E6) lands; upgrading to permanent
    // retention requires a schema change, not a routing change.
    record_security_event(
        &state,
        &user_id,
        ogrenotes_storage::models::security_audit::SecurityAuditAction::TemplateGalleryCreated {
            workspace_id: gallery.workspace_id.clone(),
            gallery_id: gallery.gallery_id.clone(),
        },
    );

    Ok((StatusCode::CREATED, Json(gallery.into())))
}

async fn list_galleries(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(ws_id): Path<String>,
) -> Result<Json<GalleryListResponse>, ApiError> {
    let _ws = require_workspace_admin(&state, &ws_id, &user_id).await?;
    let rows = state
        .template_gallery_repo
        .list_for_workspace(&ws_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(GalleryListResponse {
        galleries: rows.into_iter().map(Into::into).collect(),
    }))
}

async fn get_gallery(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((ws_id, gallery_id)): Path<(String, String)>,
) -> Result<Json<GalleryResponse>, ApiError> {
    let _ws = require_workspace_admin(&state, &ws_id, &user_id).await?;
    let gallery = state
        .template_gallery_repo
        .get(&ws_id, &gallery_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("Gallery not found".into()))?;
    Ok(Json(gallery.into()))
}

async fn update_gallery(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((ws_id, gallery_id)): Path<(String, String)>,
    Json(req): Json<UpdateGalleryRequest>,
) -> Result<Json<GalleryResponse>, ApiError> {
    let _ws = require_workspace_admin(&state, &ws_id, &user_id).await?;
    let mut gallery = state
        .template_gallery_repo
        .get(&ws_id, &gallery_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("Gallery not found".into()))?;

    // Empty PATCH ({}) is a no-op: skip the DDB rewrite AND the audit
    // emission entirely. Otherwise a health-check loop hammering the
    // endpoint would fill the audit log with no-op rows and spend WCU
    // rewriting the same row over and over.
    if req.name.is_none() && req.doc_ids.is_none() {
        return Ok(Json(gallery.into()));
    }

    if let Some(name) = req.name.as_ref() {
        validate_name(name)?;
        gallery.name = name.trim().to_string();
    }
    if let Some(doc_ids) = req.doc_ids {
        // Guard against a client that silently defaults `docIds` to
        // `[]` for an unset field — the Option distinction is only
        // useful if the wire actually preserves it. Force the caller
        // to declare destructive intent via `clearMembership: true`.
        if doc_ids.is_empty() && req.clear_membership != Some(true) {
            return Err(ApiError::BadRequest(
                "docIds: [] would wipe gallery membership — pass \
                 clearMembership: true to confirm the destructive intent"
                    .into(),
            ));
        }
        let doc_ids = canonicalize_doc_ids(doc_ids)?;
        require_doc_ids_exist(&state, &doc_ids).await?;
        gallery.doc_ids = doc_ids;
    }
    gallery.updated_at = now_usec();

    state
        .template_gallery_repo
        .put(&gallery)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    record_security_event(
        &state,
        &user_id,
        ogrenotes_storage::models::security_audit::SecurityAuditAction::TemplateGalleryUpdated {
            workspace_id: gallery.workspace_id.clone(),
            gallery_id: gallery.gallery_id.clone(),
        },
    );

    Ok(Json(gallery.into()))
}

async fn delete_gallery(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((ws_id, gallery_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let _ws = require_workspace_admin(&state, &ws_id, &user_id).await?;
    // Load first so a delete on a nonexistent id returns 404 (rather
    // than a silent 204) and so the audit row has the gallery name /
    // doc-count context if we want to add it later.
    let _existing = state
        .template_gallery_repo
        .get(&ws_id, &gallery_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("Gallery not found".into()))?;

    state
        .template_gallery_repo
        .delete(&ws_id, &gallery_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    record_security_event(
        &state,
        &user_id,
        ogrenotes_storage::models::security_audit::SecurityAuditAction::TemplateGalleryDeleted {
            workspace_id: ws_id,
            gallery_id,
        },
    );

    Ok(StatusCode::NO_CONTENT)
}
