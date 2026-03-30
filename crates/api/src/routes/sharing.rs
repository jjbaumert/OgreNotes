//! Folder sharing endpoints.
//!
//! Manages folder membership (who has access to a folder's documents).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::folder::FolderMember;
use ogrenotes_storage::models::AccessLevel;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Build the sharing router (nested under /folders).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/members", get(list_members))
        .route("/{id}/members", post(add_member))
        .route("/{id}/members/{user_id}", delete(remove_member))
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
    let response = MembersListResponse {
        members: members
            .into_iter()
            .map(|m| MemberResponse {
                user_id: m.user_id,
                access_level: m.access_level,
                added_at: m.added_at,
            })
            .collect(),
    };

    Ok(axum::Json(response))
}

/// POST /folders/:id/members — add or update a member's access to a folder.
async fn add_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(folder_id): Path<String>,
    axum::Json(body): axum::Json<AddMemberRequest>,
) -> Result<StatusCode, ApiError> {
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

    let member = FolderMember {
        folder_id: folder_id.clone(),
        user_id: body.user_id,
        access_level: body.access_level,
        added_at: now_usec(),
    };

    state.folder_repo.add_member(&member).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /folders/:id/members/:user_id — remove a member from a folder.
async fn remove_member(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((folder_id, target_user_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
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

    Ok(StatusCode::NO_CONTENT)
}
