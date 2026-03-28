use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::folder::{Folder, FolderChild};
use ogrenotes_storage::models::{ChildType, FolderType};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

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
    children: Vec<ChildResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChildResponse {
    child_id: String,
    child_type: String,
    title: String,
    added_at: i64,
}

/// POST /folders -- create a new folder.
async fn create_folder(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<CreateFolderRequest>,
) -> Result<(StatusCode, Json<FolderResponse>), ApiError> {
    let folder_id = new_id();
    let now = now_usec();

    // Resolve and verify parent folder ownership
    let parent_id = match req.parent_id {
        Some(pid) => {
            let parent = state
                .folder_repo
                .get(&pid)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .ok_or(ApiError::NotFound("Parent folder not found".to_string()))?;
            if parent.owner_id != user_id {
                return Err(ApiError::NotFound("Parent folder not found".to_string()));
            }
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

    if folder.owner_id != user_id {
        return Err(ApiError::NotFound("Folder not found".to_string()));
    }

    let children = state
        .folder_repo
        .list_children(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Enrich children with titles by fetching each item
    let mut child_responses = Vec::with_capacity(children.len());
    for c in children {
        let title = match c.child_type {
            ChildType::Doc => {
                match state.doc_repo.get(&c.child_id).await {
                    Ok(Some(doc)) if !doc.is_deleted => doc.title,
                    _ => continue, // skip deleted or missing docs
                }
            }
            ChildType::Folder => {
                match state.folder_repo.get(&c.child_id).await {
                    Ok(Some(f)) => f.title,
                    _ => continue, // skip missing folders
                }
            }
        };
        child_responses.push(ChildResponse {
            child_id: c.child_id,
            child_type: c.child_type.as_str().to_string(),
            title,
            added_at: c.added_at,
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
        children: child_responses,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateFolderRequest {
    title: Option<String>,
    color: Option<u8>,
    parent_id: Option<String>,
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

    state
        .folder_repo
        .update(
            &id,
            req.title.as_deref(),
            req.color,
            req.parent_id.as_deref(),
            now_usec(),
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

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
    // Verify user owns the target folder
    let folder = state
        .folder_repo
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    if folder.owner_id != user_id {
        return Err(ApiError::NotFound("Folder not found".to_string()));
    }

    let child_type: ChildType = serde_json::from_str(&format!("\"{}\"", req.child_type))
        .map_err(|_| ApiError::BadRequest(format!("Invalid child type: {}", req.child_type)))?;

    // Verify user owns the child being added
    match child_type {
        ChildType::Doc => {
            let doc = state
                .doc_repo
                .get(&req.child_id)
                .await?
                .ok_or(ApiError::NotFound("Document not found".to_string()))?;
            if doc.owner_id != user_id || doc.is_deleted {
                return Err(ApiError::NotFound("Document not found".to_string()));
            }
        }
        ChildType::Folder => {
            let child_folder = state
                .folder_repo
                .get(&req.child_id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .ok_or(ApiError::NotFound("Child folder not found".to_string()))?;
            if child_folder.owner_id != user_id {
                return Err(ApiError::NotFound("Child folder not found".to_string()));
            }
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
    let folder = state
        .folder_repo
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Folder not found".to_string()))?;

    if folder.owner_id != user_id {
        return Err(ApiError::NotFound("Folder not found".to_string()));
    }

    state
        .folder_repo
        .remove_child(&id, &child_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
