// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Typed Leptos/WASM client for `/api/v1/admin` routes.
//!
//! Mirrors the wire DTOs in `crates/api/src/routes/admin.rs`. Every
//! field rename matches the backend's `#[serde(rename_all =
//! "camelCase")]` so a backend rename surfaces here as a Rust
//! compile error, not a silent runtime "field missing" bug.
//!
//! The server enforces `require_admin` on every route; the
//! `pages/admin/*` route gate is UX-only. Calling these helpers from
//! a non-admin context returns `ApiClientError::Http(403, …)`.

use serde::{Deserialize, Serialize};

use super::client::{api_get, api_post_empty, api_put, ApiClientError};

// ─── User list / detail ───────────────────────────────────────

/// One row in the admin user table. Matches `AdminUserResponse` on
/// the backend.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUser {
    pub id: String,
    pub name: String,
    pub email: String,
    pub is_admin: bool,
    pub is_disabled: bool,
    pub last_active_at: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserList {
    pub users: Vec<AdminUser>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// `GET /admin/users` — paginated list with optional email-prefix
/// filter. `cursor` is the opaque cursor returned by a prior page;
/// `email_prefix` is case-insensitive and matched against the start
/// of `User.email`. Both filters apply server-side.
pub async fn list_users(
    cursor: Option<&str>,
    email_prefix: Option<&str>,
) -> Result<AdminUserList, ApiClientError> {
    let mut qs: Vec<String> = Vec::new();
    if let Some(c) = cursor.filter(|s| !s.is_empty()) {
        qs.push(format!("cursor={}", js_sys::encode_uri_component(c)));
    }
    if let Some(p) = email_prefix.filter(|s| !s.is_empty()) {
        qs.push(format!("emailPrefix={}", js_sys::encode_uri_component(p)));
    }
    let path = if qs.is_empty() {
        "/admin/users".to_string()
    } else {
        format!("/admin/users?{}", qs.join("&"))
    };
    api_get(&path).await
}

pub async fn get_user(user_id: &str) -> Result<AdminUser, ApiClientError> {
    api_get(&format!("/admin/users/{user_id}")).await
}

// ─── Mutations ────────────────────────────────────────────────

pub async fn disable_user(user_id: &str) -> Result<(), ApiClientError> {
    // POST with no body — the backend reads only the path parameter.
    // `api_post_empty` requires a JSON-serializable body, so we send
    // `{}` which the handler ignores.
    api_post_empty(
        &format!("/admin/users/{user_id}/disable"),
        &serde_json::json!({}),
    )
    .await
}

pub async fn enable_user(user_id: &str) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/admin/users/{user_id}/enable"),
        &serde_json::json!({}),
    )
    .await
}

pub async fn promote_user(user_id: &str) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/admin/users/{user_id}/promote"),
        &serde_json::json!({}),
    )
    .await
}

pub async fn demote_user(user_id: &str) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/admin/users/{user_id}/demote"),
        &serde_json::json!({}),
    )
    .await
}

// ─── Ask-policy gate (#148) ───────────────────────────────────

/// Three-state AI-assistant access policy. Mirrors
/// `crates/storage/src/models/user.rs::AskPolicy` — same
/// snake_case serialization so both sides speak the same wire.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AskPolicy {
    #[default]
    Disabled,
    SystemOnly,
    SystemOrByok,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskPolicyResponse {
    pub user_id: String,
    pub policy: AskPolicy,
}

#[derive(Debug, Clone, Serialize)]
struct SetAskPolicyRequest {
    policy: AskPolicy,
}

pub async fn get_ask_policy(user_id: &str) -> Result<AskPolicyResponse, ApiClientError> {
    api_get(&format!("/admin/users/{user_id}/ask-policy")).await
}

pub async fn set_ask_policy(user_id: &str, policy: AskPolicy) -> Result<(), ApiClientError> {
    api_put(
        &format!("/admin/users/{user_id}/ask-policy"),
        &SetAskPolicyRequest { policy },
    )
    .await
}

// ─── Metrics ──────────────────────────────────────────────────

/// Loose shape of `GET /admin/metrics`. The backend serializes the
/// in-process `MetricsSnapshot` with three `BTreeMap` fields; the
/// frontend treats each as a flat key/value table for display.
#[derive(Debug, Clone, Deserialize)]
pub struct MetricsSnapshot {
    pub counters: std::collections::BTreeMap<String, u64>,
    pub gauges: std::collections::BTreeMap<String, i64>,
    pub histograms: std::collections::BTreeMap<String, HistogramSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistogramSummary {
    pub count: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
}

pub async fn metrics() -> Result<MetricsSnapshot, ApiClientError> {
    api_get("/admin/metrics").await
}

// ─── Combined audit log ───────────────────────────────────────

/// One row in the merged admin + security audit list. `source` is
/// the discriminator: `"admin"` for AdminAudit-sourced rows,
/// `"security"` for SecurityAudit-sourced rows. `detail` is the
/// action-specific payload, already a JSON object on both sides
/// (the backend deserializes AdminAudit's string-of-json before
/// emitting).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub source: String,
    pub audit_id: String,
    pub actor_id: String,
    pub target_user_id: String,
    pub kind: String,
    pub detail: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuditList {
    pub entries: Vec<AuditEntry>,
}

/// Filter set for `GET /admin/audit`. `target` is required by the
/// backend (both audit tables are user-keyed; a global scan is the
/// v2 carry-forward). Empty / `None` fields are omitted from the
/// query string; mirror the backend's `AuditQuery` shape.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter<'a> {
    pub target: &'a str,
    pub actor: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub limit: Option<usize>,
}

pub async fn list_audit(filter: AuditFilter<'_>) -> Result<AuditList, ApiClientError> {
    let mut qs: Vec<String> = Vec::new();
    qs.push(format!("target={}", js_sys::encode_uri_component(filter.target)));
    if let Some(a) = filter.actor.filter(|s| !s.is_empty()) {
        qs.push(format!("actor={}", js_sys::encode_uri_component(a)));
    }
    if let Some(k) = filter.kind.filter(|s| !s.is_empty()) {
        qs.push(format!("kind={}", js_sys::encode_uri_component(k)));
    }
    if let Some(f) = filter.from {
        qs.push(format!("from={f}"));
    }
    if let Some(t) = filter.to {
        qs.push(format!("to={t}"));
    }
    if let Some(l) = filter.limit {
        qs.push(format!("limit={l}"));
    }
    api_get(&format!("/admin/audit?{}", qs.join("&"))).await
}
