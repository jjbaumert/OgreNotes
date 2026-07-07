// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_common::metrics;
use ogrenotes_storage::models::admin_audit::{AdminAudit, AdminAuditAction};
use ogrenotes_storage::models::security_audit::SecurityAudit;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Per-user rate limit on admin mutation routes (Phase 4 M-E2).
///
/// Threat model: a stolen admin token replayed from an attacker-
/// controlled IP. Keying the bucket on `user_id` (not the IP) binds
/// the cap to the token identity, which is what we need — the
/// attacker's IP is unconstrained, but they only have one stolen
/// token. A confused-deputy XSS scenario inside a still-valid admin
/// browser session is also bounded by the same key, since both the
/// legitimate session and the injected calls share `user_id`.
///
/// Read-only admin routes (list_users, get_user, metrics, audit)
/// intentionally stay unthrottled so the operator UI stays
/// responsive during an incident.
async fn enforce_admin_mut_rate_limit(state: &AppState, user_id: &str) -> Result<(), ApiError> {
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "admin_mut",
        user_id,
        state.config.rate_limit_admin_mut_per_min,
        60,
    )
    .await
}

/// Persist + log one admin audit row. Spawned as a background task so an
/// audit-write failure cannot block the user-visible mutation that just
/// succeeded — the structured `tracing::warn!` line is the durable
/// fallback if DynamoDB is briefly unhealthy.
///
/// Every privileged admin handler funnels through this so the schema of
/// emitted events is consistent for downstream log shipping / forensic
/// queries. Closes #32.
fn record_admin_action(
    state: &AppState,
    actor_id: &str,
    target_user_id: &str,
    action: AdminAuditAction,
    detail: serde_json::Value,
) {
    let detail_str = detail.to_string();
    // Structured tracing event at INFO — successful admin actions are
    // expected events, not anomalies. Reserving WARN/ERROR for failure
    // paths (notably the DDB write below) keeps CloudWatch alerts on
    // higher levels meaningful.
    tracing::info!(
        event_type = "admin_action",
        action = action.as_str(),
        actor_id = %actor_id,
        target_user_id = %target_user_id,
        detail = %detail_str,
        "admin_action"
    );

    let audit = AdminAudit {
        audit_id: nanoid::nanoid!(16),
        target_user_id: target_user_id.to_string(),
        actor_id: actor_id.to_string(),
        action: action.clone(),
        detail: detail_str,
        created_at: ogrenotes_common::time::now_usec(),
    };
    let repo = state.admin_audit_repo.clone();
    let actor = actor_id.to_string();
    let target = target_user_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = repo.create(&audit).await {
            // The tracing event above is already written, so the audit
            // is preserved in logs; this just signals the durable copy
            // didn't land.
            tracing::error!(
                error = %e,
                actor_id = %actor,
                target_user_id = %target,
                action = action.as_str(),
                "admin_audit DDB write failed; tracing event is the only record"
            );
        }
    });
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{id}", get(get_user))
        .route("/users/{id}/disable", post(disable_user))
        .route("/users/{id}/enable", post(enable_user))
        .route("/users/{id}/promote", post(promote_user))
        .route("/users/{id}/demote", post(demote_user))
        .route(
            "/users/{id}/ask-policy",
            get(get_ask_policy).put(set_ask_policy),
        )
        .route("/metrics", get(metrics_snapshot))
        .route("/audit", get(list_audit))
        .route("/documents/{id}/compact", post(force_compact_document))
        .route(
            "/documents/{id}/repair-liveapp-attrs",
            post(repair_liveapp_attrs),
        )
        .route(
            "/documents/{id}/link-settings",
            patch(admin_update_link_settings),
        )
}

/// PATCH /admin/documents/:id/link-settings — admin override of a
/// document's link-sharing settings (§5.5). Global-admin only (reuses
/// the live-row `is_admin` check). Emits a `LinkSharingChanged`
/// SecurityAudit row with `actor = admin`, `subject = doc owner`, so the
/// override lands in the same link-change trail as the owner's own
/// changes (it is a sharing event, not recorded in AdminAudit).
async fn admin_update_link_settings(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<crate::routes::documents::UpdateLinkSettingsRequest>,
) -> Result<StatusCode, ApiError> {
    require_admin(&auth)?;
    enforce_admin_mut_rate_limit(&state, &auth.user_id).await?;

    // Resolve the doc to get the owner (audit subject) and reject
    // missing/trashed docs (parity with the owner endpoint's 404).
    let meta = state
        .doc_repo
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Document not found".to_string()))?;
    if meta.is_deleted {
        return Err(ApiError::NotFound("Document not found".to_string()));
    }

    crate::routes::documents::apply_link_settings(&state, &meta, &req, &auth.user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /admin/metrics — current in-process metrics snapshot. Admin-only.
async fn metrics_snapshot(
    auth: AuthUser,
) -> Result<Json<metrics::MetricsSnapshot>, ApiError> {
    require_admin(&auth)?;
    Ok(Json(metrics::snapshot()))
}

fn require_admin(user: &AuthUser) -> Result<(), ApiError> {
    if !user.is_admin {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminUserResponse {
    id: String,
    name: String,
    email: String,
    is_admin: bool,
    is_disabled: bool,
    last_active_at: i64,
    created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminUserListResponse {
    users: Vec<AdminUserResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListParams {
    limit: Option<i32>,
    cursor: Option<String>,
    /// Case-insensitive prefix match against User.email. Used by the
    /// admin console's user-search box. Filters the in-memory page
    /// AFTER pagination so a deep search still walks the table — the
    /// cost is bounded by the page size (default 50, cap 200).
    email_prefix: Option<String>,
}

/// GET /admin/users — list all users (paginated).
async fn list_users(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> Result<Json<AdminUserListResponse>, ApiError> {
    require_admin(&auth)?;

    let limit = params.limit.unwrap_or(50).min(200);
    let (users, next_cursor) = state
        .user_repo
        .list_all(limit, params.cursor.as_deref())
        .await?;

    // Case-insensitive prefix match. Applied AFTER the page is
    // fetched, so a thousand-user page filtered down to one is fine —
    // the bound on cost is the page size, not the email_prefix
    // selectivity. The frontend paginates by cursor and re-applies the
    // filter on each page; this keeps the server logic trivial.
    let email_prefix_lc = params
        .email_prefix
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());

    let users = users
        .into_iter()
        .filter(|u| match email_prefix_lc {
            Some(ref pfx) => u.email.to_lowercase().starts_with(pfx),
            None => true,
        })
        .map(|u| {
            let is_admin = u.is_admin();
            AdminUserResponse {
                id: u.user_id,
                name: u.name,
                email: u.email,
                is_admin,
                is_disabled: u.is_disabled,
                last_active_at: u.last_active_at,
                created_at: u.created_at,
            }
        })
        .collect();

    Ok(Json(AdminUserListResponse { users, next_cursor }))
}

/// GET /admin/users/:id — get a single user's details.
async fn get_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<AdminUserResponse>, ApiError> {
    require_admin(&auth)?;

    let user = state
        .user_repo
        .get_by_id(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    let is_admin = user.is_admin();
    Ok(Json(AdminUserResponse {
        id: user.user_id,
        name: user.name,
        email: user.email,
        is_admin,
        is_disabled: user.is_disabled,
        last_active_at: user.last_active_at,
        created_at: user.created_at,
    }))
}

/// POST /admin/users/:id/disable — disable a user account and revoke all sessions.
async fn disable_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&auth)?;
    enforce_admin_mut_rate_limit(&state, &auth.user_id).await?;

    if id == auth.user_id {
        return Err(ApiError::BadRequest(
            "Cannot disable your own account".to_string(),
        ));
    }

    // Verify user exists
    state
        .user_repo
        .get_by_id(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    state.user_repo.set_disabled(&id, true).await?;

    // Revoke all active sessions so the user is logged out immediately
    let _ = state.session_repo.delete_all_for_user(&id).await;

    record_admin_action(
        &state,
        &auth.user_id,
        &id,
        AdminAuditAction::Disable,
        serde_json::json!({}),
    );

    Ok(StatusCode::NO_CONTENT)
}

/// POST /admin/users/:id/enable — re-enable a disabled user account.
async fn enable_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&auth)?;
    enforce_admin_mut_rate_limit(&state, &auth.user_id).await?;

    state
        .user_repo
        .get_by_id(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    state.user_repo.set_disabled(&id, false).await?;

    record_admin_action(
        &state,
        &auth.user_id,
        &id,
        AdminAuditAction::Enable,
        serde_json::json!({}),
    );

    Ok(StatusCode::NO_CONTENT)
}

/// POST /admin/users/:id/promote — grant admin privileges.
async fn promote_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&auth)?;
    enforce_admin_mut_rate_limit(&state, &auth.user_id).await?;

    state
        .user_repo
        .get_by_id(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    state.user_repo.set_admin(&id, true).await?;

    record_admin_action(
        &state,
        &auth.user_id,
        &id,
        AdminAuditAction::Promote,
        serde_json::json!({}),
    );

    Ok(StatusCode::NO_CONTENT)
}

/// POST /admin/users/:id/demote — revoke admin privileges.
async fn demote_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&auth)?;
    enforce_admin_mut_rate_limit(&state, &auth.user_id).await?;

    if id == auth.user_id {
        return Err(ApiError::BadRequest(
            "Cannot demote yourself".to_string(),
        ));
    }

    state
        .user_repo
        .get_by_id(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    state.user_repo.set_admin(&id, false).await?;

    record_admin_action(
        &state,
        &auth.user_id,
        &id,
        AdminAuditAction::Demote,
        serde_json::json!({}),
    );

    Ok(StatusCode::NO_CONTENT)
}


#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AskPolicyResponse {
    user_id: String,
    policy: ogrenotes_storage::models::user::AskPolicy,
}

#[derive(Deserialize)]
struct SetAskPolicyRequest {
    policy: ogrenotes_storage::models::user::AskPolicy,
}

/// #148 — GET /admin/users/:id/ask-policy: read the per-user
/// `/api/v1/ask` policy. Admin-only.
async fn get_ask_policy(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<AskPolicyResponse>, ApiError> {
    require_admin(&auth)?;

    let user = state
        .user_repo
        .get_by_id(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    Ok(Json(AskPolicyResponse {
        policy: user.ask_policy(),
        user_id: user.user_id,
    }))
}

/// #148 — PUT /admin/users/:id/ask-policy: set the per-user
/// `/api/v1/ask` policy. Admin-only. The policy persists on the
/// User row so it survives restarts and scale-out. Defaults to
/// `Disabled` for new production users; admin must explicitly
/// opt them in to `SystemOnly` or `SystemOrByok`.
async fn set_ask_policy(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<SetAskPolicyRequest>,
) -> Result<StatusCode, ApiError> {
    require_admin(&auth)?;
    enforce_admin_mut_rate_limit(&state, &auth.user_id).await?;

    // Capture the prior policy so the audit row records both
    // the before and after state. Critical for forensics under
    // multiple flips — without `from`, two consecutive edits
    // are indistinguishable from a single one when reading any
    // individual audit row.
    let user = state
        .user_repo
        .get_by_id(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;
    let prev_policy = user.ask_policy();

    state.user_repo.set_ask_policy(&id, req.policy).await?;

    record_admin_action(
        &state,
        &auth.user_id,
        &id,
        AdminAuditAction::SetAskPolicy,
        serde_json::json!({ "from": prev_policy, "to": req.policy }),
    );

    Ok(StatusCode::NO_CONTENT)
}

// ─── Combined audit endpoint (Phase 4 M-E2) ────────────────────

/// Query parameters for `GET /admin/audit`. All fields are optional;
/// the empty query returns the most recent admin + security audit
/// rows merged. The frontend's audit-log viewer drives this.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditQuery {
    /// Filter to events caused by this actor.
    actor: Option<String>,
    /// Filter to events whose subject (target user / doc owner) is this
    /// user_id. Both AdminAudit (keyed on `target_user_id`) and
    /// SecurityAudit (keyed on `user_id`) honor this.
    target: Option<String>,
    /// Filter to a single event kind (admin action tag like `disable`,
    /// or security audit kind like `loginFailure`). Matched
    /// case-sensitively against the same string written to the storage
    /// `action` column.
    kind: Option<String>,
    /// Inclusive lower bound on `created_at` (microseconds since epoch).
    from: Option<i64>,
    /// Exclusive upper bound on `created_at` (microseconds since epoch).
    to: Option<i64>,
    /// Hard cap per source table. The endpoint fetches up to `limit`
    /// rows from EACH of `AdminAudit` and `SecurityAudit` independently
    /// (newest-first), merges them chronologically, and truncates the
    /// merged set to `limit` again. When the two tables have asymmetric
    /// densities in the time window the user is viewing, rows from the
    /// sparser table may be fully displaced — e.g. an account with
    /// heavy MFA activity and one rare disable will show the disable
    /// only if it's newer than every MFA row in the fetched page. A
    /// global "most recent N across all sources" guarantee requires a
    /// unified GSI (v2 carry-forward, #49). Default 50, max 200.
    limit: Option<usize>,
}

/// Discriminator on the merged audit row — which storage table the
/// entry was sourced from. Typed (rather than a `&'static str`) so a
/// future fifth source can't land as a stringly-typed "admins" typo
/// that compiles, serializes, and corrupts the frontend's source-tint
/// CSS. `rename_all = "lowercase"` keeps the wire shape identical to
/// the pre-typed `"admin"` / `"security"` strings.
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum AuditSource {
    Admin,
    Security,
}

/// One row in the combined audit response. The `source` discriminator
/// lets the frontend render a per-source icon without inspecting the
/// `kind` to figure out which table the row came from.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditEntry {
    source: AuditSource,
    audit_id: String,
    actor_id: String,
    target_user_id: String,
    /// The action tag — same string written to the storage column.
    kind: String,
    /// Action-specific payload as a JSON object. Empty `{}` for unit
    /// variants like `LoginSuccess`.
    detail: serde_json::Value,
    created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditListResponse {
    entries: Vec<AuditEntry>,
}

/// GET /admin/audit — combined view across the two audit tables.
///
/// Both tables key rows by the user PK (`USER#<id>`), so this handler
/// REQUIRES a `target` query parameter — without one we'd need to scan
/// the full table, which is exactly the access pattern the Phase 4
/// plan calls out as a v2 carry-forward (actor-indexed GSI on
/// AdminAudit, gap [#49]). The frontend supplies it via the user
/// detail page; the global "all activity" view is intentionally not
/// in scope for M-E2.
async fn list_audit(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<AuditQuery>,
) -> Result<Json<AuditListResponse>, ApiError> {
    require_admin(&auth)?;

    let target = params.target.as_deref().filter(|s| !s.is_empty()).ok_or(
        ApiError::BadRequest(
            "target user_id is required (M-E2 v1; actor-indexed scan is v2)".to_string(),
        ),
    )?;
    let limit = params.limit.unwrap_or(50).min(200);

    let admin_rows = state
        .admin_audit_repo
        .list_for_user(target, limit)
        .await?;
    let security_rows = state
        .security_audit_repo
        .list_for_user(target, limit)
        .await?;

    let mut entries: Vec<AuditEntry> = admin_rows
        .into_iter()
        .map(admin_row_to_entry)
        .chain(security_rows.into_iter().map(security_row_to_entry))
        .filter(|e| match params.actor.as_deref() {
            Some(a) if !a.is_empty() => e.actor_id == a,
            _ => true,
        })
        .filter(|e| match params.kind.as_deref() {
            Some(k) if !k.is_empty() => e.kind == k,
            _ => true,
        })
        .filter(|e| match params.from {
            Some(lo) => e.created_at >= lo,
            None => true,
        })
        .filter(|e| match params.to {
            Some(hi) => e.created_at < hi,
            None => true,
        })
        .collect();

    // Newest-first across the merged set. Both source queries already
    // return newest-first individually; the merge here ensures the
    // chronological order survives the chain().
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    entries.truncate(limit);

    Ok(Json(AuditListResponse { entries }))
}

fn admin_row_to_entry(row: AdminAudit) -> AuditEntry {
    // AdminAudit.detail is already a JSON-string; deserialize it once
    // so the wire shape matches the SecurityAudit branch (object, not
    // string-of-json).
    let detail = serde_json::from_str(&row.detail).unwrap_or_else(|_| serde_json::json!({}));
    AuditEntry {
        source: AuditSource::Admin,
        audit_id: row.audit_id,
        actor_id: row.actor_id,
        target_user_id: row.target_user_id,
        kind: row.action.as_str().to_string(),
        detail,
        created_at: row.created_at,
    }
}

fn security_row_to_entry(row: SecurityAudit) -> AuditEntry {
    AuditEntry {
        source: AuditSource::Security,
        audit_id: row.audit_id,
        actor_id: row.actor_id,
        target_user_id: row.user_id,
        kind: row.action.as_str().to_string(),
        detail: row.action.detail_json(),
        created_at: row.created_at,
    }
}

// ─── Force-compact a specific document ──────────────────────────

/// Outcome surface returned to the operator. Mirrors the
/// `CompactOutcome` enum in `crate::compaction` minus the typed
/// `version` field — JSON-friendly shape with explicit kinds.
#[derive(Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "result")]
enum ForceCompactResponse {
    Compacted {
        snapshot_version: u64,
        updates_pruned: u64,
    },
    RoomMissing,
    MetadataMissing,
    SnapshotFailed,
}

/// POST `/admin/documents/{id}/compact` — force a one-shot
/// compaction of a doc's room, even if clients are connected.
///
/// Why this exists: the periodic compaction worker only fires for
/// rooms idle ≥5 min with zero connected clients. A doc that's
/// kept open in a tab for hours never qualifies and accumulates
/// UPDATE# rows indefinitely. When a doc has accumulated a large
/// number of degenerate updates (e.g. the d92dac4 bug class) the
/// operator can call this endpoint to snapshot the merged state
/// to S3 and prune the old rows, restoring the doc to a clean
/// `snapshot + 0 updates` baseline without disturbing connected
/// clients.
///
/// Race posture: `compact_room_with_outcome` captures a `cutoff`
/// timestamp *before* reading the room state; the pruner only
/// deletes UPDATE# rows with `created_at < cutoff`. Any update
/// that lands during the snapshot-write is strictly newer and
/// survives.
async fn force_compact_document(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(doc_id): Path<String>,
) -> Result<Json<ForceCompactResponse>, ApiError> {
    require_admin(&auth)?;
    enforce_admin_mut_rate_limit(&state, &auth.user_id).await?;

    let outcome = crate::compaction::compact_room_with_outcome(
        &state.room_registry,
        &state.doc_repo,
        &doc_id,
        true, // force=true — skip the no-clients gate
    )
    .await;

    tracing::warn!(
        event_type = "admin_action",
        action = "force_compact_document",
        actor_id = %auth.user_id,
        target_doc_id = %doc_id,
        outcome = ?outcome,
        "admin force-compacted document"
    );

    // Durable audit trail for this privileged action (previously only in
    // tracing logs). Emitted only when a compaction actually occurred,
    // keyed on the document owner (subject) with the admin as actor, so it
    // surfaces in GET /admin/audit alongside the user-keyed AdminAudit rows.
    if matches!(outcome, crate::compaction::CompactOutcome::Compacted { .. }) {
        if let Ok(Some(meta)) = state.doc_repo.get(&doc_id).await {
            crate::routes::audit::record_security_event_by_actor(
                &state,
                &meta.owner_id,
                &auth.user_id,
                ogrenotes_storage::models::security_audit::SecurityAuditAction::DocCompacted {
                    doc_id: doc_id.clone(),
                },
            );
        }
    }

    let resp = match outcome {
        crate::compaction::CompactOutcome::Compacted { version, updates_pruned } => {
            ForceCompactResponse::Compacted {
                snapshot_version: version,
                updates_pruned,
            }
        }
        crate::compaction::CompactOutcome::RoomMissing => ForceCompactResponse::RoomMissing,
        crate::compaction::CompactOutcome::RoomActive => {
            // Force=true so this can't be returned by the compaction
            // function. Unreachable, but map to a typed response
            // rather than panic.
            ForceCompactResponse::RoomMissing
        }
        crate::compaction::CompactOutcome::MetadataMissing => {
            ForceCompactResponse::MetadataMissing
        }
        crate::compaction::CompactOutcome::SnapshotFailed => ForceCompactResponse::SnapshotFailed,
    };
    Ok(Json(resp))
}

// ─── Admin repair of LiveApp attributes ─────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RepairLiveAppResponse {
    /// Number of LiveApp nodes whose attributes were rewritten to
    /// their canonical form.
    nodes_touched: usize,
    /// Bounded log of (nodeType, blockId, field) triples for the
    /// operator. Capped at 32 entries by the collab helper.
    changes: Vec<RepairChangeEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RepairChangeEntry {
    node_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
    field: String,
}

/// POST /admin/documents/:id/repair-liveapp-attrs — walk the
/// document's LiveApp nodes and canonicalize any attributes that
/// fail `validate_attrs` or diverge from the block's canonical
/// form. Emits a `LiveAppAttrsRepaired` SecurityAudit row keyed on
/// the doc owner (subject) with the admin as actor.
///
/// This is the escape hatch for the gap-001 class of failure: a
/// doc with a legacy invalid attribute that the Phase 3 changed-
/// refs walk still rejects when a client touches that specific
/// element. After repair, the invalid attribute is replaced with
/// the block's default and the doc becomes writable again.
///
/// Runs against the *room-loaded* doc — so any active WS clients
/// immediately receive the repair delta via normal broadcast. If
/// no room is loaded, the endpoint cold-loads the doc from
/// snapshot + updates, applies the repair, and lets the next
/// compaction round persist it.
async fn repair_liveapp_attrs(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(doc_id): Path<String>,
) -> Result<Json<RepairLiveAppResponse>, ApiError> {
    require_admin(&auth)?;
    enforce_admin_mut_rate_limit(&state, &auth.user_id).await?;

    // Load the room (creating it from disk if not resident). Any
    // active WS clients will observe the repair through their
    // update subscription.
    let room = ensure_room_loaded_for_repair(&state, &doc_id).await?;
    let report = room.repair_liveapp_attrs().await;

    tracing::warn!(
        event_type = "admin_action",
        action = "repair_liveapp_attrs",
        actor_id = %auth.user_id,
        target_doc_id = %doc_id,
        nodes_touched = report.nodes_touched,
        "admin repaired LiveApp attributes",
    );

    if report.nodes_touched > 0 {
        if let Ok(Some(meta)) = state.doc_repo.get(&doc_id).await {
            crate::routes::audit::record_security_event_by_actor(
                &state,
                &meta.owner_id,
                &auth.user_id,
                ogrenotes_storage::models::security_audit::SecurityAuditAction::LiveAppAttrsRepaired {
                    doc_id: doc_id.clone(),
                    canonicalized_count: report.nodes_touched,
                },
            );
        }
    }

    let resp = RepairLiveAppResponse {
        nodes_touched: report.nodes_touched,
        changes: report
            .changes
            .into_iter()
            .map(|(nt, bid, field)| RepairChangeEntry {
                node_type: nt.tag_name().to_string(),
                block_id: bid,
                field,
            })
            .collect(),
    };
    Ok(Json(resp))
}

async fn ensure_room_loaded_for_repair(
    state: &AppState,
    doc_id: &str,
) -> Result<std::sync::Arc<ogrenotes_collab::room::Room>, ApiError> {
    use ogrenotes_collab::document::OgreDoc;
    if let Some(existing) = state.room_registry.get(doc_id) {
        return Ok(existing);
    }
    let init_lock = state
        .room_init_locks
        .entry(doc_id.to_string())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = init_lock.lock().await;
    if let Some(existing) = state.room_registry.get(doc_id) {
        return Ok(existing);
    }
    let snapshot = state
        .doc_repo
        .load_snapshot(doc_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("no snapshot for doc {doc_id}")))?;
    let mut doc = OgreDoc::from_state_bytes(&snapshot)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let updates = state
        .doc_repo
        .get_pending_updates(doc_id, state.config.max_pending_updates_bytes)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    for update in &updates {
        // Deliberately use the raw path — pending updates are
        // historical and may pre-date the gate; refusing them
        // here would prevent the repair from even loading the
        // doc.
        let _ = doc.apply_update(&update.update_bytes);
    }
    Ok(state.room_registry.get_or_insert(doc_id, doc))
}
