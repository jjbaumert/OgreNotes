// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::folder::{Folder, FolderChild};
use ogrenotes_storage::models::{AccessLevel, ChildType, FolderType};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Check that the user has at least the required access level to a folder.
/// Access: owner always OK, then folder membership, else denied.
pub(crate) async fn check_folder_access(
    state: &AppState,
    folder_id: &str,
    user_id: &str,
    required: AccessLevel,
) -> Result<Folder, ApiError> {
    let folder = state
        .folder_repo
        .get(folder_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    if folder.owner_id == user_id {
        return Ok(folder);
    }

    if let Ok(Some(member)) = state.folder_repo.get_member(folder_id, user_id).await {
        if super::documents::access_level_satisfies(&member.access_level, &required) {
            return Ok(folder);
        }
    }

    Err(ApiError::NotFound("Folder not found".to_string()))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_folder))
        .route("/{id}", get(get_folder))
        .route("/{id}", patch(update_folder))
        .route("/{id}", delete(delete_folder))
        .route("/{id}/children", post(add_child))
        .route("/{id}/children/{child_id}", delete(remove_child))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateFolderRequest {
    title: String,
    #[serde(default)]
    color: u8,
    parent_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderResponse {
    id: String,
    title: String,
    color: u8,
    parent_id: Option<String>,
    folder_type: String,
    created_at: i64,
    updated_at: i64,
    /// True when this is the caller's Trash system folder. Lets the frontend
    /// switch into trash-mode rendering (row actions for Restore / Delete
    /// forever) without hardcoding folder IDs.
    #[serde(default)]
    is_trash: bool,
    children: Vec<ChildResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChildResponse {
    child_id: String,
    child_type: String,
    title: String,
    added_at: i64,
    /// True for trashed documents surfaced in the Trash folder listing.
    /// Always false outside of the Trash folder.
    #[serde(default)]
    is_deleted: bool,
}

/// POST /folders -- create a new folder.
async fn create_folder(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<CreateFolderRequest>,
) -> Result<(StatusCode, Json<FolderResponse>), ApiError> {
    let folder_id = new_id();
    let now = now_usec();

    // Resolve and verify parent folder access
    let parent_id = match req.parent_id {
        Some(pid) => {
            let _parent = check_folder_access(&state, &pid, &user_id, AccessLevel::Edit).await?;
            Some(pid)
        }
        None => {
            let user = state
                .user_repo
                .get_by_id(&user_id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .ok_or(ApiError::NotFound("User not found".to_string()))?;
            Some(user.home_folder_id)
        }
    };

    // Add as child of parent first (so folder is never orphaned)
    if let Some(ref pid) = parent_id {
        let child = FolderChild {
            folder_id: pid.clone(),
            child_id: folder_id.clone(),
            child_type: ChildType::Folder,
            added_at: now,
        };
        state
            .folder_repo
            .add_child(&child)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }

    // Create the folder
    let folder = Folder {
        folder_id: folder_id.clone(),
        title: req.title.clone(),
        color: Folder::clamp_color(req.color),
        parent_id: parent_id.clone(),
        owner_id: user_id,
        folder_type: FolderType::User,
        inherit_mode: ogrenotes_storage::models::InheritMode::default(),
        created_at: now,
        updated_at: now,
    };

    state
        .folder_repo
        .create(&folder)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(FolderResponse {
            id: folder_id,
            title: req.title,
            color: Folder::clamp_color(req.color),
            parent_id,
            folder_type: FolderType::User.as_str().to_string(),
            created_at: now,
            updated_at: now,
            is_trash: false,
            children: vec![],
        }),
    ))
}

/// GET /folders/:id -- get folder metadata and children.
async fn get_folder(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<FolderResponse>, ApiError> {
    let folder = state
        .folder_repo
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    // Check access: owner or folder member.
    if folder.owner_id != user_id {
        let member = state
            .folder_repo
            .get_member(&id, &user_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if member.is_none() {
            return Err(ApiError::NotFound("Folder not found".to_string()));
        }
    }

    // Is this the caller's Trash folder? In that case we include children
    // whose docs have `is_deleted=true` — the whole point of the trash view.
    // The earlier owner/member gate on this folder prevents foreign reads.
    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let is_trash = user
        .as_ref()
        .map(|u| u.trash_folder_id == folder.folder_id)
        .unwrap_or(false);

    let children = state
        .folder_repo
        .list_children(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Enrich children with titles by fetching each item
    let mut child_responses = Vec::with_capacity(children.len());
    for c in children {
        let (title, child_is_deleted) = match c.child_type {
            ChildType::Doc => {
                match state.doc_repo.get(&c.child_id).await {
                    Ok(Some(doc)) => {
                        if doc.is_deleted && !is_trash {
                            continue; // hide deleted docs outside of trash
                        }
                        (doc.title, doc.is_deleted)
                    }
                    _ => continue, // skip missing docs
                }
            }
            ChildType::Folder => {
                match state.folder_repo.get(&c.child_id).await {
                    Ok(Some(f)) => (f.title, false),
                    _ => continue, // skip missing folders
                }
            }
        };
        child_responses.push(ChildResponse {
            child_id: c.child_id,
            child_type: c.child_type.as_str().to_string(),
            title,
            added_at: c.added_at,
            is_deleted: child_is_deleted,
        });
    }

    Ok(Json(FolderResponse {
        id: folder.folder_id,
        title: folder.title,
        color: folder.color,
        parent_id: folder.parent_id,
        folder_type: folder.folder_type.as_str().to_string(),
        created_at: folder.created_at,
        updated_at: folder.updated_at,
        is_trash,
        children: child_responses,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateFolderRequest {
    title: Option<String>,
    color: Option<u8>,
    parent_id: Option<String>,
    #[serde(default)]
    inherit_mode: Option<ogrenotes_storage::models::InheritMode>,
}

/// PATCH /folders/:id -- update folder metadata.
async fn update_folder(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateFolderRequest>,
) -> Result<StatusCode, ApiError> {
    let folder = state
        .folder_repo
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    if folder.owner_id != user_id {
        return Err(ApiError::NotFound("Folder not found".to_string()));
    }

    if folder.folder_type == FolderType::System && (req.title.is_some() || req.parent_id.is_some())
    {
        return Err(ApiError::Forbidden);
    }

    // Reject reparenting that would create a cycle. This is O(depth) with a
    // hard cap — real folder trees are shallow.
    if let Some(ref new_parent) = req.parent_id {
        if new_parent == &id {
            return Err(ApiError::BadRequest(
                "Cannot set folder's parent to itself".to_string(),
            ));
        }
        let mut cursor = new_parent.clone();
        for _ in 0..64 {
            let Some(f) = state
                .folder_repo
                .get(&cursor)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
            else {
                break;
            };
            match f.parent_id {
                Some(pid) if pid == id => {
                    return Err(ApiError::BadRequest(
                        "Cannot move folder under its own descendant".to_string(),
                    ));
                }
                Some(pid) => cursor = pid,
                None => break,
            }
        }
    }

    state
        .folder_repo
        .update(
            &id,
            req.title.as_deref(),
            req.color,
            req.parent_id.as_deref(),
            req.inherit_mode.as_ref(),
            now_usec(),
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // #37: drop the cached inherit_mode so a Restrict/inherit change takes
    // effect on the next REST access check instead of lingering for the TTL.
    // Unconditional (a rename is a harmless cache miss).
    state.folder_inherit_cache.invalidate(&id);

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /folders/:id -- delete a user folder.
async fn delete_folder(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let folder = state
        .folder_repo
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    if folder.owner_id != user_id {
        return Err(ApiError::NotFound("Folder not found".to_string()));
    }

    if folder.folder_type == FolderType::System {
        return Err(ApiError::Forbidden);
    }

    state
        .folder_repo
        .delete(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // #37: drop any cached inherit_mode for the now-deleted folder so a
    // later access check re-reads authoritatively (a `get` miss → no grant).
    state.folder_inherit_cache.invalidate(&id);

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddChildRequest {
    child_id: String,
    child_type: String,
}

/// POST /folders/:id/children -- add a document or subfolder to a folder.
async fn add_child(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<AddChildRequest>,
) -> Result<StatusCode, ApiError> {
    // Verify user has Edit access to the target folder
    let _folder = check_folder_access(&state, &id, &user_id, AccessLevel::Edit).await?;

    let child_type: ChildType = serde_json::from_str(&format!("\"{}\"", req.child_type))
        .map_err(|_| ApiError::BadRequest(format!("Invalid child type: {}", req.child_type)))?;

    // Verify user has Edit access to the child being added
    match child_type {
        ChildType::Doc => {
            let _doc = super::documents::check_doc_access(
                &state, &req.child_id, &user_id, AccessLevel::Edit,
            ).await?;
        }
        ChildType::Folder => {
            let _child = check_folder_access(
                &state, &req.child_id, &user_id, AccessLevel::Edit,
            ).await?;
        }
    }

    let child = FolderChild {
        folder_id: id,
        child_id: req.child_id,
        child_type,
        added_at: now_usec(),
    };

    state
        .folder_repo
        .add_child(&child)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::CREATED)
}

/// DELETE /folders/:id/children/:child_id -- remove a child from a folder.
async fn remove_child(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, child_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    // Verify user has Edit access to the folder
    let _folder = check_folder_access(&state, &id, &user_id, AccessLevel::Edit).await?;

    // Verify the child actually belongs to this folder before removing
    let children = state
        .folder_repo
        .list_children(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !children.iter().any(|c| c.child_id == child_id) {
        return Err(ApiError::NotFound("Child not found in this folder".to_string()));
    }

    state
        .folder_repo
        .remove_child(&id, &child_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
