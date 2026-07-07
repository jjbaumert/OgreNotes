// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::collections::BTreeMap;

use serde::Deserialize;
use super::client::{api_get, api_get_bytes, api_post_empty, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub version: u64,
    pub size_bytes: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionListResponse {
    pub versions: Vec<VersionEntry>,
}

/// Mirror of `ogrenotes_collab::diff::DiffKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffKind {
    Added,
    Removed,
    Modified,
}

/// Mirror of `ogrenotes_collab::diff::Mark`. Marks with payload (link
/// href, color) carry the value inline so the renderer doesn't have to
/// parse a JSON blob.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Link { href: String },
    TextColor { color: String },
    Highlight { color: String },
    Subscript,
    Superscript,
}

/// Mirror of `ogrenotes_collab::diff::InlineRun`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InlineRun {
    pub text: String,
    pub marks: Vec<Mark>,
}

/// Mirror of `ogrenotes_collab::diff::RichBlock` — a renderable block of
/// text + structural shape. `attrs` excludes `blockId` (split into the
/// sibling field).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RichBlock {
    pub node_type: String,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
    #[serde(default)]
    pub block_id: Option<String>,
    #[serde(default)]
    pub inline: Vec<InlineRun>,
    #[serde(default)]
    pub children: Vec<RichBlock>,
}

/// Mirror of `ogrenotes_collab::diff::DiffEntry`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffEntry {
    pub kind: DiffKind,
    #[serde(default)]
    pub block_id: Option<String>,
    pub block_index: usize,
    pub node_type: String,
    pub blocks: Vec<RichBlock>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResponse {
    pub diffs: Vec<DiffEntry>,
}

pub async fn list_versions(doc_id: &str) -> Result<VersionListResponse, ApiClientError> {
    api_get(&format!("/documents/{doc_id}/versions")).await
}

pub async fn get_version_content(doc_id: &str, version: u64) -> Result<Vec<u8>, ApiClientError> {
    api_get_bytes(&format!("/documents/{doc_id}/versions/{version}")).await
}

/// Compute the attributed block-level diff between two versions. The
/// backend stamps every entry with the author + timestamp of `v2`.
pub async fn diff_versions(
    doc_id: &str,
    v1: u64,
    v2: u64,
) -> Result<DiffResponse, ApiClientError> {
    api_get(&format!("/documents/{doc_id}/versions/{v1}/diff/{v2}")).await
}

/// Restore the document to the given snapshot version. The server writes
/// a fresh snapshot reusing the historical bytes and bumps `snapshot_version`,
/// so the restore itself is auditable as a new version (not a rollback).
pub async fn restore_version(doc_id: &str, version: u64) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/documents/{doc_id}/versions/{version}/restore"),
        &serde_json::json!({}),
    )
    .await
}
