// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Sharing endpoints for folders and documents.
//!
//! Folder sharing: manages folder membership (who has access to a folder's documents).
//! Document sharing: manages direct document membership (Phase 2).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::document::DocMember;
use ogrenotes_storage::models::folder::{FolderChild, FolderMember};
use ogrenotes_storage::models::notification::{NotifType, Notification};
use ogrenotes_storage::models::security_audit::SecurityAuditAction;
use ogrenotes_storage::models::AccessLevel;

use crate::error::ApiError;
use crate::routes::audit::record_security_event_by_actor;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Wire-shape label for an `AccessLevel`, matching the uppercase
/// rename used by the serde derive. Defined locally so the audit
/// row's `level` string stays stable even if the derive's rename
/// rule changes elsewhere — the audit log is durable storage and
/// its keys are part of the `GET /admin/audit` response.
fn access_level_label(l: &AccessLevel) -> &'static str {
    match l {
        AccessLevel::Own => "OWN",
        AccessLevel::Edit => "EDIT",
        AccessLevel::Comment => "COMMENT",
        AccessLevel::View => "VIEW",
    }
}

/// Per-user rate limit on sharing mutations (#36). Defends against a
/// compromised account fanning out share-spam to many targets, each
/// of which fires a downstream Notification + email work item.
async fn enforce_sharing_rate_limit(state: &AppState, user_id: &str) -> Result<(), ApiError> {
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "sharing",
        user_id,
        state.config.rate_limit_sharing_per_min,
        60,
    )
    .await
}

/// Build the folder sharing router (nested under /folders).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/members", get(list_members))
        .route("/{id}/members", post(add_member))
        .route("/{id}/members/{user_id}", patch(update_folder_member))
        .route("/{id}/members/{user_id}", delete(remove_member))
}

/// Build the document sharing router (nested under /documents).
pub fn doc_sharing_router() -> Router<AppState> {
    Router::new()
        .route("/{id}/members", get(list_doc_members))
        .route("/{id}/members", post(add_doc_member))
        .route("/{id}/members/{user_id}", patch(update_doc_member))
        .route("/{id}/members/{user_id}", delete(remove_doc_member))
}

// ─── Types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMemberRequest {
    user_id: String,
    access_level: AccessLevel,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberResponse {
    user_id: String,
    name: String,
    email: String,
    access_level: AccessLevel,
    added_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MembersListResponse {
    members: Vec<MemberResponse>,
}

// ─── Handlers ───────────────────────────────────────────────────

/// GET /folders/:id/members — list all members of a folder.
async fn list_members(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(folder_id): Path<String>,
) -> Result<axum::Json<MembersListResponse>, ApiError> {
    // Verify the caller has access to the folder (must be owner or member)
    let folder = state
        .folder_repo
        .get(&folder_id)
        .await?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    // Check: caller must be owner or a member of this folder
    if folder.owner_id != user_id {
        let member = state.folder_repo.get_member(&folder_id, &user_id).await?;
        if member.is_none() {
            return Err(ApiError::Forbidden);
        }
    }

    let members = state.folder_repo.list_members(&folder_id).await?;
    // One BatchGetItem for every member's name+email (#38) — previously a
    // sequential get_by_id per member. Missing rows keep the old fallback.
    let member_ids: Vec<String> = members.iter().map(|m| m.user_id.clone()).collect();
    let users = state.user_repo.get_by_ids(&member_ids).await.unwrap_or_default();
    let mut response_members = Vec::new();
    for m in members {
        let (name, email) = users
            .get(&m.user_id)
            .map(|u| (u.name.clone(), u.email.clone()))
            .unwrap_or_else(|| (m.user_id.clone(), String::new()));
        response_members.push(MemberResponse {
            user_id: m.user_id,
            name,
            email,
            access_level: m.access_level,
            added_at: m.added_at,
        });
    }

    Ok(axum::Json(MembersListResponse { members: response_members }))
}

/// POST /folders/:id/members — add or update a member's access to a folder.
async fn add_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(folder_id): Path<String>,
    axum::Json(body): axum::Json<AddMemberRequest>,
) -> Result<StatusCode, ApiError> {
    enforce_sharing_rate_limit(&state, &user_id).await?;
    // Only the folder owner can add members
    let folder = state
        .folder_repo
        .get(&folder_id)
        .await?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    if folder.owner_id != user_id {
        return Err(ApiError::Forbidden);
    }

    // Verify the target user exists
    let target_user = state.user_repo.get_by_id(&body.user_id).await?;
    if target_user.is_none() {
        return Err(ApiError::NotFound("User not found".to_string()));
    }

    // Cannot share with yourself (owner already has access)
    if body.user_id == user_id {
        return Err(ApiError::BadRequest(
            "Cannot share with yourself".to_string(),
        ));
    }

    // Cannot grant Own access — ownership is not transferable via sharing
    if body.access_level == AccessLevel::Own {
        return Err(ApiError::BadRequest(
            "Cannot grant Own access via sharing".to_string(),
        ));
    }

    // Cap folder membership (#34). Counted live each call: member sets
    // are bounded (config.max_members_per_folder, default 200) so the
    // list is cheap, and a stored counter would have to handle the same
    // race the count-then-add does.
    //
    // The owner is stored as a member row with `Own` access (FolderRepo
    // ::create writes it). Exclude that row from the cap so
    // `max_members_per_folder` is the count of *shared* users — without
    // this filter the cap silently subtracts 1 from the configured budget
    // and a fresh folder with cap=200 only admits 199 shared members.
    // Doc-side sharing has the same cap but no auto-owner row in
    // doc_members, so its check stays plain.
    let existing = state.folder_repo.list_members(&folder_id).await?;
    let non_owner_count = existing
        .iter()
        .filter(|m| m.user_id != folder.owner_id)
        .count();
    if non_owner_count >= state.config.max_members_per_folder
        && !existing.iter().any(|m| m.user_id == body.user_id)
    {
        return Err(ApiError::TooManyRequests {
            message: format!(
                "Folder has reached the membership cap of {}; remove a member before adding another.",
                state.config.max_members_per_folder
            ),
            retry_after_secs: 60,
        });
    }

    let member = FolderMember {
        folder_id: folder_id.clone(),
        user_id: body.user_id,
        access_level: body.access_level,
        added_at: now_usec(),
    };

    state.folder_repo.add_member(&member).await?;

    // SecurityAudit row keyed on the target (the granted member);
    // actor is the folder owner. `doc_id` holds the folder_id; the
    // audit schema does not distinguish docs from folders (see
    // ShareRevoked docs).
    record_security_event_by_actor(
        &state,
        &member.user_id,
        &user_id,
        SecurityAuditAction::ShareGranted {
            doc_id: folder_id.clone(),
            target: member.user_id.clone(),
            level: access_level_label(&member.access_level).to_string(),
        },
    );

    // Add the shared folder's documents to the recipient's home folder
    // so they appear in the recipient's file browser.
    {
        let folder_repo = state.folder_repo.clone();
        let user_repo = state.user_repo.clone();
        let target_id = member.user_id.clone();
        let shared_folder_id = folder_id.clone();
        tokio::spawn(async move {
            // Get recipient's home folder
            let home_folder_id = match user_repo.get_by_id(&target_id).await {
                Ok(Some(user)) => user.home_folder_id,
                _ => return,
            };

            // List children of the shared folder
            let children = match folder_repo.list_children(&shared_folder_id).await {
                Ok(c) => c,
                _ => return,
            };

            // Add each doc/folder as a child of the recipient's home folder
            let now = now_usec();
            for child in children {
                let new_child = FolderChild {
                    folder_id: home_folder_id.clone(),
                    child_id: child.child_id,
                    child_type: child.child_type,
                    added_at: now,
                };
                let _ = folder_repo.add_child(&new_child).await;
            }
        });
    }

    // Notify the target user that they've been granted access.
    let notif_repo = state.notification_repo.clone();
    let email_service = state.email_service.clone();
    let target = member.user_id.clone();
    let actor = user_id.clone();
    tokio::spawn(async move {
        let notif = Notification {
            notif_id: nanoid::nanoid!(16),
            user_id: target,
            notif_type: NotifType::Shared,
            doc_id: None,
            thread_id: None,
            actor_id: actor,
            message: format!("shared a folder with you"),
            preview: None,
            block_id: None,
            read: false,
            created_at: now_usec(),
        };
        let _ = notif_repo.create(&notif).await;
        // Share invites are direct — the recipient is the intended target.
        email_service.spawn_for_notification(notif, true);
    });

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /folders/:id/members/:user_id — remove a member from a folder.
async fn remove_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((folder_id, target_user_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    enforce_sharing_rate_limit(&state, &user_id).await?;
    // Only the folder owner can remove members
    let folder = state
        .folder_repo
        .get(&folder_id)
        .await?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    if folder.owner_id != user_id {
        return Err(ApiError::Forbidden);
    }

    // Cannot remove the owner
    if target_user_id == folder.owner_id {
        return Err(ApiError::BadRequest(
            "Cannot remove the folder owner".to_string(),
        ));
    }

    state
        .folder_repo
        .remove_member(&folder_id, &target_user_id)
        .await?;

    // SecurityAudit row keyed on the *target* (the removed member) —
    // they're the subject of "what happened to your access." The
    // authenticated caller (the folder owner here) is the actor;
    // capturing them separately answers "who removed me." `doc_id`
    // holds the folder_id here (the audit schema's column doesn't
    // distinguish between docs and folders; readers correlate the
    // id against the doc/folder repos).
    record_security_event_by_actor(
        &state,
        &target_user_id,
        &user_id,
        SecurityAuditAction::ShareRevoked {
            doc_id: folder_id.clone(),
            target: target_user_id.clone(),
        },
    );

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /folders/:id/members/:user_id — update a folder member's access level.
async fn update_folder_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((folder_id, target_user_id)): Path<(String, String)>,
    axum::Json(body): axum::Json<AddMemberRequest>,
) -> Result<StatusCode, ApiError> {
    enforce_sharing_rate_limit(&state, &user_id).await?;
    let folder = state
        .folder_repo
        .get(&folder_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    if folder.owner_id != user_id {
        return Err(ApiError::Forbidden);
    }

    if body.access_level == AccessLevel::Own {
        return Err(ApiError::BadRequest("Cannot grant Own access".to_string()));
    }

    // Verify member exists
    if state.folder_repo.get_member(&folder_id, &target_user_id).await?.is_none() {
        return Err(ApiError::NotFound("Member not found".to_string()));
    }

    // Overwrite with new access level (PutItem replaces)
    let member = FolderMember {
        folder_id,
        user_id: target_user_id,
        access_level: body.access_level,
        added_at: now_usec(),
    };
    state.folder_repo.add_member(&member).await?;

    // SecurityAudit row for the access-level change. Subject is the
    // target user; actor is the folder owner. `doc_id` carries the
    // folder id (audit schema doesn't distinguish).
    record_security_event_by_actor(
        &state,
        &member.user_id,
        &user_id,
        SecurityAuditAction::ShareUpdated {
            doc_id: member.folder_id.clone(),
            target: member.user_id.clone(),
            level: access_level_label(&member.access_level).to_string(),
        },
    );

    Ok(StatusCode::NO_CONTENT)
}

// ─── Document sharing handlers ──────────────────────────────────

/// GET /documents/:id/members — list direct document members.
async fn list_doc_members(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(doc_id): Path<String>,
) -> Result<axum::Json<MembersListResponse>, ApiError> {
    let meta = super::documents::check_doc_access(
        &state, &doc_id, &user_id, AccessLevel::View,
    ).await?;

    let members = state.doc_repo.list_doc_members(&doc_id).await?;
    // Batch member hydration (#38), same fallback as before.
    let member_ids: Vec<String> = members.iter().map(|m| m.user_id.clone()).collect();
    let users = state.user_repo.get_by_ids(&member_ids).await.unwrap_or_default();
    let mut responses = Vec::new();
    for m in members {
        let (name, email) = users
            .get(&m.user_id)
            .map(|u| (u.name.clone(), u.email.clone()))
            .unwrap_or_else(|| (m.user_id.clone(), String::new()));
        responses.push(MemberResponse {
            user_id: m.user_id,
            name,
            email,
            access_level: m.access_level,
            added_at: m.added_at,
        });
    }

    let _ = meta;
    Ok(axum::Json(MembersListResponse { members: responses }))
}

/// POST /documents/:id/members — share a document directly with a user.
async fn add_doc_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(doc_id): Path<String>,
    axum::Json(body): axum::Json<AddMemberRequest>,
) -> Result<StatusCode, ApiError> {
    enforce_sharing_rate_limit(&state, &user_id).await?;
    // Require Own access to share documents
    let _meta = super::documents::check_doc_access(
        &state, &doc_id, &user_id, AccessLevel::Own,
    ).await?;

    if body.access_level == AccessLevel::Own {
        return Err(ApiError::BadRequest("Cannot grant Own access".to_string()));
    }

    if body.user_id == user_id {
        return Err(ApiError::BadRequest("Cannot share with yourself".to_string()));
    }

    if state.user_repo.get_by_id(&body.user_id).await?.is_none() {
        return Err(ApiError::NotFound("User not found".to_string()));
    }

    // Check for duplicate
    if let Ok(Some(_)) = state.doc_repo.get_doc_member(&doc_id, &body.user_id).await {
        return Err(ApiError::Conflict("User already has access".to_string()));
    }

    // Cap document membership (#34). The duplicate check above means by
    // this point we know body.user_id is a *new* member, so an at-cap
    // doc rejects unconditionally.
    let existing_count = state.doc_repo.list_doc_members(&doc_id).await?.len();
    if existing_count >= state.config.max_members_per_doc {
        return Err(ApiError::TooManyRequests {
            message: format!(
                "Document has reached the membership cap of {}; remove a member before adding another.",
                state.config.max_members_per_doc
            ),
            retry_after_secs: 60,
        });
    }

    let member = DocMember {
        doc_id: doc_id.clone(),
        user_id: body.user_id.clone(),
        access_level: body.access_level,
        added_at: now_usec(),
    };
    state.doc_repo.add_doc_member(&member).await?;

    // SecurityAudit row for the grant — subject is the target user;
    // actor is the doc owner. Complements the Activity::Share event
    // below: Activity feeds the user-facing per-doc timeline,
    // SecurityAudit feeds the durable security log surfaced by
    // GET /admin/audit and the access-history forensic recipe.
    record_security_event_by_actor(
        &state,
        &member.user_id,
        &user_id,
        SecurityAuditAction::ShareGranted {
            doc_id: doc_id.clone(),
            target: member.user_id.clone(),
            level: access_level_label(&member.access_level).to_string(),
        },
    );

    // Record activity event
    {
        let activity_repo = state.activity_repo.clone();
        let act_doc_id = doc_id.clone();
        let act_user_id = user_id.clone();
        let act_target = body.user_id.clone();
        tokio::spawn(async move {
            let activity = ogrenotes_storage::models::activity::Activity {
                activity_id: nanoid::nanoid!(16),
                doc_id: act_doc_id,
                event_type: ogrenotes_storage::models::activity::ActivityEventType::Share,
                actor_id: act_user_id,
                detail: serde_json::json!({ "sharedWith": act_target }).to_string(),
                created_at: ogrenotes_common::time::now_usec(),
            };
            let _ = activity_repo.create(&activity).await;
        });
    }

    // Notify the target user
    let notif_repo = state.notification_repo.clone();
    let email_service = state.email_service.clone();
    let actor = user_id.clone();
    let target = body.user_id.clone();
    tokio::spawn(async move {
        let notif = Notification {
            notif_id: nanoid::nanoid!(16),
            user_id: target,
            notif_type: NotifType::Shared,
            doc_id: Some(doc_id),
            thread_id: None,
            actor_id: actor,
            message: "shared a document with you".to_string(),
            preview: None,
            block_id: None,
            read: false,
            created_at: now_usec(),
        };
        let _ = notif_repo.create(&notif).await;
        // Direct share invite — always fires under MentionsOnly too.
        email_service.spawn_for_notification(notif, true);
    });

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /documents/:id/members/:user_id — update a document member's access level.
async fn update_doc_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((doc_id, target_user_id)): Path<(String, String)>,
    axum::Json(body): axum::Json<AddMemberRequest>,
) -> Result<StatusCode, ApiError> {
    enforce_sharing_rate_limit(&state, &user_id).await?;
    let _meta = super::documents::check_doc_access(
        &state, &doc_id, &user_id, AccessLevel::Own,
    ).await?;

    if body.access_level == AccessLevel::Own {
        return Err(ApiError::BadRequest("Cannot grant Own access".to_string()));
    }

    if state.doc_repo.get_doc_member(&doc_id, &target_user_id).await?.is_none() {
        return Err(ApiError::NotFound("Member not found".to_string()));
    }

    let member = DocMember {
        doc_id,
        user_id: target_user_id,
        access_level: body.access_level,
        added_at: now_usec(),
    };
    state.doc_repo.add_doc_member(&member).await?;

    // SecurityAudit row for the access-level change. Same shape as
    // the folder update path above.
    record_security_event_by_actor(
        &state,
        &member.user_id,
        &user_id,
        SecurityAuditAction::ShareUpdated {
            doc_id: member.doc_id.clone(),
            target: member.user_id.clone(),
            level: access_level_label(&member.access_level).to_string(),
        },
    );

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /documents/:id/members/:user_id — remove a document member.
async fn remove_doc_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((doc_id, target_user_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    enforce_sharing_rate_limit(&state, &user_id).await?;
    let meta = super::documents::check_doc_access(
        &state, &doc_id, &user_id, AccessLevel::Own,
    ).await?;

    if target_user_id == meta.owner_id {
        return Err(ApiError::BadRequest("Cannot remove the document owner".to_string()));
    }

    state.doc_repo.remove_doc_member(&doc_id, &target_user_id).await?;

    // Audit row keyed on the target (the removed member); actor is
    // the authenticated caller (the doc owner). Same shape as the
    // folder revoke path above.
    record_security_event_by_actor(
        &state,
        &target_user_id,
        &user_id,
        SecurityAuditAction::ShareRevoked {
            doc_id: doc_id.clone(),
            target: target_user_id.clone(),
        },
    );

    Ok(StatusCode::NO_CONTENT)
}
