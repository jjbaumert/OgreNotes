// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Typed Leptos/WASM client for workspace-scoped SAML IdP config
//! (Phase 4 M-E4 piece E). Mirrors the wire DTOs in
//! `crates/api/src/routes/workspaces.rs`. Owner / workspace-admin
//! only; non-admins see 403.

use serde::{Deserialize, Serialize};

use super::client::{api_delete, api_get, api_put, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamlConfig {
    pub workspace_id: String,
    pub idp_entity_id: String,
    pub idp_metadata_xml: String,
    pub attribute_email: String,
    pub attribute_name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PutSamlConfigRequest {
    pub idp_entity_id: String,
    pub idp_metadata_xml: String,
    pub attribute_email: String,
    pub attribute_name: String,
}

/// GET /workspaces/:id/saml-config. Returns `None` if the workspace
/// has no SAML config; `Some(_)` if one is set. 403 if the caller
/// isn't a workspace admin/owner.
pub async fn get_config(workspace_id: &str) -> Result<Option<SamlConfig>, ApiClientError> {
    api_get(&format!("/workspaces/{workspace_id}/saml-config")).await
}

pub async fn put_config(
    workspace_id: &str,
    req: &PutSamlConfigRequest,
) -> Result<(), ApiClientError> {
    api_put(&format!("/workspaces/{workspace_id}/saml-config"), req).await
}

pub async fn delete_config(workspace_id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/workspaces/{workspace_id}/saml-config")).await
}
