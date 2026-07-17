// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Document edit history endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use ogrenotes_collab::diff::DiffEntry;
use ogrenotes_collab::document::OgreDoc;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::repo::doc_repo::SnapshotWrite;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/versions", get(list_versions))
        .route("/{id}/versions/{version}", get(get_version_content))
        .route("/{id}/versions/{v1}/diff/{v2}", get(diff_versions))
        .route("/{id}/versions/{version}/restore", post(restore_version))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionEntry {
    version: u64,
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
    let meta = super::documents::check_doc_access(
        &state,
        &id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;
    // View-mode link viewers see history only when `show_history` is on
    // (Phase 2, §5.3); durable members and Edit-link viewers are unaffected.
    super::documents::enforce_view_link_option(
        &state, &meta, &user_id, meta.link_view_options.show_history,
    )
    .await?;

    // Load snapshot entries from DynamoDB.
    let snapshots = state.snapshot_repo.list(&id).await?;

    let mut versions: Vec<VersionEntry> = snapshots
        .into_iter()
        .map(|s| VersionEntry {
            version: s.version,
            size_bytes: s.size_bytes,
            created_at: s.created_at,
        })
        .collect();

    // Always include the current live version.
    if let Ok(Some(meta)) = state.doc_repo.get(&id).await {
        if meta.snapshot_s3_key.is_some() {
            if !versions.iter().any(|v| v.version == meta.snapshot_version) {
                versions.push(VersionEntry {
                    version: meta.snapshot_version,
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
    super::documents::enforce_view_link_option(
        &state, &meta, &user_id, meta.link_view_options.show_history,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffResponse {
    diffs: Vec<DiffEntry>,
}

/// Resolve the S3 key for a given version number, plus the underlying
/// `DocSnapshot` row when the version is not the live one. Returning the
/// full row lets callers reuse the (user_id, created_at) attribution
/// fields without a second DynamoDB read.
async fn resolve_snapshot(
    state: &AppState,
    doc_id: &str,
    version: u64,
    current_version: u64,
    current_s3_key: Option<&str>,
) -> Result<(String, Option<ogrenotes_storage::models::snapshot::DocSnapshot>), ApiError> {
    if version == current_version {
        let key = current_s3_key
            .map(|k| k.to_string())
            .ok_or_else(|| ApiError::NotFound("No snapshot for this version".to_string()))?;
        Ok((key, None))
    } else {
        let snap = state
            .snapshot_repo
            .get(doc_id, version)
            .await?
            .ok_or_else(|| ApiError::NotFound("Version not found".to_string()))?;
        Ok((snap.s3_key.clone(), Some(snap)))
    }
}

/// GET /documents/:id/versions/:v1/diff/:v2 — compute diff between two versions.
async fn diff_versions(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, v1, v2)): Path<(String, u64, u64)>,
) -> Result<Json<DiffResponse>, ApiError> {
    if v1 == v2 {
        return Err(ApiError::BadRequest("Cannot diff a version against itself".to_string()));
    }

    let meta = super::documents::check_doc_access(
        &state, &id, &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    ).await?;
    super::documents::enforce_view_link_option(
        &state, &meta, &user_id, meta.link_view_options.show_history,
    )
    .await?;

    let (key_v1, _snap_v1) = resolve_snapshot(&state, &id, v1, meta.snapshot_version, meta.snapshot_s3_key.as_deref()).await?;
    let (key_v2, snap_v2) = resolve_snapshot(&state, &id, v2, meta.snapshot_version, meta.snapshot_s3_key.as_deref()).await?;

    let bytes_v1 = state.doc_repo.s3().get_object(&key_v1).await
        .map_err(|e| ApiError::Internal(format!("Failed to load snapshot v{v1}: {e}")))?;
    let bytes_v2 = state.doc_repo.s3().get_object(&key_v2).await
        .map_err(|e| ApiError::Internal(format!("Failed to load snapshot v{v2}: {e}")))?;

    let old_doc = OgreDoc::from_state_bytes(&bytes_v1)?;
    let new_doc = OgreDoc::from_state_bytes(&bytes_v2)?;

    // Per-snapshot attribution: every entry in this diff is stamped with
    // the author + timestamp of v2's snapshot row. If v2 is the live
    // version (no SNAPSHOT# row written yet), fall back to the meta's
    // updated_at and leave user_id None — DocumentMeta does not track
    // last editor at the doc level, so we honestly report "unknown" there
    // rather than guessing. The SNAPSHOT# row was already fetched by
    // `resolve_snapshot` above; reusing it here avoids a second DynamoDB
    // read on the non-live path.
    let (v2_user_id, v2_ts) = match snap_v2 {
        Some(snap) => (Some(snap.user_id), Some(snap.created_at)),
        None => (None, Some(meta.updated_at)),
    };

    let diffs = ogrenotes_collab::diff::diff_documents_attributed(
        old_doc.inner(),
        new_doc.inner(),
        v2_user_id,
        v2_ts,
    );

    Ok(Json(DiffResponse { diffs }))
}

/// POST /documents/:id/versions/:version/restore — restore to a previous version.
async fn restore_version(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, version)): Path<(String, u64)>,
) -> Result<StatusCode, ApiError> {
    let meta = super::documents::check_doc_access(
        &state, &id, &user_id,
        ogrenotes_storage::models::AccessLevel::Edit,
    ).await?;

    if version == meta.snapshot_version {
        return Ok(StatusCode::NO_CONTENT);
    }

    let snapshot = state
        .snapshot_repo
        .get(&id, version)
        .await?
        .ok_or_else(|| ApiError::NotFound("Version not found".to_string()))?;

    let snapshot_bytes = state
        .doc_repo
        .s3()
        .get_object(&snapshot.s3_key)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to load snapshot: {e}")))?;

    // Write S3 first (orphaned blob is harmless), then optimistic-locked
    // version bump. The repo owns the S3 write, the conditional DynamoDB
    // bump, and the best-effort SNAPSHOT# row.
    let new_version = meta.snapshot_version + 1;
    let outcome = state
        .doc_repo
        .save_snapshot_conditional(
            &id,
            &snapshot_bytes,
            meta.snapshot_version,
            new_version,
            now_usec(),
            &user_id,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if outcome == SnapshotWrite::VersionConflict {
        return Err(ApiError::Conflict(
            "Document was modified concurrently -- reload and retry".to_string(),
        ));
    }

    // A restore discards content newer than the target version. Any
    // UPDATE# rows recorded since the prior snapshot (e.g. a live WS
    // editing session that never went idle to compact) are NOT part of
    // the restored bytes and would otherwise be replayed on top of them
    // by every read path (`load_current_doc_state`, WS cold-load),
    // silently reinstating exactly what the user reverted. Drop them —
    // the same fix `import_file` applies for its wholesale-replace path.
    let _ = state.doc_repo.delete_updates_before(&id, now_usec()).await;

    // Record activity event
    let activity_repo = state.activity_repo.clone();
    let act_doc_id = id.clone();
    let act_user_id = user_id.clone();
    tokio::spawn(async move {
        let activity = ogrenotes_storage::models::activity::Activity {
            activity_id: nanoid::nanoid!(16),
            doc_id: act_doc_id,
            event_type: ogrenotes_storage::models::activity::ActivityEventType::Restore,
            actor_id: act_user_id,
            detail: serde_json::json!({
                "fromVersion": version,
                "toVersion": new_version,
            }).to_string(),
            created_at: ogrenotes_common::time::now_usec(),
        };
        let _ = activity_repo.create(&activity).await;
    });

    // Re-index with restored content
    super::documents::spawn_index_document_from_bytes(
        &state, meta, snapshot_bytes.to_vec(),
    );

    Ok(StatusCode::NO_CONTENT)
}
