// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Workspace management endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use samael::metadata::EntityDescriptor;
use samael::service_provider::ServiceProviderBuilder;

use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::workspace::{Workspace, WorkspaceMember};
use ogrenotes_storage::models::workspace_saml_config::{
    WorkspaceSamlConfig, MAX_METADATA_BYTES,
};
use ogrenotes_storage::models::WorkspaceRole;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_workspace))
        .route("/{id}", get(get_workspace))
        .route("/{id}", patch(update_workspace))
        .route("/{id}/members", get(list_members))
        .route("/{id}/members", post(add_member))
        .route("/{id}/members/{user_id}", delete(remove_member))
        // Phase 4 M-E3 piece D: workspace owner flips this to force
        // members through MFA enrollment on next login. PUT not
        // PATCH because the body is the complete new value, not a
        // patch document.
        .route("/{id}/mfa-required", axum::routing::put(set_mfa_required))
        // Phase 4 M-E4 piece B: per-workspace SAML IdP config.
        // GET surfaces current config (None if unset). PUT replaces
        // it; DELETE removes it. All three require workspace owner /
        // admin via `require_workspace_admin`.
        .route(
            "/{id}/saml-config",
            get(get_saml_config)
                .put(put_saml_config)
                .delete(delete_saml_config),
        )
        // Phase 4 M-E5 piece F: SCIM bearer tokens. POST mints a
        // fresh token (plaintext shown ONCE), GET lists existing
        // tokens (without secrets), DELETE revokes one.
        .route(
            "/{id}/scim-tokens",
            get(list_scim_tokens).post(create_scim_token),
        )
        .route(
            "/{id}/scim-tokens/{token_id}",
            delete(revoke_scim_token),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorkspaceRequest {
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceResponse {
    id: String,
    name: String,
    owner_id: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateWorkspaceRequest {
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMemberRequest {
    user_id: String,
    role: WorkspaceRole,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberResponse {
    user_id: String,
    name: String,
    email: String,
    role: WorkspaceRole,
    joined_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MembersListResponse {
    members: Vec<MemberResponse>,
}

/// POST /workspaces
async fn create_workspace(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<WorkspaceResponse>), ApiError> {
    let workspace_id = new_id();
    let now = now_usec();

    let workspace = Workspace {
        workspace_id: workspace_id.clone(),
        name: req.name.clone(),
        owner_id: user_id,
        mfa_required: false,
        created_at: now,
        updated_at: now,
    };

    state
        .workspace_repo
        .create(&workspace)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(WorkspaceResponse {
            id: workspace_id,
            name: req.name,
            owner_id: workspace.owner_id,
            created_at: now,
            updated_at: now,
        }),
    ))
}

/// GET /workspaces/:id
async fn get_workspace(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceResponse>, ApiError> {
    let ws = state
        .workspace_repo
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Workspace not found".to_string()))?;

    // Only owner or members may view
    if ws.owner_id != user_id {
        let member = state
            .workspace_repo
            .get_member(&id, &user_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if member.is_none() {
            return Err(ApiError::NotFound("Workspace not found".to_string()));
        }
    }

    Ok(Json(WorkspaceResponse {
        id: ws.workspace_id,
        name: ws.name,
        owner_id: ws.owner_id,
        created_at: ws.created_at,
        updated_at: ws.updated_at,
    }))
}

/// PATCH /workspaces/:id
async fn update_workspace(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateWorkspaceRequest>,
) -> Result<StatusCode, ApiError> {
    let _ws = require_workspace_admin(&state, &id, &user_id).await?;

    state
        .workspace_repo
        .update(&id, req.name.as_deref(), now_usec())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /workspaces/:id/members
async fn list_members(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<MembersListResponse>, ApiError> {
    let ws = state
        .workspace_repo
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Workspace not found".to_string()))?;

    if ws.owner_id != user_id {
        let member = state
            .workspace_repo
            .get_member(&id, &user_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if member.is_none() {
            return Err(ApiError::Forbidden);
        }
    }

    let members = state
        .workspace_repo
        .list_members(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut responses = Vec::new();
    for m in members {
        let (name, email) = match state.user_repo.get_by_id(&m.user_id).await {
            Ok(Some(user)) => (user.name, user.email),
            _ => (m.user_id.clone(), String::new()),
        };
        responses.push(MemberResponse {
            user_id: m.user_id,
            name,
            email,
            role: m.role,
            joined_at: m.joined_at,
        });
    }

    Ok(Json(MembersListResponse { members: responses }))
}

/// Check if the caller is the owner or has Admin role. `pub(crate)` so
/// other workspace-scoped admin routes (template galleries, etc.) can
/// reuse the same guard without duplicating its owner + Admin fallback.
pub(crate) async fn require_workspace_admin(
    state: &AppState,
    workspace_id: &str,
    user_id: &str,
) -> Result<Workspace, ApiError> {
    let ws = state
        .workspace_repo
        .get(workspace_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Workspace not found".to_string()))?;

    if ws.owner_id == user_id {
        return Ok(ws);
    }

    // Admin role can manage members
    if let Ok(Some(member)) = state.workspace_repo.get_member(workspace_id, user_id).await {
        if member.role == WorkspaceRole::Admin {
            return Ok(ws);
        }
    }

    Err(ApiError::Forbidden)
}

/// POST /workspaces/:id/members
async fn add_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<AddMemberRequest>,
) -> Result<StatusCode, ApiError> {
    let _ws = require_workspace_admin(&state, &id, &user_id).await?;

    if body.user_id == user_id {
        return Err(ApiError::BadRequest("Cannot add yourself".to_string()));
    }

    if body.role == WorkspaceRole::Owner {
        return Err(ApiError::BadRequest("Cannot grant Owner role".to_string()));
    }

    if state.user_repo.get_by_id(&body.user_id).await?.is_none() {
        return Err(ApiError::NotFound("User not found".to_string()));
    }

    // Check if user is already a member
    if let Ok(Some(_)) = state.workspace_repo.get_member(&id, &body.user_id).await {
        return Err(ApiError::Conflict("User is already a member".to_string()));
    }

    let member = WorkspaceMember {
        workspace_id: id,
        user_id: body.user_id,
        role: body.role,
        joined_at: now_usec(),
    };

    state
        .workspace_repo
        .add_member(&member)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /workspaces/:id/members/:user_id
async fn remove_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, target_user_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let ws = require_workspace_admin(&state, &id, &user_id).await?;

    if target_user_id == ws.owner_id {
        return Err(ApiError::BadRequest("Cannot remove the owner".to_string()));
    }

    state
        .workspace_repo
        .remove_member(&id, &target_user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetMfaRequiredRequest {
    required: bool,
}

/// PUT /workspaces/:id/mfa-required
///
/// Workspace-admin-only. When `required = true`, members who haven't
/// yet enrolled in MFA see `mfa_enrollment_required: true` on their
/// next login response — the frontend redirects them to enrollment
/// before they can navigate. Already-enrolled members are unaffected
/// (their next login goes through the existing MFA challenge step).
/// Setting `required = false` REMOVEs the attribute on the row;
/// future login responses omit `mfa_enrollment_required` entirely.
async fn set_mfa_required(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<SetMfaRequiredRequest>,
) -> Result<StatusCode, ApiError> {
    require_workspace_admin(&state, &id, &user_id).await?;
    state
        .workspace_repo
        .set_mfa_required(&id, req.required)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    // Structured tracing event — the durable forensic trail for
    // "who flipped the MFA flag on which workspace and when."
    // AdminAudit's key shape (target_user_id) doesn't fit a
    // workspace mutation; the unified audit table is M-E6 work, so
    // log-only for now. Matches the fallback pattern in
    // `routes::admin::record_admin_action`.
    tracing::info!(
        event_type = "workspace_mfa_required_changed",
        workspace_id = %id,
        actor_id = %user_id,
        required = req.required,
        "workspace_mfa_required_changed"
    );
    Ok(StatusCode::NO_CONTENT)
}

// ─── SAML IdP config (Phase 4 M-E4 piece B) ──────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutSamlConfigRequest {
    idp_entity_id: String,
    idp_metadata_xml: String,
    /// Optional with sensible defaults. Most IdPs publish the
    /// simple `email`/`name` attribute names; admins only override
    /// for AD FS-style schema URIs.
    #[serde(default = "default_attribute_email")]
    attribute_email: String,
    #[serde(default = "default_attribute_name")]
    attribute_name: String,
}

fn default_attribute_email() -> String {
    "email".to_string()
}

fn default_attribute_name() -> String {
    "name".to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SamlConfigResponse {
    workspace_id: String,
    idp_entity_id: String,
    /// Returned verbatim so the admin UI can render a "currently
    /// configured" view without re-uploading. Not redacted — IdP
    /// metadata is public information (the certificate it embeds
    /// is the IdP's PUBLIC signing key).
    idp_metadata_xml: String,
    attribute_email: String,
    attribute_name: String,
    created_at: i64,
    updated_at: i64,
}

/// GET /workspaces/:id/saml-config
async fn get_saml_config(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Option<SamlConfigResponse>>, ApiError> {
    require_workspace_admin(&state, &id, &user_id).await?;
    let config = state.workspace_saml_config_repo.get(&id).await?;
    Ok(Json(config.map(|c| SamlConfigResponse {
        workspace_id: c.workspace_id,
        idp_entity_id: c.idp_entity_id,
        idp_metadata_xml: c.idp_metadata_xml,
        attribute_email: c.attribute_email,
        attribute_name: c.attribute_name,
        created_at: c.created_at,
        updated_at: c.updated_at,
    })))
}

/// PUT /workspaces/:id/saml-config
async fn put_saml_config(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<PutSamlConfigRequest>,
) -> Result<StatusCode, ApiError> {
    require_workspace_admin(&state, &id, &user_id).await?;

    if req.idp_entity_id.trim().is_empty() {
        return Err(ApiError::BadRequest("idp_entity_id is required".to_string()));
    }
    if req.idp_metadata_xml.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "idp_metadata_xml is required".to_string(),
        ));
    }
    if req.idp_metadata_xml.len() > MAX_METADATA_BYTES {
        return Err(ApiError::BadRequest(format!(
            "idp_metadata_xml exceeds {MAX_METADATA_BYTES}-byte cap; \
             trim the IdP-published document or open an issue if you need a larger cap"
        )));
    }
    if !req.idp_metadata_xml.contains('<') {
        return Err(ApiError::BadRequest(
            "idp_metadata_xml must be an XML document".to_string(),
        ));
    }

    // Gap-001: reject IdP metadata that doesn't carry at least one
    // signing certificate. samael's verify path is "no signing certs
    // → skip XMLDSig verification entirely", so cert-free metadata
    // silently disables every signature check for the workspace.
    // Use samael's own idp_signing_certs() so the check matches the
    // verify-path logic bit-for-bit.
    let idp_metadata: EntityDescriptor =
        req.idp_metadata_xml.parse().map_err(|e| {
            tracing::warn!(error = ?e, workspace_id = %id, "idp_metadata_xml failed to parse");
            ApiError::BadRequest(
                "idp_metadata_xml is not a valid SAML 2.0 EntityDescriptor".to_string(),
            )
        })?;
    // Best-effort SP build — entity_id + acs_url are required by the
    // builder but their values don't matter for the cert check, so
    // pin placeholders.
    let probe_sp = ServiceProviderBuilder::default()
        .entity_id("sp-probe".to_string())
        .acs_url("sp-probe".to_string())
        .idp_metadata(idp_metadata)
        .build()
        .map_err(|e| {
            tracing::warn!(error = ?e, workspace_id = %id, "probe ServiceProvider build failed");
            ApiError::BadRequest(
                "idp_metadata_xml could not be loaded as an IdP descriptor".to_string(),
            )
        })?;
    match probe_sp.idp_signing_certs() {
        Ok(Some(certs)) if !certs.is_empty() => {}
        Ok(_) => {
            return Err(ApiError::BadRequest(
                "idp_metadata_xml has no signing KeyDescriptor; SAML signature \
                 verification cannot be performed without an IdP signing cert. \
                 Re-export the metadata from your IdP and confirm it includes \
                 <KeyDescriptor use=\"signing\">."
                    .to_string(),
            ));
        }
        Err(e) => {
            tracing::warn!(error = ?e, workspace_id = %id, "IdP signing cert failed to parse");
            return Err(ApiError::BadRequest(
                "idp_metadata_xml signing certificate failed to parse as X.509 — \
                 confirm the <X509Certificate> body is base64-encoded DER."
                    .to_string(),
            ));
        }
    }

    let now = now_usec();
    // Preserve created_at across updates so the UI can show "first
    // configured at X." Without this read-merge, every PUT would
    // overwrite created_at with `now`. A DDB read failure here
    // propagates (was previously swallowed by `.ok()` → silent
    // `created_at` reset on transient errors).
    let existing_created_at = state
        .workspace_saml_config_repo
        .get(&id)
        .await?
        .map(|c| c.created_at)
        .unwrap_or(now);

    let config = WorkspaceSamlConfig {
        workspace_id: id.clone(),
        idp_entity_id: req.idp_entity_id,
        idp_metadata_xml: req.idp_metadata_xml,
        attribute_email: req.attribute_email,
        attribute_name: req.attribute_name,
        created_at: existing_created_at,
        updated_at: now,
    };
    state.workspace_saml_config_repo.put(&config).await?;

    tracing::info!(
        event_type = "workspace_saml_config_changed",
        workspace_id = %id,
        actor_id = %user_id,
        idp_entity_id = %config.idp_entity_id,
        "workspace_saml_config_changed"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /workspaces/:id/saml-config
async fn delete_saml_config(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_workspace_admin(&state, &id, &user_id).await?;
    state.workspace_saml_config_repo.delete(&id).await?;
    tracing::info!(
        event_type = "workspace_saml_config_removed",
        workspace_id = %id,
        actor_id = %user_id,
        "workspace_saml_config_removed"
    );
    Ok(StatusCode::NO_CONTENT)
}

// ─── SCIM token management (Phase 4 M-E5 piece F) ───────────────

/// Response shape for `POST /workspaces/:id/scim-tokens`. The
/// plaintext token appears ONCE here and is never recoverable
/// after this response is returned — only the bcrypt hash lives
/// in DDB.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedScimTokenResponse {
    token_id: String,
    /// `<token_id>.<secret>` — the wire-format bearer string the
    /// admin pastes into the IdP's SCIM connector. Shown once.
    token: String,
    name: String,
    created_at: i64,
}

/// Response shape for `GET /workspaces/:id/scim-tokens`. Same fields
/// as the token row except `secret_hash` (admin doesn't need it)
/// and a derived `is_active` boolean for the UI.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScimTokenSummary {
    token_id: String,
    name: String,
    created_at: i64,
    last_used_at: i64,
    disabled_at: i64,
    is_active: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateScimTokenRequest {
    /// Admin-set label (e.g., "Okta connector for Acme"). Required
    /// because tracking unnamed tokens is a usability disaster.
    name: String,
}

/// POST /workspaces/:id/scim-tokens
///
/// Mints a fresh SCIM bearer token. Returns the plaintext exactly
/// once in the response body; subsequent reads return only the
/// `token_id` and metadata. Workspace owner / admin only.
async fn create_scim_token(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<CreateScimTokenRequest>,
) -> Result<(StatusCode, Json<CreatedScimTokenResponse>), ApiError> {
    require_workspace_admin(&state, &id, &user_id).await?;

    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }
    if name.len()
        > ogrenotes_storage::models::workspace_scim_token::MAX_TOKEN_NAME_LEN
    {
        return Err(ApiError::BadRequest(format!(
            "name exceeds {} bytes",
            ogrenotes_storage::models::workspace_scim_token::MAX_TOKEN_NAME_LEN,
        )));
    }

    let minted = crate::middleware::scim_auth::mint_token().map_err(|e| {
        tracing::error!(error = %e, "SCIM mint_token failed");
        ApiError::Internal("token mint".to_string())
    })?;

    let now = now_usec();
    let row = ogrenotes_storage::models::workspace_scim_token::WorkspaceScimToken {
        workspace_id: id.clone(),
        token_id: minted.token_id.clone(),
        secret_hash: minted.secret_hash,
        name: name.to_string(),
        created_at: now,
        last_used_at: 0,
        disabled_at: 0,
    };
    state.workspace_scim_token_repo.put(&row).await?;

    tracing::info!(
        event_type = "workspace_scim_token_created",
        workspace_id = %id,
        token_id = %minted.token_id,
        actor_id = %user_id,
        "workspace_scim_token_created"
    );

    Ok((
        StatusCode::CREATED,
        Json(CreatedScimTokenResponse {
            token_id: minted.token_id,
            token: minted.plaintext,
            name: name.to_string(),
            created_at: now,
        }),
    ))
}

/// GET /workspaces/:id/scim-tokens
///
/// Lists every token (active + revoked) for the workspace.
/// Plaintext / secret-hash are never returned. Workspace owner /
/// admin only.
async fn list_scim_tokens(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<ScimTokenSummary>>, ApiError> {
    require_workspace_admin(&state, &id, &user_id).await?;
    let rows = state
        .workspace_scim_token_repo
        .list_for_workspace(&id)
        .await?;
    let out: Vec<ScimTokenSummary> = rows
        .into_iter()
        .map(|r| ScimTokenSummary {
            is_active: r.is_active(),
            token_id: r.token_id,
            name: r.name,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
            disabled_at: r.disabled_at,
        })
        .collect();
    Ok(Json(out))
}

/// DELETE /workspaces/:id/scim-tokens/:token_id
///
/// Revokes a SCIM token by setting `disabled_at`. The row stays
/// in DDB so historical SecurityAudit references to the token_id
/// still resolve.
async fn revoke_scim_token(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, token_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    require_workspace_admin(&state, &id, &user_id).await?;
    let now = now_usec();
    state
        .workspace_scim_token_repo
        .set_disabled_at(&id, &token_id, now)
        .await?;
    tracing::info!(
        event_type = "workspace_scim_token_revoked",
        workspace_id = %id,
        token_id = %token_id,
        actor_id = %user_id,
        "workspace_scim_token_revoked"
    );
    Ok(StatusCode::NO_CONTENT)
}
