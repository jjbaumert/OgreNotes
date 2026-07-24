// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Mention resolution (block-links/mentions feature, Task 2).
//!
//! Batch-resolves mention targets to live titles/snippets. Access gating
//! runs BEFORE any lookup, and — deliberately unlike the document
//! endpoints' 403-vs-404 policy — an inaccessible target is per-target
//! byte-identical to a nonexistent one, so mention resolution can never
//! leak a document's title or its existence.

use axum::extract::State;
use axum::{routing::post, Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::documents::{check_doc_access, load_current_doc_state};

const SNIPPET_MAX_CHARS: usize = 120;
const MAX_TARGETS: usize = 100;

pub fn router() -> Router<AppState> {
    Router::new().route("/resolve", post(resolve))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveRequest {
    targets: Vec<ResolveTarget>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveTarget {
    doc_id: String,
    #[serde(default)]
    block_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolveResponse {
    results: Vec<ResolveResult>,
}

#[derive(Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
struct ResolveResult {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_found: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
}

impl ResolveResult {
    fn not_found() -> Self {
        Self {
            status: "notFound",
            title: None,
            block_found: None,
            snippet: None,
        }
    }
}

async fn resolve(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<ResolveRequest>,
) -> Result<Json<ResolveResponse>, ApiError> {
    if req.targets.len() > MAX_TARGETS {
        return Err(ApiError::BadRequest(format!(
            "too many targets (max {MAX_TARGETS})"
        )));
    }
    let mut results = Vec::with_capacity(req.targets.len());
    for target in &req.targets {
        results.push(resolve_target(&state, &user_id, target).await?);
    }
    Ok(Json(ResolveResponse { results }))
}

async fn resolve_target(
    state: &AppState,
    user_id: &str,
    target: &ResolveTarget,
) -> Result<ResolveResult, ApiError> {
    // Gate first. Forbidden and NotFound collapse to the same result;
    // infrastructure errors still surface as 500 rather than masquerading
    // as a missing document.
    let meta = match check_doc_access(
        state,
        &target.doc_id,
        user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await
    {
        Ok(meta) => meta,
        Err(ApiError::NotFound(_) | ApiError::Forbidden | ApiError::ForbiddenMsg(_)) => {
            return Ok(ResolveResult::not_found());
        }
        Err(other) => return Err(other),
    };

    let (block_found, snippet) = match &target.block_id {
        None => (false, None),
        Some(block_id) => {
            let doc = load_current_doc_state(state, &target.doc_id).await?;
            match ogrenotes_collab::diff::block_plain_text(doc.inner(), block_id, SNIPPET_MAX_CHARS)
            {
                Some(text) => (true, Some(text)),
                None => (false, None),
            }
        }
    };

    Ok(ResolveResult {
        status: "ok",
        title: Some(meta.title),
        block_found: Some(block_found),
        snippet,
    })
}
