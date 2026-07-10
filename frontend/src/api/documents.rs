// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use serde::{Deserialize, Serialize};

use super::client::{api_delete, api_get, api_get_bytes, api_patch, api_post, api_post_empty, api_post_multipart, api_put, api_put_bytes, ApiClientError};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentResponse {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub folder_id: Option<String>,
    pub doc_type: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// True when the document is currently in the owner's Trash. Drives the
    /// read-only banner on the document page.
    #[serde(default)]
    pub is_deleted: bool,
    /// #110: true when the current user is a view-only viewer on a doc whose
    /// link invites edit-access requests. Drives the "Request edit access"
    /// affordance; the backend is the authority on eligibility.
    #[serde(default)]
    pub can_request_access: bool,
    /// #111: the caller's effective write authority. Drives the editor's
    /// read-only state — a View-only user gets a read-only editor even
    /// though the WS now delivers live updates. Defaults to `true` so an
    /// older server that omits the field keeps the editable behavior.
    #[serde(default = "ret_true")]
    pub can_edit: bool,
    /// #144: whether the current user has starred this document.
    #[serde(default)]
    pub is_favorite: bool,
    /// #140: whether the document is locked for editing (doc-wide freeze).
    /// Drives the read-only editor + lock banner — the editor reads
    /// `locked || !can_edit`. Defaults to `false` for older servers.
    #[serde(default)]
    pub locked: bool,
    /// #140: whether the caller may toggle the lock (true iff they own the
    /// doc). Gates whether the Format-menu "Lock Edits" control is shown.
    #[serde(default)]
    pub can_manage: bool,
    /// #142: whether the doc is marked as a template. Drives the Document-menu
    /// label ("Mark as Template" vs "Unmark Template") and the gallery badge.
    #[serde(default)]
    pub is_template: bool,
}

/// Serde default for `DocumentResponse::can_edit` — see its doc comment.
fn ret_true() -> bool {
    true
}

/// #140: set or clear the document's edit-lock. Owner-only on the server
/// (PUT /documents/{id}/lock); a non-owner caller gets 403.
pub async fn set_document_lock(id: &str, locked: bool) -> Result<(), ApiClientError> {
    api_put(&format!("/documents/{id}/lock"), &serde_json::json!({ "locked": locked })).await
}

// ─── Templates (#142) ─────────────────────────────────────────────

/// #142: mark or unmark the document as a template. Edit-gated on the server;
/// when marking `true`, the server also auto-locks the doc if the caller owns it.
pub async fn set_document_template(id: &str, is_template: bool) -> Result<(), ApiClientError> {
    api_put(
        &format!("/documents/{id}/template"),
        &serde_json::json!({ "isTemplate": is_template }),
    )
    .await
}

/// #142: one entry in the workspace template gallery.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateItem {
    pub id: String,
    pub title: String,
    pub owner_id: String,
    pub doc_type: String,
    pub updated_at: i64,
    /// Phase 2: unique placeholder keys the template body uses. When
    /// non-empty, the picker shows a "Fill in template values" step
    /// before copying; empty → straight copy.
    #[serde(default)]
    pub placeholders: Vec<String>,
    /// Phase 3: which section this row belongs to in the picker gallery.
    /// The server tags each row; the picker groups them into headed
    /// sections (Your templates / Shared with you / Samples).
    ///
    /// `Option` rather than a defaulted enum: an older server that omits
    /// the field, a proxy that strips unknowns, or a wire-format drift
    /// would silently mislabel every row as `Mine` (samples appearing
    /// under "Your templates" with no visible error). Keeping the None
    /// state observable lets the picker fall back to a flat-list render
    /// AND log a console warning so the drift shows up in bug reports.
    #[serde(default)]
    pub gallery: Option<Gallery>,
}

/// Phase 3/4 gallery tag. Matches the server's tagged `Gallery` enum shape:
///
/// - `{"type": "mine"}` / `"shared"` / `"sample"`
/// - `{"type": "company", "galleryId": "...", "galleryName": "..."}`
///
/// The Company variant carries the admin-curated gallery's id and display
/// name so the picker can group rows by their gallery name.
///
/// `Unknown` is the `#[serde(other)]` catch-all: without it, a rolling deploy
/// that ships a new server variant (e.g. `Featured`) would hard-error the
/// whole `TemplateItem` deserialize on the old wasm bundle — `Option::default`
/// only rescues a missing field, not an unknown internally-tagged variant.
/// Unknown rows route through the same fallback bucket as `None`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Gallery {
    Mine,
    Shared,
    Sample,
    #[serde(rename_all = "camelCase")]
    Company {
        gallery_id: String,
        gallery_name: String,
    },
    #[serde(other)]
    Unknown,
}

/// #142: templates the caller can use, scoped to their default workspace.
pub async fn list_templates() -> Result<Vec<TemplateItem>, ApiClientError> {
    api_get("/documents/templates").await
}

/// #142: body for `POST /documents/{id}/copy`. All fields optional;
/// `values` is reserved for Phase 2 mail-merge substitution and unused
/// in v1 (kept on the wire so the modal can already collect the dict
/// without a coupled backend change).
#[derive(Serialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CopyDocumentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<serde_json::Value>,
}

/// #142: duplicate a document the caller can read. Returns the new doc's
/// metadata; the caller typically navigates to `/d/{id}/{slug}` on success.
pub async fn copy_document(
    src_id: &str,
    req: &CopyDocumentRequest,
) -> Result<DocumentResponse, ApiClientError> {
    api_post(&format!("/documents/{src_id}/copy"), req).await
}

/// #144: a starred document, as returned by `GET /documents/favorites`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteItem {
    pub id: String,
    pub title: String,
    pub doc_type: String,
    pub updated_at: i64,
}

/// Star a document for the current user (#144).
pub async fn add_favorite(id: &str) -> Result<(), ApiClientError> {
    api_put(&format!("/documents/{id}/favorite"), &serde_json::json!({})).await
}

/// Unstar a document (#144).
pub async fn remove_favorite(id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/documents/{id}/favorite")).await
}

/// The current user's starred documents (#144).
pub async fn list_favorites() -> Result<Vec<FavoriteItem>, ApiClientError> {
    api_get("/documents/favorites").await
}

// ─── Collections (#144) — named groups within Favorites ──────────

/// A collection with its accessible docs inlined (`GET /documents/collections`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionWithItems {
    pub id: String,
    pub name: String,
    pub items: Vec<FavoriteItem>,
}

/// A collection plus whether the current doc is in it
/// (`GET /documents/{id}/collections`). Drives the star-dropdown checkmarks.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionMembership {
    pub id: String,
    pub name: String,
    pub contains: bool,
}

/// Result of creating a collection.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedCollection {
    pub id: String,
    pub name: String,
}

/// All of the caller's collections, each with its docs (for the sidebar).
pub async fn list_collections() -> Result<Vec<CollectionWithItems>, ApiClientError> {
    api_get("/documents/collections").await
}

/// Collections + membership of `doc_id` (for the star dropdown).
pub async fn list_doc_collections(doc_id: &str) -> Result<Vec<CollectionMembership>, ApiClientError> {
    api_get(&format!("/documents/{doc_id}/collections")).await
}

/// Create a new collection containing `doc_id` ("New Collection…").
pub async fn create_doc_collection(doc_id: &str, name: &str) -> Result<CreatedCollection, ApiClientError> {
    api_post(&format!("/documents/{doc_id}/collections"), &serde_json::json!({ "name": name })).await
}

/// Add `doc_id` to an existing collection.
pub async fn add_doc_to_collection(doc_id: &str, collection_id: &str) -> Result<(), ApiClientError> {
    api_put(&format!("/documents/{doc_id}/collections/{collection_id}"), &serde_json::json!({})).await
}

/// Remove `doc_id` from a collection.
pub async fn remove_doc_from_collection(doc_id: &str, collection_id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/documents/{doc_id}/collections/{collection_id}")).await
}

/// #149: a folder a document belongs to. `is_primary` marks the doc's home
/// folder (changed via Move); the rest are additional memberships.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocFolder {
    pub id: String,
    pub title: String,
    pub is_primary: bool,
}

/// #149: list the folders a document is in (primary first).
pub async fn list_doc_folders(doc_id: &str) -> Result<Vec<DocFolder>, ApiClientError> {
    api_get(&format!("/documents/{doc_id}/folders")).await
}

/// #149: add the document to an additional folder.
pub async fn add_doc_to_folder(doc_id: &str, folder_id: &str) -> Result<(), ApiClientError> {
    api_put(
        &format!("/documents/{doc_id}/folders/{folder_id}"),
        &serde_json::json!({}),
    )
    .await
}

/// #149: remove the document from an additional folder (not the primary).
pub async fn remove_doc_from_folder(doc_id: &str, folder_id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/documents/{doc_id}/folders/{folder_id}")).await
}

/// Delete a whole collection.
pub async fn delete_collection(collection_id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/documents/collections/{collection_id}")).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateDocumentRequest {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    doc_type: Option<String>,
}

pub async fn create_document(
    title: &str,
    folder_id: Option<&str>,
) -> Result<DocumentResponse, ApiClientError> {
    create_document_with_type(title, folder_id, None).await
}

/// Body shape for POST /documents/import — Phase 5 M-P5 piece A/B.
/// `format` accepts `"markdown" | "md" | "html"`. Body cap is 1 MB
/// on the server side; the home-page drop UI enforces the same
/// limit client-side before the round-trip.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportTextRequest {
    format: String,
    title: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder_id: Option<String>,
}

/// Create a new document by importing Markdown or HTML source.
/// Wraps POST /api/v1/documents/import. Returns the new doc's
/// metadata; the home-page drop UI navigates to `/d/{id}/{slug}`
/// on success.
pub async fn import_document_from_text(
    format: &str,
    title: &str,
    content: &str,
    folder_id: Option<&str>,
) -> Result<DocumentResponse, ApiClientError> {
    let body = ImportTextRequest {
        format: format.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder_id: folder_id.map(|s| s.to_string()),
    };
    api_post("/documents/import", &body).await
}

/// 202 response from POST /documents/import-job.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportJobAccepted {
    job_id: String,
}

/// Async binary import (M-6.5 / M-6.6 piece D). Streams the file to the
/// import-job endpoint as multipart, then polls `GET /jobs/{id}` until
/// the worker reaches a terminal state, returning the created
/// document's id. Format-agnostic: the server picks the parser from the
/// upload's filename (`.docx` or `.pdf`). Conversion runs off the
/// request path, so this can take a few seconds; the poll caps at ~60s
/// so an absent or backed-up worker surfaces an error instead of
/// hanging the UI forever.
pub async fn import_document_via_job(
    file: &web_sys::File,
    folder_id: Option<&str>,
) -> Result<String, ApiClientError> {
    // Multipart body. The filename must be present and carry a supported
    // extension — the server rejects a field with no filename, and
    // derives the document title from it.
    let form = web_sys::FormData::new()
        .map_err(|e| ApiClientError::Network(format!("FormData init failed: {e:?}")))?;
    form.append_with_blob_and_filename("file", file, &file.name())
        .map_err(|e| ApiClientError::Network(format!("FormData append failed: {e:?}")))?;

    let mut path = "/documents/import-job".to_string();
    if let Some(fid) = folder_id {
        path.push_str(&format!("?folderId={fid}"));
    }

    let accepted: ImportJobAccepted = api_post_multipart(&path, form).await?;

    let job_path = format!("/jobs/{}", accepted.job_id);
    // ~500ms cadence, 120 attempts = ~60s ceiling.
    for _ in 0..120 {
        gloo_timers::future::TimeoutFuture::new(500).await;
        let status: serde_json::Value = api_get(&job_path).await?;
        match status.get("state").and_then(|s| s.as_str()) {
            Some("succeeded") => return import_result_doc_id(&status),
            Some("failed") => {
                // #47: `error` here is a field on the import job's
                // structured status DTO (a deserialized 200 response),
                // not a raw HTTP error body — a safe, opt-in detail, so
                // it's fine to surface to the user.
                let err = status
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("import failed");
                return Err(ApiClientError::Http(422, Some(err.to_string())));
            }
            // pending / running / unrecognized → keep polling.
            _ => {}
        }
    }
    Err(ApiClientError::Network(
        "import timed out waiting for the converter".to_string(),
    ))
}

/// Extract the created document id from a terminal `succeeded` job
/// status. The result field is a JSON *string* carrying { "docId": ... }.
///
/// Field name is snake_case on purpose: the worker's `JobStatus` enum
/// camelCases its `state` *tag* only — serde's enum-level `rename_all`
/// does not touch variant fields, so the wire carries `result_json`
/// (issue #9).
fn import_result_doc_id(status: &serde_json::Value) -> Result<String, ApiClientError> {
    let result_json = status
        .get("result_json")
        .and_then(|r| r.as_str())
        .unwrap_or("");
    let parsed: serde_json::Value = serde_json::from_str(result_json)
        .map_err(|e| ApiClientError::Deserialize(e.to_string()))?;
    parsed
        .get("docId")
        .and_then(|d| d.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ApiClientError::Deserialize("import result missing docId".to_string()))
}

/// M-P6 piece B — call the backend embed resolver. Returns the
/// iframe-ready src + provider tag + provider-default height when
/// the URL is accepted by the allowlist; ApiClientError::Http with
/// 400 + a reason string otherwise. The toolbar's Insert Embed
/// button calls this before dispatching `ToolbarCommand::InsertEmbed`
/// — keeps URL-validation logic on one side (backend) instead of
/// duplicating the matcher in WASM.
#[derive(Serialize)]
struct ResolveEmbedRequest {
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveEmbedResponse {
    pub provider: String,
    pub src: String,
    pub height: u32,
}

pub async fn resolve_embed(url: &str) -> Result<ResolveEmbedResponse, ApiClientError> {
    let body = ResolveEmbedRequest {
        url: url.to_string(),
    };
    api_post("/documents/embeds/resolve", &body).await
}

// ─── Bulk operations (Phase 5 M-P7 piece C) ──────────────────────

/// Per-id outcome row in a bulk-op response body. Mirrors the
/// backend's `BulkOpResultEntry`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkOpResultEntry {
    pub doc_id: String,
    pub status: u16,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkOpResponse {
    pub results: Vec<BulkOpResultEntry>,
    pub succeeded: usize,
    pub failed: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BulkDocIdsRequest {
    doc_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BulkRestoreRequest {
    doc_ids: Vec<String>,
    target_folder_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BulkMoveRequest {
    doc_ids: Vec<String>,
    dest_folder_id: String,
}

pub async fn bulk_delete(doc_ids: Vec<String>) -> Result<BulkOpResponse, ApiClientError> {
    api_post("/documents/bulk/delete", &BulkDocIdsRequest { doc_ids }).await
}

pub async fn bulk_restore(
    doc_ids: Vec<String>,
    target_folder_id: &str,
) -> Result<BulkOpResponse, ApiClientError> {
    api_post(
        "/documents/bulk/restore",
        &BulkRestoreRequest {
            doc_ids,
            target_folder_id: target_folder_id.to_string(),
        },
    )
    .await
}

pub async fn bulk_move(
    doc_ids: Vec<String>,
    dest_folder_id: &str,
) -> Result<BulkOpResponse, ApiClientError> {
    api_post(
        "/documents/bulk/move",
        &BulkMoveRequest {
            doc_ids,
            dest_folder_id: dest_folder_id.to_string(),
        },
    )
    .await
}

pub async fn create_spreadsheet(
    title: &str,
    folder_id: Option<&str>,
) -> Result<DocumentResponse, ApiClientError> {
    create_document_with_type(title, folder_id, Some("spreadsheet")).await
}

async fn create_document_with_type(
    title: &str,
    folder_id: Option<&str>,
    doc_type: Option<&str>,
) -> Result<DocumentResponse, ApiClientError> {
    let body = CreateDocumentRequest {
        title: title.to_string(),
        folder_id: folder_id.map(|s| s.to_string()),
        doc_type: doc_type.map(|s| s.to_string()),
    };
    api_post("/documents", &body).await
}

pub async fn get_document(id: &str) -> Result<DocumentResponse, ApiClientError> {
    api_get(&format!("/documents/{id}")).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDocumentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

pub async fn update_document_title(id: &str, title: &str) -> Result<(), ApiClientError> {
    let body = UpdateDocumentRequest {
        title: Some(title.to_string()),
    };
    api_patch(&format!("/documents/{id}"), &body).await
}

pub async fn delete_document(id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/documents/{id}")).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreDocumentRequest {
    target_folder_id: String,
}

/// Restore a trashed document into a target folder owned by the user.
/// The server returns 204 No Content, so we use `api_post_empty` to avoid
/// `api_post`'s mandatory JSON deserialize on an empty body.
pub async fn restore_document(id: &str, target_folder_id: &str) -> Result<(), ApiClientError> {
    let body = RestoreDocumentRequest {
        target_folder_id: target_folder_id.to_string(),
    };
    api_post_empty(&format!("/documents/{id}/restore"), &body).await
}

/// Permanently delete a trashed document (hard-delete: DynamoDB rows, S3
/// blobs, reverse-relationships, search index).
pub async fn purge_document(id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/documents/{id}/purge")).await
}

pub async fn get_content(id: &str) -> Result<Vec<u8>, ApiClientError> {
    api_get_bytes(&format!("/documents/{id}/content")).await
}

pub async fn put_content(id: &str, data: &[u8]) -> Result<(), ApiClientError> {
    api_put_bytes(&format!("/documents/{id}/content"), data).await
}

/// Fetches an exported document and returns the raw response bytes.
/// All export formats — text (markdown/html/csv) and binary
/// (xlsx/docx/pdf) — flow through this same shape; the caller picks
/// the filename + extension. Bytes (not a typed body) because the
/// route only authenticates via the bearer header, so the caller
/// must hand the result to a client-side Blob download — a fresh
/// `window.open` navigation has no way to carry the token.
pub async fn export_document(id: &str, format: &str) -> Result<Vec<u8>, ApiClientError> {
    api_get_bytes(&format!("/documents/{id}/export/{format}")).await
}

#[derive(Deserialize)]
pub struct WsTokenResponse {
    pub token: String,
}

/// Request a single-use WebSocket authentication token for a document.
/// Sends the client version so the server can tag each update for triage.
pub async fn request_ws_token(id: &str) -> Result<WsTokenResponse, ApiClientError> {
    api_post(&format!("/documents/{id}/ws-token"), &serde_json::json!({
        "clientVersion": env!("CARGO_PKG_VERSION")
    })).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: issue #9 — the job-status DTO's enum-level camelCase
    /// rename covers the `state` tag only; variant *fields* stay
    /// snake_case on the wire. A succeeded import must be parsed from
    /// the shape the API actually returns.
    #[test]
    fn import_result_parses_actual_wire_shape() {
        let status = serde_json::json!({
            "state": "succeeded",
            "started_at_ms": 1,
            "finished_at_ms": 2,
            "result_json": "{\"docId\":\"d1\"}",
        });
        assert_eq!(import_result_doc_id(&status).unwrap(), "d1");
    }
}
