// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::document::RelationType;
use ogrenotes_storage::models::AccessLevel;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::documents::check_doc_access;

/// Router for document relationship endpoints.
/// Merged into the documents nest: /documents/{id}/relationships
pub fn doc_relationships_router() -> Router<AppState> {
    Router::new()
        .route(
            "/{id}/relationships",
            post(create_relationship).get(list_relationships),
        )
        .route(
            "/{id}/relationships/{relation_type}/{target_id}",
            delete(delete_relationship),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRelationshipRequest {
    target_doc_id: String,
    relation_type: RelationType,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RelationshipResponse {
    source_doc_id: String,
    target_doc_id: String,
    relation_type: String,
    created_by: String,
    created_at: i64,
}

#[derive(Deserialize)]
struct ListRelParams {
    #[serde(rename = "type")]
    relation_type: Option<RelationType>,
}

/// POST /documents/:id/relationships
async fn create_relationship(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<CreateRelationshipRequest>,
) -> Result<StatusCode, ApiError> {
    if id == req.target_doc_id {
        return Err(ApiError::BadRequest(
            "Cannot create a relationship from a document to itself".to_string(),
        ));
    }

    // Require Edit on source document
    check_doc_access(&state, &id, &user_id, AccessLevel::Edit).await?;
    // Require View on target document (user must be able to see it)
    check_doc_access(&state, &req.target_doc_id, &user_id, AccessLevel::View).await?;

    let rel = ogrenotes_storage::models::document::DocRelationship {
        source_doc_id: id,
        target_doc_id: req.target_doc_id,
        relation_type: req.relation_type,
        created_by: user_id,
        created_at: now_usec(),
    };

    state.doc_repo.create_relationship(&rel).await?;

    Ok(StatusCode::CREATED)
}

/// GET /documents/:id/relationships?type=...
async fn list_relationships(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Query(params): Query<ListRelParams>,
) -> Result<Json<Vec<RelationshipResponse>>, ApiError> {
    check_doc_access(&state, &id, &user_id, AccessLevel::View).await?;

    let rels = state
        .doc_repo
        .list_relationships(&id, params.relation_type.as_ref())
        .await?;

    // Permission-filter: only include relationships where user can see the target
    let mut results = Vec::with_capacity(rels.len());
    for rel in rels {
        if check_doc_access(&state, &rel.target_doc_id, &user_id, AccessLevel::View)
            .await
            .is_ok()
        {
            results.push(RelationshipResponse {
                source_doc_id: rel.source_doc_id,
                target_doc_id: rel.target_doc_id,
                relation_type: rel.relation_type.as_str().to_string(),
                created_by: rel.created_by,
                created_at: rel.created_at,
            });
        }
    }

    Ok(Json(results))
}

/// DELETE /documents/:id/relationships/:relation_type/:target_id
async fn delete_relationship(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, relation_type_str, target_id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    check_doc_access(&state, &id, &user_id, AccessLevel::Edit).await?;

    let relation_type = RelationType::from_str(&relation_type_str)
        .ok_or_else(|| ApiError::BadRequest(format!("Invalid relation type: {relation_type_str}")))?;

    state
        .doc_repo
        .delete_relationship(&id, &relation_type, &target_id)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
