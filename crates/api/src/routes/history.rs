//! Document edit history endpoints.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/{id}/versions", get(list_versions))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionEntry {
    version: u64,
    s3_key: String,
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
    // Verify access
    let meta = super::documents::check_doc_access(
        &state,
        &id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;

    // For MVP, the versions are just the current snapshot.
    // Full version listing from SNAPSHOT# rows will be activated when
    // idle compaction (Batch 2) starts writing SNAPSHOT# entries.
    let mut versions = Vec::new();
    if let Some(ref s3_key) = meta.snapshot_s3_key {
        versions.push(VersionEntry {
            version: meta.snapshot_version,
            s3_key: s3_key.clone(),
            created_at: meta.updated_at,
        });
    }

    Ok(axum::Json(VersionListResponse { versions }))
}
