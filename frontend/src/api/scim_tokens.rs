// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Typed Leptos/WASM client for workspace-scoped SCIM tokens
//! (Phase 4 M-E5 piece F). Mirrors the admin endpoints in
//! `crates/api/src/routes/workspaces.rs`. Owner / workspace-admin
//! only; non-admins see 403.

use serde::{Deserialize, Serialize};

use super::client::{api_delete, api_get, api_post, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScimTokenSummary {
    pub token_id: String,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: i64,
    pub disabled_at: i64,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateScimTokenRequest {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedScimToken {
    pub token_id: String,
    /// `<token_id>.<secret>` — shown once. The admin must copy
    /// this immediately; we cannot retrieve it again.
    pub token: String,
    pub name: String,
    pub created_at: i64,
}

pub async fn list_tokens(workspace_id: &str) -> Result<Vec<ScimTokenSummary>, ApiClientError> {
    api_get(&format!("/workspaces/{workspace_id}/scim-tokens")).await
}

pub async fn create_token(
    workspace_id: &str,
    req: &CreateScimTokenRequest,
) -> Result<CreatedScimToken, ApiClientError> {
    api_post(&format!("/workspaces/{workspace_id}/scim-tokens"), req).await
}

pub async fn revoke_token(
    workspace_id: &str,
    token_id: &str,
) -> Result<(), ApiClientError> {
    api_delete(&format!("/workspaces/{workspace_id}/scim-tokens/{token_id}")).await
}
