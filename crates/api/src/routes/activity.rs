// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Document activity feed endpoint.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{doc_id}/activity", get(list_activity))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

/// Default page size for `GET /documents/:doc_id/activity` when the
/// client omits `?limit=N`. Returns the 50 newest activity rows —
/// enough to fill a typical activity-feed pane without paginating, and
/// far below the per-handler hard cap of 200 enforced at line 63.
fn default_limit() -> usize {
    50
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityResponse {
    activity_id: String,
    doc_id: String,
    event_type: String,
    actor_id: String,
    actor_name: String,
    detail: serde_json::Value,
    created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityListResponse {
    activities: Vec<ActivityResponse>,
}

/// GET /documents/:doc_id/activity — list activity events (newest first).
async fn list_activity(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(doc_id): Path<String>,
    Query(params): Query<ActivityQuery>,
) -> Result<Json<ActivityListResponse>, ApiError> {
    let meta = super::documents::check_doc_access(
        &state,
        &doc_id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await?;
    // View-mode link viewers see the conversation/activity pane only when
    // `show_conversation` is on (Phase 2, §5.3).
    super::documents::enforce_view_link_option(
        &state, &meta, &user_id, meta.link_view_options.show_conversation,
    )
    .await?;

    let limit = params.limit.min(200); // cap at 200
    let activities = state.activity_repo.list(&doc_id, limit).await?;

    let mut user_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut responses = Vec::with_capacity(activities.len());
    for a in activities {
        let name = if let Some(cached) = user_names.get(&a.actor_id) {
            cached.clone()
        } else {
            let name = match state.user_repo.get_by_id(&a.actor_id).await {
                Ok(Some(user)) => user.name,
                _ => a.actor_id.clone(),
            };
            user_names.insert(a.actor_id.clone(), name.clone());
            name
        };

        let event_type = serde_json::to_string(&a.event_type)
            .unwrap_or_default()
            .trim_matches('"')
            .to_string();

        let detail = serde_json::from_str(&a.detail).unwrap_or(serde_json::json!({}));

        responses.push(ActivityResponse {
            activity_id: a.activity_id,
            doc_id: a.doc_id,
            event_type,
            actor_id: a.actor_id,
            actor_name: name,
            detail,
            created_at: a.created_at,
        });
    }

    Ok(Json(ActivityListResponse { activities: responses }))
}
