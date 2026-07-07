// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use serde::{Deserialize, Serialize};
use super::client::{api_get, api_patch, api_post_empty, api_delete, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberItem {
    pub user_id: String,
    pub name: String,
    #[serde(default)]
    pub email: String,
    pub access_level: String,
    pub added_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MembersListResponse {
    pub members: Vec<MemberItem>,
}

pub async fn list_members(folder_id: &str) -> Result<MembersListResponse, ApiClientError> {
    api_get(&format!("/folders/{folder_id}/members")).await
}

pub async fn add_member(
    folder_id: &str,
    user_id: &str,
    access_level: &str,
) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/folders/{folder_id}/members"),
        &serde_json::json!({
            "userId": user_id,
            "accessLevel": access_level
        }),
    )
    .await
}

pub async fn remove_member(folder_id: &str, user_id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/folders/{folder_id}/members/{user_id}")).await
}

// ─── Document link sharing ──────────────────────────────────────

/// View-mode sub-options for a link-shared document. Mirrors the
/// backend `ViewOptions`; serialized camelCase both ways.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewOptions {
    pub allow_comments: bool,
    pub show_history: bool,
    pub show_conversation: bool,
    pub allow_request_access: bool,
}

/// Response of `GET /documents/{id}/link-settings`. `link_sharing_mode`
/// is `"view"` / `"edit"` / `None` (disabled). `can_manage` is true only
/// for the doc owner — non-owners get a read-only view.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkSettings {
    #[serde(default)]
    pub link_sharing_mode: Option<String>,
    #[serde(default)]
    pub view_options: ViewOptions,
    #[serde(default)]
    pub can_manage: bool,
}

pub async fn get_link_settings(doc_id: &str) -> Result<LinkSettings, ApiClientError> {
    api_get(&format!("/documents/{doc_id}/link-settings")).await
}

/// Set the link mode: `"view"`, `"edit"`, or `"none"` (disable). Leaves
/// the view-options untouched (omitted from the PATCH body).
pub async fn set_link_mode(doc_id: &str, mode: &str) -> Result<(), ApiClientError> {
    api_patch(
        &format!("/documents/{doc_id}/link-settings"),
        &serde_json::json!({ "linkSharingMode": mode }),
    )
    .await
}

/// Ask the doc owner for edit access on a View-mode link (§5.4). The
/// backend returns 204 on success; the caller can distinguish the failure
/// modes by matching the returned `ApiClientError::Http(status, _)` —
/// 403 (the link no longer offers requests) vs 429 (rate-limited).
pub async fn request_access(doc_id: &str) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/documents/{doc_id}/request-access"),
        &serde_json::json!({}),
    )
    .await
}

/// Replace the view-mode sub-options. Leaves the mode untouched.
pub async fn set_link_view_options(
    doc_id: &str,
    options: &ViewOptions,
) -> Result<(), ApiClientError> {
    api_patch(
        &format!("/documents/{doc_id}/link-settings"),
        &serde_json::json!({ "viewOptions": options }),
    )
    .await
}
