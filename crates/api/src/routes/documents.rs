use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_collab::document::OgreDoc;
use ogrenotes_collab::export;
use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::document::DocumentMeta;
use ogrenotes_storage::models::folder::FolderChild;
use ogrenotes_storage::models::{ChildType, DocType};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Maximum document content size: 10 MB.
const MAX_CONTENT_SIZE: usize = 10 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_document))
        .route("/{id}", get(get_document))
        .route("/{id}", patch(update_document))
        .route("/{id}", delete(delete_document))
        .route("/{id}/content", get(get_content))
        .route("/{id}/content", put(put_content))
        .route("/{id}/export/{format}", get(export_document))
        .route("/{id}/blobs", post(request_upload_url))
        .route("/{id}/blobs/{blob_id}", get(request_download_url))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateDocumentRequest {
    #[serde(default = "default_title")]
    title: String,
    folder_id: Option<String>,
}

fn default_title() -> String {
    "Untitled".to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentResponse {
    id: String,
    title: String,
    doc_type: String,
    created_at: i64,
    updated_at: i64,
}

/// POST /documents -- create a new document.
async fn create_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<CreateDocumentRequest>,
) -> Result<(StatusCode, Json<DocumentResponse>), ApiError> {
    let doc_id = new_id();
    let now = now_usec();

    // Resolve and verify target folder ownership
    let folder_id = match req.folder_id {
        Some(ref id) => {
            let folder = state
                .folder_repo
                .get(id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .ok_or(ApiError::NotFound("Target folder not found".to_string()))?;
            if folder.owner_id != user_id {
                return Err(ApiError::NotFound("Target folder not found".to_string()));
            }
            id.clone()
        }
        None => {
            let user = state
                .user_repo
                .get_by_id(&user_id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .ok_or(ApiError::NotFound("User not found".to_string()))?;
            user.home_folder_id
        }
    };

    // Add to folder first (so document is never orphaned)
    let child = FolderChild {
        folder_id: folder_id.clone(),
        child_id: doc_id.clone(),
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&child)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Create document with initial snapshot
    let ogre_doc = OgreDoc::new();
    let snapshot = ogre_doc.to_state_bytes();

    let meta = DocumentMeta {
        doc_id: doc_id.clone(),
        title: req.title.clone(),
        owner_id: user_id,
        doc_type: DocType::Document,
        snapshot_version: 1,
        snapshot_s3_key: Some(format!("docs/{doc_id}/snapshots/1.bin")),
        is_deleted: false,
        deleted_at: None,
        created_at: now,
        updated_at: now,
    };

    state.doc_repo.create(&meta, &snapshot).await?;

    Ok((
        StatusCode::CREATED,
        Json(DocumentResponse {
            id: doc_id,
            title: req.title,
            doc_type: DocType::Document.as_str().to_string(),
            created_at: now,
            updated_at: now,
        }),
    ))
}

/// Fetch and verify a document belongs to the user and is not deleted.
async fn get_verified_doc(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
) -> Result<DocumentMeta, ApiError> {
    let meta = state
        .doc_repo
        .get(doc_id)
        .await?
        .ok_or(ApiError::NotFound("Document not found".to_string()))?;

    if meta.is_deleted || meta.owner_id != user_id {
        return Err(ApiError::NotFound("Document not found".to_string()));
    }

    Ok(meta)
}

/// GET /documents/:id -- get document metadata.
async fn get_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DocumentResponse>, ApiError> {
    let meta = get_verified_doc(&state, &id, &user_id).await?;

    Ok(Json(DocumentResponse {
        id: meta.doc_id,
        title: meta.title,
        doc_type: meta.doc_type.as_str().to_string(),
        created_at: meta.created_at,
        updated_at: meta.updated_at,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDocumentRequest {
    title: Option<String>,
}

/// PATCH /documents/:id -- update document metadata.
async fn update_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateDocumentRequest>,
) -> Result<StatusCode, ApiError> {
    // get_verified_doc checks is_deleted and ownership
    let _meta = get_verified_doc(&state, &id, &user_id).await?;

    state
        .doc_repo
        .update_metadata(&id, req.title.as_deref(), now_usec())
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteDocumentRequest {
    /// The folder the document is currently in (so we can remove it).
    source_folder_id: Option<String>,
}

/// DELETE /documents/:id -- soft delete and move to trash.
async fn delete_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    delete_req: Option<Json<DeleteDocumentRequest>>,
) -> Result<StatusCode, ApiError> {
    let meta = state
        .doc_repo
        .get(&id)
        .await?
        .ok_or(ApiError::NotFound("Document not found".to_string()))?;

    if meta.owner_id != user_id {
        return Err(ApiError::NotFound("Document not found".to_string()));
    }

    // Already deleted -- idempotent
    if meta.is_deleted {
        return Ok(StatusCode::NO_CONTENT);
    }

    let now = now_usec();
    state.doc_repo.soft_delete(&id, now).await?;

    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::Internal("User not found".to_string()))?;

    // Remove from source folder if provided
    if let Some(Json(req)) = delete_req {
        if let Some(source_id) = req.source_folder_id {
            let _ = state.folder_repo.remove_child(&source_id, &id).await;
        }
    }

    // Add to trash folder
    let trash_child = FolderChild {
        folder_id: user.trash_folder_id,
        child_id: id,
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&trash_child)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /documents/:id/content -- load Y.Doc state as binary.
async fn get_content(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<(HeaderMap, Bytes), ApiError> {
    let _meta = get_verified_doc(&state, &id, &user_id).await?;

    let snapshot = state
        .doc_repo
        .load_snapshot(&id)
        .await?
        .ok_or(ApiError::Internal("Snapshot not found".to_string()))?;

    // Apply any pending updates on top of the snapshot
    let mut doc = OgreDoc::from_state_bytes(&snapshot)?;
    let updates = state.doc_repo.get_pending_updates(&id).await?;
    for update in &updates {
        doc.apply_update(&update.update_bytes)?;
    }

    let state_bytes = doc.to_state_bytes();
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/octet-stream".parse().unwrap());

    Ok((headers, Bytes::from(state_bytes)))
}

/// PUT /documents/:id/content -- save Y.Doc state as binary.
async fn put_content(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    // Enforce body size limit
    if body.len() > MAX_CONTENT_SIZE {
        return Err(ApiError::BadRequest(format!(
            "Content too large: {} bytes (max {})",
            body.len(),
            MAX_CONTENT_SIZE
        )));
    }

    let meta = get_verified_doc(&state, &id, &user_id).await?;

    // Validate that the bytes are a valid Y.Doc state
    let _doc = OgreDoc::from_state_bytes(&body)?;

    // Use optimistic locking: only update if the current version matches
    let new_version = meta.snapshot_version + 1;
    let s3_key = format!("docs/{id}/snapshots/{new_version}.bin");

    // Conditional update: only succeed if snapshot_version hasn't changed
    let pk = format!("DOC#{id}");
    let mut values = std::collections::HashMap::new();
    values.insert(
        ":new_version".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::N(new_version.to_string()),
    );
    values.insert(
        ":expected_version".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::N(meta.snapshot_version.to_string()),
    );
    values.insert(
        ":s3_key".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::S(s3_key.clone()),
    );
    values.insert(
        ":updated_at".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::N(now_usec().to_string()),
    );

    let result = state
        .doc_repo
        .conditional_update_snapshot(
            &pk,
            "SET snapshot_version = :new_version, snapshot_s3_key = :s3_key, updated_at = :updated_at",
            "snapshot_version = :expected_version",
            values,
        )
        .await;

    match result {
        Ok(()) => {
            // Write snapshot to S3
            state
                .doc_repo
                .s3()
                .put_object(&s3_key, body.to_vec())
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("ConditionalCheckFailed") {
                Err(ApiError::Conflict(
                    "Document was modified concurrently -- reload and retry".to_string(),
                ))
            } else {
                Err(ApiError::Internal(err_str))
            }
        }
    }
}

/// GET /documents/:id/export/:format -- export as html or markdown.
async fn export_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, format)): Path<(String, String)>,
) -> Result<(HeaderMap, String), ApiError> {
    let _meta = get_verified_doc(&state, &id, &user_id).await?;

    let snapshot = state
        .doc_repo
        .load_snapshot(&id)
        .await?
        .ok_or(ApiError::Internal("Snapshot not found".to_string()))?;

    let doc = OgreDoc::from_state_bytes(&snapshot)?;

    let (content_type, content) = match format.as_str() {
        "html" => ("text/html; charset=utf-8", export::to_html(doc.inner())),
        "markdown" | "md" => ("text/markdown; charset=utf-8", export::to_markdown(doc.inner())),
        _ => return Err(ApiError::BadRequest(format!("Unsupported export format: {format}"))),
    };

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", content_type.parse().unwrap());

    Ok((headers, content))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadRequest {
    filename: String,
    content_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadResponse {
    upload_url: String,
    blob_id: String,
    key: String,
}

/// POST /documents/:id/blobs -- request presigned upload URL.
async fn request_upload_url(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UploadRequest>,
) -> Result<Json<UploadResponse>, ApiError> {
    let _meta = get_verified_doc(&state, &id, &user_id).await?;

    if !is_allowed_content_type(&req.content_type) {
        return Err(ApiError::BadRequest(format!(
            "Content type not allowed: {}",
            req.content_type
        )));
    }

    let blob_id = new_id();
    let key = format!("blobs/{id}/{blob_id}/{}", req.filename);

    let url = state
        .doc_repo
        .s3()
        .presigned_put_url(&key, &req.content_type, 900)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(UploadResponse {
        upload_url: url,
        blob_id,
        key,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadRequest {
    key: String,
}

/// GET /documents/:id/blobs/:blob_id -- request presigned download URL.
/// Requires the `key` query parameter (returned from the upload response).
async fn request_download_url(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, blob_id)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<DownloadRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _meta = get_verified_doc(&state, &id, &user_id).await?;

    // Verify the key belongs to this document and blob
    let expected_prefix = format!("blobs/{id}/{blob_id}/");
    if !query.key.starts_with(&expected_prefix) {
        return Err(ApiError::BadRequest("Invalid blob key".to_string()));
    }

    let url = state
        .doc_repo
        .s3()
        .presigned_get_url(&query.key, 14400)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "downloadUrl": url
    })))
}

fn is_allowed_content_type(ct: &str) -> bool {
    let ct_lower = ct.to_lowercase();
    ct_lower.starts_with("image/")
        || ct_lower.starts_with("application/pdf")
        || ct_lower.starts_with("text/")
        || ct_lower == "application/octet-stream"
}
