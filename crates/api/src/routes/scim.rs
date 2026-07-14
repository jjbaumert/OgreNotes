// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! `/scim/v2/workspaces/<id>/Users` — SCIM 2.0 User CRUD (Phase 4
//! M-E5 piece D).
//!
//! The router is mounted under `/scim/v2` at the api crate root.
//! Each endpoint:
//!   1. Calls `verify_scim_request` to authenticate the bearer
//!      token against the URL's workspace_id.
//!   2. Carries out the SCIM operation, marshalling between the
//!      wire DTOs and the internal `User` / workspace-member model.
//!   3. On any error returns a SCIM-shaped error body (not the
//!      OgreNotes default `ApiError` body) — IdPs parse the SCIM
//!      schema and would otherwise misclassify generic errors.
//!
//! Endpoints in this file:
//!
//!   GET    /workspaces/:ws/Users
//!   GET    /workspaces/:ws/Users/:id
//!   POST   /workspaces/:ws/Users
//!   PUT    /workspaces/:ws/Users/:id
//!   PATCH  /workspaces/:ws/Users/:id
//!   DELETE /workspaces/:ws/Users/:id
//!
//! Group endpoints and the static /ServiceProviderConfig endpoints
//! land in piece E.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;

use ogrenotes_storage::models::security_audit::SecurityAuditAction;
use ogrenotes_storage::models::workspace::WorkspaceMember;
use ogrenotes_storage::models::WorkspaceRole;

use crate::middleware::scim_auth::{verify_scim_request, ScimAuth};
use crate::routes::audit::record_security_event;
use crate::scim::discovery;
use crate::scim::dtos::{
    scim_type, GroupMember, ListResponse, PatchOp, PatchVerb, ScimError, ScimGroup,
    ScimUser, SCHEMA_GROUP,
};
use crate::scim::filter::{parse_filter, SupportedFilter};
use crate::scim::mapping::user_to_scim;
use crate::state::AppState;

/// SCIM pagination defaults. RFC 7644 §3.4.2.4 says servers MAY
/// pick their own; Okta and Entra both use 100, JumpCloud uses 50.
/// We pick 50 as a middle-ground that won't blow up DDB read
/// capacity on a workspace with thousands of members.
const DEFAULT_COUNT: usize = 50;
/// Hard cap. SCIM clients may request larger pages; we silently
/// truncate to keep DDB query sizes bounded.
const MAX_COUNT: usize = 200;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{ws_id}/Users", get(list_users))
        .route("/workspaces/{ws_id}/Users", post(create_user))
        .route("/workspaces/{ws_id}/Users/{user_id}", get(get_user))
        .route("/workspaces/{ws_id}/Users/{user_id}", put(replace_user))
        .route(
            "/workspaces/{ws_id}/Users/{user_id}",
            patch(patch_user),
        )
        .route(
            "/workspaces/{ws_id}/Users/{user_id}",
            delete(deprovision_user),
        )
        // Groups: piece E. Each workspace is one Group; the
        // group_id in the URL path must equal the URL's ws_id.
        .route("/workspaces/{ws_id}/Groups", get(list_groups))
        .route(
            "/workspaces/{ws_id}/Groups/{group_id}",
            get(get_group),
        )
        .route(
            "/workspaces/{ws_id}/Groups/{group_id}",
            patch(patch_group),
        )
        // Discovery endpoints: piece E. Same auth as the resource
        // endpoints — workspace-scoped via the URL even though the
        // body is static.
        .route(
            "/workspaces/{ws_id}/ServiceProviderConfig",
            get(get_service_provider_config),
        )
        .route(
            "/workspaces/{ws_id}/ResourceTypes",
            get(get_resource_types),
        )
        .route("/workspaces/{ws_id}/Schemas", get(get_schemas))
}

// ─── Query parameter shape ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    #[serde(rename = "startIndex")]
    start_index: Option<usize>,
    #[serde(default)]
    count: Option<usize>,
}

// ─── Handlers ───────────────────────────────────────────────────

/// `GET /scim/v2/workspaces/:ws/Users`
///
/// Lists workspace members, optionally filtered. The filter is
/// `userName eq "..."` or `externalId eq "..."` — see
/// `scim::filter` for the supported subset. Returns a SCIM
/// ListResponse envelope (Resources capital-R, RFC-mandated).
async fn list_users(
    State(state): State<AppState>,
    Path(ws_id): Path<String>,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let filter = match parse_filter(q.filter.as_deref()) {
        Ok(f) => f,
        Err(scim_err) => return scim_err_response(scim_err),
    };

    let start_index = q.start_index.unwrap_or(1).max(1);
    let count = q.count.unwrap_or(DEFAULT_COUNT).min(MAX_COUNT);

    // Fetch the workspace member list. The members hold (user_id,
    // role) — we then fetch each User row to populate the SCIM
    // resource. For a workspace with thousands of members this is
    // an N+1 against DDB; v1 accepts that cost (typical workspaces
    // are <500 members) and a future piece could swap in a BatchGet.
    let members = match state.workspace_repo.list_members(&ws_id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, ws_id, "SCIM list_users: workspace_repo failed");
            return scim_storage_err();
        }
    };

    // Batch-hydrate every member's User row in one chunked BatchGetItem
    // rather than an N+1 `get_by_id` loop (#38). A membership row whose
    // User is missing is simply absent from the map and skipped below,
    // preserving the previous per-member skip behavior.
    let member_ids: Vec<String> = members.iter().map(|m| m.user_id.clone()).collect();
    let users = match state.user_repo.get_by_ids(&member_ids).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, ws_id, "SCIM list_users: batch user fetch failed");
            return scim_storage_err();
        }
    };

    let mut resources: Vec<ScimUser> = Vec::new();
    for m in &members {
        let Some(user) = users.get(&m.user_id) else {
            continue; // membership row references a missing User; skip
        };
        // Apply filter post-fetch. A future GSI-backed lookup could
        // skip the membership scan for the eq-on-externalId path,
        // but the membership constraint always applies — a user
        // matching the filter who is NOT in this workspace must NOT
        // appear in the response.
        if let Some(ref f) = filter {
            let included = match f {
                SupportedFilter::UserNameEq(v) => user.email == *v,
                SupportedFilter::ExternalIdEq(v) => {
                    user.external_id.as_deref() == Some(v.as_str())
                }
            };
            if !included {
                continue;
            }
        }
        let location = user_location(&state, &ws_id, &user.user_id);
        resources.push(user_to_scim(user, Some(location)));
    }

    let total = resources.len();
    // SCIM `startIndex` is 1-based. Slice the deterministic
    // member-list output to the requested window.
    let from = start_index.saturating_sub(1).min(total);
    let to = (from + count).min(total);
    let page: Vec<ScimUser> = resources[from..to].to_vec();
    audit_scim(&state, &scim, ScimOp::UsersList);
    Json(ListResponse::new(page, total, start_index)).into_response()
}

/// `GET /scim/v2/workspaces/:ws/Users/:id`
async fn get_user(
    State(state): State<AppState>,
    Path((ws_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_workspace_member(&state, &ws_id, &user_id).await {
        return resp;
    }
    let user = match state.user_repo.get_by_id(&user_id).await {
        Ok(Some(u)) => u,
        Ok(None) => return scim_err_response(scim_not_found(&user_id)),
        Err(e) => {
            tracing::warn!(error = %e, user_id, "SCIM get_user: user fetch failed");
            return scim_storage_err();
        }
    };
    let location = user_location(&state, &ws_id, &user.user_id);
    audit_scim(&state, &scim, ScimOp::UsersGet);
    Json(user_to_scim(&user, Some(location))).into_response()
}

/// `POST /scim/v2/workspaces/:ws/Users`
///
/// JIT user provisioning. The SCIM `externalId` is the dedupe key
/// via the User row's `external_id` GSI. The created user is
/// auto-added as a workspace member with role `Member`.
async fn create_user(
    State(state): State<AppState>,
    Path(ws_id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<ScimUser>,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    // RFC 7643 §4.1: userName is required.
    if req.user_name.trim().is_empty() {
        return scim_err_response(ScimError::new(
            400,
            Some(scim_type::INVALID_VALUE),
            "userName is required",
        ));
    }

    // M-E8 gap-003: length-bound every IdP-supplied string before
    // we either compose them into a row or pass them downstream.
    let user_name = match validate_scim_string("userName", &req.user_name) {
        Ok(s) => s,
        Err(e) => return scim_err_response(e),
    };
    let external_id_raw = req.external_id.as_deref().unwrap_or("").trim().to_string();
    if external_id_raw.is_empty() {
        return scim_err_response(ScimError::new(
            400,
            Some(scim_type::INVALID_VALUE),
            "externalId is required for SCIM provisioning",
        ));
    }
    let external_id = match validate_scim_string("externalId", &external_id_raw) {
        Ok(s) => s,
        Err(e) => return scim_err_response(e),
    };
    // Validate every name-bearing field BEFORE composing the
    // display name so a malicious 10 KB givenName can't bypass the
    // cap by being concatenated with a sane familyName.
    let display_name_validated = match req.display_name.as_deref() {
        Some(s) => match validate_scim_string("displayName", s) {
            Ok(v) => Some(v),
            Err(e) => return scim_err_response(e),
        },
        None => None,
    };
    let name_formatted_validated = match req.name.as_ref().and_then(|n| n.formatted.as_deref()) {
        Some(s) => match validate_scim_string("name.formatted", s) {
            Ok(v) => Some(v),
            Err(e) => return scim_err_response(e),
        },
        None => None,
    };
    let given_validated = match req.name.as_ref().and_then(|n| n.given_name.as_deref()) {
        Some(s) => match validate_scim_string("name.givenName", s) {
            Ok(v) => Some(v),
            Err(e) => return scim_err_response(e),
        },
        None => None,
    };
    let family_validated = match req.name.as_ref().and_then(|n| n.family_name.as_deref()) {
        Some(s) => match validate_scim_string("name.familyName", s) {
            Ok(v) => Some(v),
            Err(e) => return scim_err_response(e),
        },
        None => None,
    };

    // Display name → User.name. Fall back to userName if missing.
    let name = display_name_validated
        .clone()
        .or_else(|| {
            name_formatted_validated.clone().or_else(|| {
                match (given_validated.as_deref(), family_validated.as_deref()) {
                    (Some(g), Some(f)) => Some(format!("{g} {f}")),
                    (Some(g), None) => Some(g.to_string()),
                    (None, Some(f)) => Some(f.to_string()),
                    (None, None) => None,
                }
            })
        })
        .unwrap_or_else(|| user_name.clone());
    // The concatenated `Given Family` may exceed `SCIM_TRUNCATE_BYTES`
    // even when each part is below. Final pass on the composed value
    // keeps the row size consistent.
    let name = match validate_scim_string("displayName", &name) {
        Ok(s) => s,
        Err(e) => return scim_err_response(e),
    };

    // JIT via the shared auth helper.
    let mut user = match ogrenotes_auth::user::find_or_create_scim_user(
        &state.user_repo,
        &state.folder_repo,
        &state.workspace_repo,
        &external_id,
        &user_name,
        &name,
    )
    .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, ws_id, "SCIM create_user: JIT failed");
            // Email-collision against a different-provider account
            // is the most-common cause; map to `uniqueness` so the
            // IdP marks the row failed-with-cause.
            return scim_err_response(ScimError::new(
                409,
                Some(scim_type::UNIQUENESS),
                e.to_string(),
            ));
        }
    };

    // SCIM `active=false` on create = "provisioned but disabled".
    // Apply before workspace-add so a deprovisioned-on-create user
    // is never even briefly active.
    if req.active == Some(false) && !user.is_disabled {
        if let Err(e) = state.user_repo.set_disabled(&user.user_id, true).await {
            tracing::warn!(error = %e, user_id = %user.user_id, "SCIM create_user: set_disabled failed");
            return scim_storage_err();
        }
        user.is_disabled = true;
    }

    // Add to workspace. If they're already a member (re-POST
    // pattern Okta uses to confirm a row), this no-ops via the
    // upsert semantics of put_member.
    let now = ogrenotes_common::time::now_usec();
    let member = WorkspaceMember {
        workspace_id: ws_id.clone(),
        user_id: user.user_id.clone(),
        role: WorkspaceRole::Member,
        joined_at: now,
    };
    if let Err(e) = state.workspace_repo.add_member(&member).await {
        tracing::warn!(error = %e, ws_id, user_id = %user.user_id, "SCIM create_user: add_member failed");
        return scim_storage_err();
    }

    let location = user_location(&state, &ws_id, &user.user_id);
    let body = Json(user_to_scim(&user, Some(location.clone())));
    let mut response = (StatusCode::CREATED, body).into_response();
    // RFC 7644 §3.3 — Location header on a POST 201.
    if let Ok(hv) = location.parse() {
        response.headers_mut().insert(axum::http::header::LOCATION, hv);
    }
    audit_scim(&state, &scim, ScimOp::UsersCreate);
    response
}

/// `PUT /scim/v2/workspaces/:ws/Users/:id`
///
/// Full-replace semantics. The body becomes the new state of the
/// resource. v1 lets the SCIM client adjust `displayName`,
/// `name.*`, and `active`; mutability of `userName` (the email)
/// would require a re-bind of the User row and is rejected.
async fn replace_user(
    State(state): State<AppState>,
    Path((ws_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(req): Json<ScimUser>,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_workspace_member(&state, &ws_id, &user_id).await {
        return resp;
    }
    let Ok(Some(mut user)) = state.user_repo.get_by_id(&user_id).await else {
        return scim_err_response(scim_not_found(&user_id));
    };
    // M-E8 gap-004: capture the pre-mutation disabled state so we
    // can emit a per-user SecurityAudit row iff this PUT is the
    // call that actually flipped active→disabled.
    let was_disabled = user.is_disabled;

    // userName change is treated as immutable. RFC 7643 §7.5
    // permits `mutability=immutable` semantics and SCIM error
    // `scimType=mutability` is the precise reply.
    if req.user_name != user.email {
        return scim_err_response(ScimError::new(
            400,
            Some(scim_type::MUTABILITY),
            "userName cannot be changed via SCIM in v1",
        ));
    }

    // Apply display-name via update_name so the M-E8 gap-003
    // length validation applies on this path too. PUT can carry
    // either displayName or name.formatted; both are SCIM-supplied
    // and must pass the cap. The fallback to user.name when neither
    // is present preserves the existing "no-name-change" idempotency
    // (update_name short-circuits when the new value matches).
    let proposed_name = req
        .display_name
        .clone()
        .or_else(|| req.name.as_ref().and_then(|n| n.formatted.clone()))
        .unwrap_or_else(|| user.name.clone());
    if let Err(resp) = update_name(&state, &mut user, &proposed_name).await {
        return resp;
    }
    if let Err(e) = apply_active(&state, &mut user, req.active).await {
        return e;
    }
    emit_scim_deprovision_audit_if_flipped(&state, &user.user_id, &ws_id, was_disabled, user.is_disabled);
    let location = user_location(&state, &ws_id, &user.user_id);
    audit_scim(&state, &scim, ScimOp::UsersReplace);
    Json(user_to_scim(&user, Some(location))).into_response()
}

/// M-E8 gap-004 helper: emit a per-user SecurityAudit row when a
/// SCIM mutation flips a user from active→disabled. The
/// `audit_scim` workspace-keyed row is operational forensics for
/// the SCIM token's actions; this row is the per-user view that
/// /admin/audit?target=<user> can surface.
///
/// Idempotent re-disable (already-disabled user) is intentionally
/// silent — auditing every no-op PATCH would flood the audit log
/// when an IdP runs hourly reconciliation against an already-
/// disabled user.
fn emit_scim_deprovision_audit_if_flipped(
    state: &AppState,
    user_id: &str,
    workspace_id: &str,
    was_disabled: bool,
    now_disabled: bool,
) {
    if !was_disabled && now_disabled {
        crate::routes::audit::record_security_event_by_actor(
            state,
            user_id,
            workspace_id,
            ogrenotes_storage::models::security_audit::SecurityAuditAction::SessionRevoked {
                reason: SCIM_DEPROVISION_REASON.to_string(),
            },
        );
    }
}

/// `PATCH /scim/v2/workspaces/:ws/Users/:id`
///
/// Apply a SCIM PatchOp. v1 supports the operations real
/// provisioners send:
///   - `replace { active: bool }`  — Okta/Entra deprovision path
///   - `replace { displayName: "..." }`
///   - `replace { name.givenName/familyName }` (compound name)
///   - `replace` with `path` = `active` / `displayName`
///
/// Unsupported operations return 400 with
/// `scimType=invalidValue`; unsupported `path` selectors return
/// `scimType=invalidPath`.
async fn patch_user(
    State(state): State<AppState>,
    Path((ws_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(req): Json<PatchOp>,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_workspace_member(&state, &ws_id, &user_id).await {
        return resp;
    }
    let Ok(Some(mut user)) = state.user_repo.get_by_id(&user_id).await else {
        return scim_err_response(scim_not_found(&user_id));
    };
    // M-E8 gap-004: capture the pre-mutation disabled state so the
    // post-loop audit-emit can detect an active→disabled flip
    // regardless of which PATCH op did it (replace+active, untargeted
    // replace, etc.).
    let was_disabled = user.is_disabled;

    for op in &req.operations {
        match op.op {
            PatchVerb::Replace | PatchVerb::Add => {
                if let Err(e) = apply_replace_or_add(&state, &mut user, op).await {
                    return e;
                }
            }
            PatchVerb::Remove => {
                if let Err(e) = apply_remove(&state, &mut user, op).await {
                    return e;
                }
            }
        }
    }

    emit_scim_deprovision_audit_if_flipped(&state, &user.user_id, &ws_id, was_disabled, user.is_disabled);
    let location = user_location(&state, &ws_id, &user.user_id);
    audit_scim(&state, &scim, ScimOp::UsersPatch);
    Json(user_to_scim(&user, Some(location))).into_response()
}

/// Apply a `replace` or `add` operation. Both shapes are accepted:
///   - Untargeted: `{ op: "replace", value: { active: false } }`
///     — value is a partial ScimUser; merge each field present.
///   - Path-targeted: `{ op: "replace", path: "active", value: false }`
///     — value is the new value for the specified attribute.
async fn apply_replace_or_add(
    state: &AppState,
    user: &mut ogrenotes_storage::models::user::User,
    op: &crate::scim::dtos::PatchOperation,
) -> Result<(), Response> {
    use serde_json::Value;
    let Some(value) = op.value.as_ref() else {
        return Err(scim_err_response(ScimError::new(
            400,
            Some(scim_type::INVALID_VALUE),
            "replace/add operation requires a value",
        )));
    };

    match op.path.as_deref() {
        // Untargeted: value is a partial ScimUser object.
        None => {
            let obj = value.as_object().ok_or_else(|| {
                scim_err_response(ScimError::new(
                    400,
                    Some(scim_type::INVALID_VALUE),
                    "untargeted replace value must be a JSON object",
                ))
            })?;
            if let Some(active) = obj.get("active").and_then(Value::as_bool) {
                apply_active(state, user, Some(active)).await?;
            }
            if let Some(dn) = obj.get("displayName").and_then(Value::as_str) {
                update_name(state, user, dn).await?;
            }
            if let Some(name_obj) = obj.get("name").and_then(Value::as_object) {
                if let Some(s) = name_obj.get("formatted").and_then(Value::as_str) {
                    update_name(state, user, s).await?;
                }
            }
            Ok(())
        }
        Some("active") => {
            let Some(b) = value.as_bool() else {
                return Err(scim_err_response(ScimError::new(
                    400,
                    Some(scim_type::INVALID_VALUE),
                    "active value must be a boolean",
                )));
            };
            apply_active(state, user, Some(b)).await

        }
        Some("displayName") | Some("name.formatted") => {
            let Some(s) = value.as_str() else {
                return Err(scim_err_response(ScimError::new(
                    400,
                    Some(scim_type::INVALID_VALUE),
                    "displayName value must be a string",
                )));
            };
            update_name(state, user, s).await

        }
        Some(other) => Err(scim_err_response(ScimError::new(
            400,
            Some(scim_type::INVALID_PATH),
            format!("unsupported SCIM path `{other}`"),
        ))),
    }
}

/// Apply a `remove` op. v1 supports removing `active` (re-enables
/// the user) only as a path-targeted op; untargeted remove is
/// rejected as invalid.
async fn apply_remove(
    state: &AppState,
    user: &mut ogrenotes_storage::models::user::User,
    op: &crate::scim::dtos::PatchOperation,
) -> Result<(), Response> {
    match op.path.as_deref() {
        Some("active") => apply_active(state, user, Some(true)).await,
        Some(other) => Err(scim_err_response(ScimError::new(
            400,
            Some(scim_type::INVALID_PATH),
            format!("remove not supported for path `{other}`"),
        ))),
        None => Err(scim_err_response(ScimError::new(
            400,
            Some(scim_type::NO_TARGET),
            "remove requires a path",
        ))),
    }
}

/// `DELETE /scim/v2/workspaces/:ws/Users/:id`
///
/// Soft-disable the global User row. Per the M-E5 plan: "does NOT
/// hard-delete (audit trail intact)". Returns 204 with no body on
/// success — SCIM convention.
async fn deprovision_user(
    State(state): State<AppState>,
    Path((ws_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_workspace_member(&state, &ws_id, &user_id).await {
        return resp;
    }
    let Ok(Some(existing)) = state.user_repo.get_by_id(&user_id).await else {
        return scim_err_response(scim_not_found(&user_id));
    };
    let was_disabled = existing.is_disabled;
    if let Err(e) = state.user_repo.set_disabled(&user_id, true).await {
        tracing::warn!(error = %e, user_id, "SCIM DELETE: set_disabled failed");
        return scim_storage_err();
    }
    audit_scim(&state, &scim, ScimOp::UsersDelete);
    // M-E8 gap-004: per-user audit row keyed on the affected user's
    // PK so /admin/audit?target=<user> surfaces the SCIM-driven
    // disable event. The workspace_id is the closest identifier we
    // have to an "actor" for a SCIM-token-driven mutation — no
    // individual user took this action. Only emitted when the
    // disable actually flipped (idempotent re-DELETEs don't append).
    if !was_disabled {
        crate::routes::audit::record_security_event_by_actor(
            &state,
            &user_id,
            &ws_id,
            ogrenotes_storage::models::security_audit::SecurityAuditAction::SessionRevoked {
                reason: SCIM_DEPROVISION_REASON.to_string(),
            },
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Reason tag attached to SecurityAudit::SessionRevoked rows that
/// originate from a SCIM-driven disable (DELETE or PATCH active=false).
/// Stable so /admin/audit consumers and downstream alerting can
/// distinguish SCIM-driven deprovisioning from interactive admin
/// disables (which already write their own AdminAudit row).
const SCIM_DEPROVISION_REASON: &str = "scim_deprovision";

// ─── Groups (Phase 4 M-E5 piece E) ──────────────────────────────
//
// One Group per workspace in v1: the group_id MUST equal the URL's
// workspace_id. Listing returns exactly one Resource (the
// workspace itself); fetching a Group with a different id returns
// 404 — there is no cross-workspace Group lookup.

/// `GET /scim/v2/workspaces/:ws/Groups`
///
/// Returns a ListResponse with a single Group describing the
/// caller's workspace. RFC 7644 requires the envelope even when
/// the result set is one row.
async fn list_groups(
    State(state): State<AppState>,
    Path(ws_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match build_group(&state, &ws_id).await {
        Ok(group) => {
            audit_scim(&state, &scim, ScimOp::GroupsList);
            Json(ListResponse::new(vec![group], 1, 1)).into_response()
        }
        Err(resp) => resp,
    }
}

/// `GET /scim/v2/workspaces/:ws/Groups/:group_id`
async fn get_group(
    State(state): State<AppState>,
    Path((ws_id, group_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if group_id != ws_id {
        // Cross-workspace Group lookup is forbidden by design;
        // the only Group visible to a workspace's SCIM token is
        // that workspace itself.
        return scim_err_response(scim_group_not_found(&group_id));
    }
    match build_group(&state, &ws_id).await {
        Ok(group) => {
            audit_scim(&state, &scim, ScimOp::GroupsGet);
            Json(group).into_response()
        }
        Err(resp) => resp,
    }
}

/// `PATCH /scim/v2/workspaces/:ws/Groups/:group_id`
///
/// v1 supports member add and remove. Real provisioners send these
/// patterns:
///
///   `add` (untargeted, value is { members: [...] }):
///       Okta's "add member" call.
///
///   `add` path="members", value=[{value: "user-id"}]:
///       Targeted form some IdPs send instead.
///
///   `remove` path="members", value=[{value: "user-id"}]:
///       Okta's "remove member" call when it provides the list.
///
/// The filter-path form `members[value eq "x"]` is NOT supported
/// in v1 — it returns `invalidPath`. If a real IdP emits that
/// form, expanding the filter parser is a one-spot follow-up.
async fn patch_group(
    State(state): State<AppState>,
    Path((ws_id, group_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(req): Json<PatchOp>,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if group_id != ws_id {
        return scim_err_response(scim_group_not_found(&group_id));
    }

    for op in &req.operations {
        if let Err(resp) = apply_group_op(&state, &ws_id, op).await {
            return resp;
        }
    }

    match build_group(&state, &ws_id).await {
        Ok(group) => {
            audit_scim(&state, &scim, ScimOp::GroupsPatch);
            Json(group).into_response()
        }
        Err(resp) => resp,
    }
}

/// Apply a single PATCH operation against the workspace's
/// membership list. Each branch validates only the shapes piece D
/// promised — anything else is `invalidPath` or `invalidValue`.
async fn apply_group_op(
    state: &AppState,
    ws_id: &str,
    op: &crate::scim::dtos::PatchOperation,
) -> Result<(), Response> {
    use serde_json::Value;

    // Extract the list of member user_ids from either the
    // untargeted shape (`value.members = [...]`) or the targeted
    // shape (`path = "members"`, `value = [...]`).
    let member_ids: Vec<String> = match (op.path.as_deref(), op.value.as_ref()) {
        (None, Some(value)) => {
            // Untargeted: value is a partial Group { members: [...] }.
            let arr = value
                .get("members")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    scim_err_response(ScimError::new(
                        400,
                        Some(scim_type::INVALID_VALUE),
                        "untargeted Group op must have `value.members` array",
                    ))
                })?;
            collect_member_values(arr)?
        }
        (Some("members"), Some(value)) => {
            let arr = value.as_array().ok_or_else(|| {
                scim_err_response(ScimError::new(
                    400,
                    Some(scim_type::INVALID_VALUE),
                    "path=members requires a value array",
                ))
            })?;
            collect_member_values(arr)?
        }
        (Some(other), _) => {
            return Err(scim_err_response(ScimError::new(
                400,
                Some(scim_type::INVALID_PATH),
                format!("unsupported Group path `{other}` (v1 supports `members` only)"),
            )));
        }
        (None, None) => {
            return Err(scim_err_response(ScimError::new(
                400,
                Some(scim_type::INVALID_VALUE),
                "Group op requires a value",
            )));
        }
    };

    if member_ids.is_empty() {
        return Err(scim_err_response(ScimError::new(
            400,
            Some(scim_type::INVALID_VALUE),
            "Group op `members` list cannot be empty",
        )));
    }

    let now = ogrenotes_common::time::now_usec();
    match op.op {
        PatchVerb::Add => {
            // Union: add each new uid (idempotent via add_member
            // upsert). Existing members not in the list stay.
            for uid in &member_ids {
                verify_member_user_exists(state, uid).await?;
                add_member_row(state, ws_id, uid, now).await?;
            }
        }
        PatchVerb::Replace => {
            // Reconciliation: the new list IS the complete
            // membership. Compute (current ∖ new) and remove those;
            // (new ∖ current) and add those. RFC 7644 §3.5.2 is
            // explicit that `replace` on a multi-valued attribute
            // sets the value, not appends. Okta's "reconciliation
            // sync" mode relies on this semantic.
            let current = state
                .workspace_repo
                .list_members(ws_id)
                .await
                .map_err(|e| {
                    tracing::warn!(error = %e, ws_id, "SCIM patch_group replace: list_members failed");
                    scim_storage_err()
                })?;
            // Verify every new uid exists BEFORE we start mutating.
            // If a typo is mid-list, half-applying the change would
            // be much worse than rejecting the whole op.
            for uid in &member_ids {
                verify_member_user_exists(state, uid).await?;
            }
            use std::collections::HashSet;
            let new_set: HashSet<&str> = member_ids.iter().map(String::as_str).collect();
            for row in &current {
                if !new_set.contains(row.user_id.as_str()) {
                    if let Err(e) = state.workspace_repo.remove_member(ws_id, &row.user_id).await {
                        tracing::warn!(error = %e, ws_id, user_id = %row.user_id, "SCIM patch_group replace: remove_member failed");
                        return Err(scim_storage_err());
                    }
                }
            }
            let current_set: HashSet<&str> =
                current.iter().map(|r| r.user_id.as_str()).collect();
            for uid in &member_ids {
                if !current_set.contains(uid.as_str()) {
                    add_member_row(state, ws_id, uid, now).await?;
                }
            }
        }
        PatchVerb::Remove => {
            for uid in &member_ids {
                if let Err(e) = state.workspace_repo.remove_member(ws_id, uid).await {
                    tracing::warn!(error = %e, ws_id, user_id = %uid, "SCIM patch_group: remove_member failed");
                    return Err(scim_storage_err());
                }
            }
        }
    }
    Ok(())
}

/// Refuse to add a user_id that doesn't exist — without this guard
/// a typo in the IdP would silently create a "ghost" membership row
/// pointing at no User.
async fn verify_member_user_exists(state: &AppState, uid: &str) -> Result<(), Response> {
    match state.user_repo.get_by_id(uid).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(scim_err_response(ScimError::new(
            400,
            Some(scim_type::INVALID_VALUE),
            format!("member `{uid}` does not exist"),
        ))),
        Err(e) => {
            tracing::warn!(error = %e, user_id = %uid, "SCIM patch_group: user lookup failed");
            Err(scim_storage_err())
        }
    }
}

/// Upsert a workspace membership row for `uid`. Caller has already
/// verified the user exists.
async fn add_member_row(
    state: &AppState,
    ws_id: &str,
    uid: &str,
    now: i64,
) -> Result<(), Response> {
    let member = WorkspaceMember {
        workspace_id: ws_id.to_string(),
        user_id: uid.to_string(),
        role: WorkspaceRole::Member,
        joined_at: now,
    };
    state.workspace_repo.add_member(&member).await.map_err(|e| {
        tracing::warn!(error = %e, ws_id, user_id = %uid, "SCIM patch_group: add_member failed");
        scim_storage_err()
    })
}

/// Pull `{value: "..."}` user_ids out of a `members` array. Every
/// entry MUST have a string `value`; an entry missing it is rejected
/// rather than silently dropped, because silent drop turns an IdP
/// typo into invisible partial-success — the opposite of the
/// ghost-member-guard intent below.
fn collect_member_values(arr: &[serde_json::Value]) -> Result<Vec<String>, Response> {
    arr.iter()
        .map(|m| {
            m.get("value")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    scim_err_response(ScimError::new(
                        400,
                        Some(scim_type::INVALID_VALUE),
                        "each members entry must have a string `value` field",
                    ))
                })
        })
        .collect()
}

/// Build the Group resource for `ws_id` — workspace metadata
/// becomes the Group's `id`/`displayName`/`meta`, and the
/// workspace's members populate the `members` array.
async fn build_group(
    state: &AppState,
    ws_id: &str,
) -> Result<ScimGroup, Response> {
    let workspace = match state.workspace_repo.get(ws_id).await {
        Ok(Some(w)) => w,
        Ok(None) => return Err(scim_err_response(scim_group_not_found(ws_id))),
        Err(e) => {
            tracing::warn!(error = %e, ws_id, "SCIM build_group: workspace fetch failed");
            return Err(scim_storage_err());
        }
    };

    let members_rows = match state.workspace_repo.list_members(ws_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, ws_id, "SCIM build_group: list_members failed");
            return Err(scim_storage_err());
        }
    };

    let mut members: Vec<GroupMember> = Vec::with_capacity(members_rows.len());
    for row in &members_rows {
        let display = state
            .user_repo
            .get_by_id(&row.user_id)
            .await
            .ok()
            .flatten()
            .map(|u| u.name);
        members.push(GroupMember {
            value: row.user_id.clone(),
            display,
            type_: Some("User".to_string()),
            ref_: Some(format!(
                "{}/api/v1/scim/v2/workspaces/{ws_id}/Users/{uid}",
                state.config.frontend_origin,
                uid = row.user_id,
            )),
        });
    }

    let location = format!(
        "{}/api/v1/scim/v2/workspaces/{ws_id}/Groups/{ws_id}",
        state.config.frontend_origin,
    );
    Ok(ScimGroup {
        id: Some(workspace.workspace_id.clone()),
        external_id: None,
        display_name: workspace.name.clone(),
        members,
        meta: Some(crate::scim::dtos::Meta {
            resource_type: Some("Group".to_string()),
            created: None,
            last_modified: None,
            location: Some(location),
            version: None,
        }),
        schemas: vec![SCHEMA_GROUP.to_string()],
    })
}

fn scim_group_not_found(group_id: &str) -> ScimError {
    ScimError::new(404, None, format!("group `{group_id}` not found"))
}

// ─── Discovery endpoints (RFC 7643 §5–§7) ───────────────────────

/// `GET /scim/v2/workspaces/:ws/ServiceProviderConfig`
async fn get_service_provider_config(
    State(state): State<AppState>,
    Path(ws_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    audit_scim(&state, &scim, ScimOp::DiscoveryServiceProviderConfig);
    Json(discovery::service_provider_config(&scim_base_url(&state, &ws_id))).into_response()
}

/// `GET /scim/v2/workspaces/:ws/ResourceTypes`
async fn get_resource_types(
    State(state): State<AppState>,
    Path(ws_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    audit_scim(&state, &scim, ScimOp::DiscoveryResourceTypes);
    Json(discovery::resource_types(&scim_base_url(&state, &ws_id))).into_response()
}

/// `GET /scim/v2/workspaces/:ws/Schemas`
async fn get_schemas(
    State(state): State<AppState>,
    Path(ws_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let scim = match scim_auth(&state, &headers, &ws_id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    audit_scim(&state, &scim, ScimOp::DiscoverySchemas);
    Json(discovery::schemas(&scim_base_url(&state, &ws_id))).into_response()
}

fn scim_base_url(state: &AppState, ws_id: &str) -> String {
    format!(
        "{}/api/v1/scim/v2/workspaces/{ws_id}",
        state.config.frontend_origin
    )
}

// ─── Audit (Phase 4 M-E5 piece F) ───────────────────────────────

/// Closed set of SCIM operation labels written to the SecurityAudit
/// `op` field. Typed (not a `&str`) so the twelve call sites can't
/// drift independently and a typo at any one site is a compile
/// error rather than a silently-uncorrelatable audit row.
#[derive(Copy, Clone)]
enum ScimOp {
    UsersList,
    UsersGet,
    UsersCreate,
    UsersReplace,
    UsersPatch,
    UsersDelete,
    GroupsList,
    GroupsGet,
    GroupsPatch,
    DiscoveryServiceProviderConfig,
    DiscoveryResourceTypes,
    DiscoverySchemas,
}

impl ScimOp {
    fn as_str(self) -> &'static str {
        match self {
            Self::UsersList => "users.list",
            Self::UsersGet => "users.get",
            Self::UsersCreate => "users.create",
            Self::UsersReplace => "users.replace",
            Self::UsersPatch => "users.patch",
            Self::UsersDelete => "users.delete",
            Self::GroupsList => "groups.list",
            Self::GroupsGet => "groups.get",
            Self::GroupsPatch => "groups.patch",
            Self::DiscoveryServiceProviderConfig => "discovery.serviceProviderConfig",
            Self::DiscoveryResourceTypes => "discovery.resourceTypes",
            Self::DiscoverySchemas => "discovery.schemas",
        }
    }
}

/// Audit-log a successfully authenticated SCIM request. Fires
/// fire-and-forget via `record_security_event` so the audit write
/// never adds latency to the SCIM response. Attributed to the
/// workspace_id (used as the SecurityAudit row's user_id PK so
/// admins can query "all SCIM activity for workspace X" with one
/// PK lookup); the action body carries token_id + op for filtering.
fn audit_scim(state: &AppState, scim: &ScimAuth, op: ScimOp) {
    record_security_event(
        state,
        &scim.workspace_id,
        SecurityAuditAction::ScimTokenUsed {
            token_id: scim.token_id.clone(),
            op: op.as_str().to_string(),
        },
    );
}

/// Authenticate a SCIM request and return the verified principal,
/// converting any error into the SCIM-shaped Response shape every
/// route returns. Centralizes the per-handler boilerplate.
async fn scim_auth(
    state: &AppState,
    headers: &HeaderMap,
    ws_id: &str,
) -> Result<ScimAuth, Response> {
    verify_scim_request(state, headers, ws_id)
        .await
        .map_err(scim_err_for_api_error)
}

// ─── Shared helpers ─────────────────────────────────────────────

/// Workspace-scope authorization gate. Every read AND write handler
/// calls this before touching the user row — the SCIM bearer's
/// workspace_id is enforced via `verify_scim_request`, but without
/// this gate a workspace-A token could mutate or disable a user who
/// belongs to workspace B. Returns `Err(404 response)` if the user
/// isn't a member of the URL's workspace, matching the per-RFC
/// "not visible in this scope" semantics.
async fn require_workspace_member(
    state: &AppState,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), Response> {
    match state
        .workspace_repo
        .get_member(workspace_id, user_id)
        .await
    {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(scim_err_response(scim_not_found(user_id))),
        Err(e) => {
            tracing::warn!(
                error = %e,
                workspace_id,
                user_id,
                "SCIM: workspace-member gate lookup failed"
            );
            Err(scim_storage_err())
        }
    }
}

/// Apply an `active` change to a user (idempotent — no DDB write if
/// already in the target state).
async fn apply_active(
    state: &AppState,
    user: &mut ogrenotes_storage::models::user::User,
    active: Option<bool>,
) -> Result<(), Response> {
    let Some(want_active) = active else {
        return Ok(());
    };
    let want_disabled = !want_active;
    if user.is_disabled == want_disabled {
        return Ok(());
    }
    state
        .user_repo
        .set_disabled(&user.user_id, want_disabled)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, user_id = %user.user_id, "SCIM apply_active: set_disabled failed");
            scim_storage_err()
        })?;
    user.is_disabled = want_disabled;
    Ok(())
}

/// Update User.name. Idempotent — no DDB write if already equal.
///
/// M-E8 gap-003 chokepoint: every SCIM-driven name write flows
/// through here, and every call validates the IdP-supplied string
/// against the per-field length policy. A misconfigured IdP that
/// pushes a 10 KB displayName fails closed with a 400 rather than
/// quietly bloating the DDB row.
async fn update_name(
    state: &AppState,
    user: &mut ogrenotes_storage::models::user::User,
    new_name: &str,
) -> Result<(), Response> {
    let validated = validate_scim_string("displayName", new_name).map_err(scim_err_response)?;
    if user.name == validated {
        return Ok(());
    }
    let now = ogrenotes_common::time::now_usec();
    state
        .user_repo
        .update(&user.user_id, Some(&validated), None, now)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, user_id = %user.user_id, "SCIM update_name: update failed");
            scim_storage_err()
        })?;
    user.name = validated;
    Ok(())
}

/// Hard-reject threshold for SCIM input strings (M-E8 gap-003). Any
/// individual field above this is presumed to be a misconfigured
/// IdP rather than a real user value — surface as 400 so the admin
/// sees the issue instead of seeing silently-truncated names.
const SCIM_MAX_FIELD_BYTES: usize = 1024;

/// Truncate target for accepted SCIM input strings. Matches the
/// OAuth path's `MAX_NAME_LEN` so the two ingress paths produce
/// consistent row sizes downstream. 256 chars covers every real
/// human name and externalId we've seen.
const SCIM_TRUNCATE_BYTES: usize = 256;

/// Validate a SCIM-supplied string against the M-E8 gap-003 policy.
///
/// - Empty / whitespace-only strings are NOT rejected here — the
///   handler may have a stricter "required" check (e.g. `userName`
///   non-empty after trim); this function only bounds the byte
///   length.
/// - Strings longer than `SCIM_MAX_FIELD_BYTES` (1 KB) are rejected
///   with `400 invalidValue` and a message naming the field, so an
///   IdP admin sees which attribute is misconfigured.
/// - Strings between `SCIM_TRUNCATE_BYTES` (256) and `SCIM_MAX_FIELD_BYTES`
///   are silently truncated at a char boundary so a slightly-long
///   value (e.g. a 300-char "Full Legal Name" attribute) still
///   persists rather than failing the whole provision.
fn validate_scim_string(field: &str, value: &str) -> Result<String, ScimError> {
    if value.len() > SCIM_MAX_FIELD_BYTES {
        return Err(ScimError::new(
            400,
            Some(scim_type::INVALID_VALUE),
            format!(
                "{field} exceeds {SCIM_MAX_FIELD_BYTES} bytes (likely a misconfigured IdP attribute mapping)"
            ),
        ));
    }
    if value.len() <= SCIM_TRUNCATE_BYTES {
        return Ok(value.to_string());
    }
    // Truncate at a char boundary so we never produce invalid UTF-8.
    let mut end = SCIM_TRUNCATE_BYTES;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    Ok(value[..end].to_string())
}

fn user_location(state: &AppState, ws_id: &str, user_id: &str) -> String {
    format!(
        "{}/api/v1/scim/v2/workspaces/{ws_id}/Users/{user_id}",
        state.config.frontend_origin,
    )
}

fn scim_not_found(user_id: &str) -> ScimError {
    ScimError::new(404, None, format!("user `{user_id}` not found"))
}

/// 500 response for any storage-layer failure in a SCIM handler.
/// Centralizes the detail string (was previously duplicated ~14× with one
/// site drifted to "internal storage error") so the wire shape stays
/// consistent. The `tracing::warn!` describing the specific failure is left
/// at each call site.
fn scim_storage_err() -> Response {
    let err = ScimError::new(500, None, "storage");
    scim_err_response(err)
}

/// Wrap a SCIM error in an HTTP response. Status code is parsed
/// back from the SCIM body's stringified `status` field — the
/// stringification is RFC-mandated, so we have to live with it.
/// `expect` here rather than a silent 500 fallback: if the string
/// fails to parse we constructed the ScimError with an invalid
/// HTTP code, which is a programmer error that should crash loud
/// rather than send a 500 with a body whose own status disagrees.
fn scim_err_response(err: ScimError) -> Response {
    let status = err
        .status
        .parse::<u16>()
        .ok()
        .and_then(|n| StatusCode::from_u16(n).ok())
        .expect("ScimError constructed with invalid HTTP status code");
    (status, Json(err)).into_response()
}

/// Map an `ApiError` (the ScimAuth extractor's rejection shape) to
/// a SCIM-flavored error body. The extractor collapses everything
/// to `Unauthorized`, so this is a 401 in practice — but the
/// pattern is here for clarity.
fn scim_err_for_api_error(err: crate::error::ApiError) -> Response {
    use crate::error::ApiError;
    let scim = match err {
        ApiError::Unauthorized => ScimError::new(401, None, "unauthorized"),
        ApiError::Forbidden => ScimError::new(403, None, "forbidden"),
        ApiError::NotFound(_) => ScimError::new(404, None, "not found"),
        // M-E8 gap-005's `enforce("scim_request", ...)` inside
        // `verify_scim_request` returns `TooManyRequests`. Without
        // a dedicated branch it fell through to 500, which is what
        // the gap-005 integration tests caught once cargo's
        // --no-fail-fast was enabled. A future revision could attach
        // `Retry-After` via a header on the Response, but the SCIM
        // spec doesn't require it and the existing scim_err_response
        // doesn't expose a header path; bare 429 + JSON body is
        // honest for v1.
        ApiError::TooManyRequests { .. } => ScimError::new(429, None, "rate limit exceeded"),
        _ => ScimError::new(500, None, "internal"),
    };
    scim_err_response(scim)
}
