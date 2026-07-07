// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.2 piece D — frontend client for the document
//! knowledge-graph CRUD endpoints. Backend lives at
//! `crates/api/src/routes/relationships.rs`:
//!
//!   POST   /documents/:id/relationships              { targetDocId, relationType }
//!   GET    /documents/:id/relationships              [{ sourceDocId, targetDocId, relationType, … }]
//!   DELETE /documents/:id/relationships/:type/:tid   204
//!
//! The relation_type wire shape matches the backend's
//! `RelationType::as_str` — lowercase, hyphenated for the
//! two-word variants. Mirror the enum here so the UI can render
//! human-readable labels without an extra round trip.

use serde::{Deserialize, Serialize};

use super::client::{api_get, api_post_empty, http_error, ApiClientError, API_BASE};
use gloo_net::http::Request;

/// Mirror of `ogrenotes_storage::models::document::RelationType`.
/// String literals match the backend's `RelationType::as_str` so
/// `serde(rename_all)` isn't enough — explicit per-variant rename
/// for the hyphenated wire form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationType {
    #[serde(rename = "implements")]
    Implements,
    #[serde(rename = "derived-from")]
    DerivedFrom,
    #[serde(rename = "depends-on")]
    DependsOn,
    #[serde(rename = "references")]
    References,
    #[serde(rename = "supersedes")]
    Supersedes,
}

impl RelationType {
    pub fn as_str(self) -> &'static str {
        match self {
            RelationType::Implements => "implements",
            RelationType::DerivedFrom => "derived-from",
            RelationType::DependsOn => "depends-on",
            RelationType::References => "references",
            RelationType::Supersedes => "supersedes",
        }
    }

    /// All variants in render order — used by the picker UI's
    /// type dropdown.
    pub fn all() -> &'static [RelationType] {
        &[
            RelationType::Implements,
            RelationType::DerivedFrom,
            RelationType::DependsOn,
            RelationType::References,
            RelationType::Supersedes,
        ]
    }

    /// Localized human-readable label for the picker dropdown
    /// and the list rows. Matches inline because `t!()` only
    /// accepts literal keys; the per-variant compile-time match
    /// is the standard idiom in this codebase
    /// (see formula_keyboard.rs::tab_label).
    pub fn label(self) -> String {
        match self {
            RelationType::Implements => crate::t!("relation-type-implements"),
            RelationType::DerivedFrom => crate::t!("relation-type-derived-from"),
            RelationType::DependsOn => crate::t!("relation-type-depends-on"),
            RelationType::References => crate::t!("relation-type-references"),
            RelationType::Supersedes => crate::t!("relation-type-supersedes"),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateRequest<'a> {
    target_doc_id: &'a str,
    relation_type: RelationType,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipDto {
    pub source_doc_id: String,
    pub target_doc_id: String,
    pub relation_type: String,
    pub created_by: String,
    pub created_at: i64,
}

/// `POST /documents/:id/relationships`. Returns 201 with no body.
pub async fn create(
    doc_id: &str,
    target_doc_id: &str,
    relation_type: RelationType,
) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/documents/{doc_id}/relationships"),
        &CreateRequest {
            target_doc_id,
            relation_type,
        },
    )
    .await
}

/// `GET /documents/:id/relationships[?type=...]`. Permission-
/// filtered server-side — returns only targets the caller can
/// view.
pub async fn list(
    doc_id: &str,
    relation_type: Option<RelationType>,
) -> Result<Vec<RelationshipDto>, ApiClientError> {
    let qs = relation_type
        .map(|t| format!("?type={}", t.as_str()))
        .unwrap_or_default();
    api_get(&format!("/documents/{doc_id}/relationships{qs}")).await
}

/// `DELETE /documents/:id/relationships/:type/:target_id`. Returns
/// 204 with no body. `api_post_empty` is JSON-only, so this is
/// hand-rolled via gloo's Request.
pub async fn delete(
    doc_id: &str,
    relation_type: RelationType,
    target_doc_id: &str,
) -> Result<(), ApiClientError> {
    let url = format!(
        "{API_BASE}/documents/{doc_id}/relationships/{}/{target_doc_id}",
        relation_type.as_str()
    );
    let mut req = Request::delete(&url);
    if let Some(token) = super::client::get_token() {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;
    let status = resp.status();
    if (200..300).contains(&status) {
        Ok(())
    } else if status == 401 {
        Err(ApiClientError::Unauthorized)
    } else {
        Err(http_error(&resp))
    }
}
