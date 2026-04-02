//! Document edit history endpoints.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/versions", get(list_versions))
        .route("/{id}/versions/{version}", get(get_version_content))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionEntry {
    version: u64,
    s3_key: String,
    size_bytes: u64,
    created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionListResponse {
    versions: Vec<VersionEntry>,
}

/// GET /documents/:id/versions — list snapshot versions for a document.
async fn list_versions(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<axum::Json<VersionListResponse>, ApiError> {
    let _meta = super::documents::check_doc_access(
        &state,
        &id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;

    // Load snapshot entries from DynamoDB.
    let snapshots = state.snapshot_repo.list(&id).await?;

    let mut versions: Vec<VersionEntry> = snapshots
        .into_iter()
        .map(|s| VersionEntry {
            version: s.version,
            s3_key: s.s3_key,
            size_bytes: s.size_bytes,
            created_at: s.created_at,
        })
        .collect();

    // Always include the current live version.
    if let Ok(Some(meta)) = state.doc_repo.get(&id).await {
        if let Some(ref s3_key) = meta.snapshot_s3_key {
            // Avoid duplicating if the latest snapshot was just written.
            if !versions.iter().any(|v| v.version == meta.snapshot_version) {
                versions.push(VersionEntry {
                    version: meta.snapshot_version,
                    s3_key: s3_key.clone(),
                    size_bytes: 0,
                    created_at: meta.updated_at,
                });
            }
        }
    }

    // Sort descending (newest first).
    versions.sort_by(|a, b| b.version.cmp(&a.version));

    Ok(axum::Json(VersionListResponse { versions }))
}

/// GET /documents/:id/versions/:version — load the content of a specific version.
/// Returns the raw yrs state bytes as application/octet-stream.
async fn get_version_content(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, version)): Path<(String, u64)>,
) -> Result<axum::body::Bytes, ApiError> {
    let meta = super::documents::check_doc_access(
        &state,
        &id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;

    // Look up the snapshot record to get the verified S3 key.
    // This prevents path traversal via crafted version numbers.
    let s3_key = if meta.snapshot_version == version {
        // Current live version — use metadata directly.
        meta.snapshot_s3_key
            .ok_or_else(|| ApiError::NotFound("No snapshot for this version".to_string()))?
    } else {
        // Historical version — look up from SNAPSHOT# records.
        let snapshots = state.snapshot_repo.list(&id).await?;
        snapshots
            .into_iter()
            .find(|s| s.version == version)
            .map(|s| s.s3_key)
            .ok_or_else(|| ApiError::NotFound("Version not found".to_string()))?
    };

    let data = state
        .doc_repo
        .s3()
        .get_object(&s3_key)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to load snapshot: {e}")))?;

    Ok(axum::body::Bytes::from(data))
}
