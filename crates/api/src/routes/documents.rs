// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_collab::document::OgreDoc;
use ogrenotes_collab::export;
use ogrenotes_collab::import_spreadsheet;
use ogrenotes_common::id::new_id;
use ogrenotes_common::metrics::{counter, histogram, MetricKey};
use ogrenotes_common::time::now_usec;
use ogrenotes_search::SearchDocument;
use ogrenotes_storage::models::document::{DocMember, DocumentMeta, Favorite};
use ogrenotes_storage::repo::doc_repo::SnapshotWrite;
use ogrenotes_storage::models::folder::FolderChild;
use ogrenotes_storage::models::notification::{NotifType, Notification};
use ogrenotes_storage::models::security_audit::SecurityAuditAction;
use ogrenotes_storage::models::{AccessLevel, ChildType, DocType, FolderType};

use crate::error::ApiError;
use crate::routes::audit::{record_security_event, record_security_event_by_actor};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Maximum document content size: 10 MB.
const MAX_CONTENT_SIZE: usize = 10 * 1024 * 1024;

/// Upper bound on a derived import title (chars). The filename a title
/// can fall back to is attacker-controlled within the multipart body,
/// and the title lands in a DynamoDB attribute — cap it well under the
/// 400 KB item limit.
const MAX_IMPORT_TITLE_CHARS: usize = 512;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_document))
        // Phase 5 M-P5 piece A: text-format import → new doc. Distinct
        // from `/{id}/import` (which overwrites an existing doc with
        // binary XLSX / CSV). Body cap 1 MB — markdown + html source
        // up to ~hundreds of pages.
        .route("/import", post(create_from_text)
            .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)))
        // Phase 6 M-6.5/6.6 piece C: async DOCX/PDF import → new doc.
        // The upload is staged to S3 and an Import{Docx,Pdf} job is
        // enqueued; the client polls GET /jobs/{id} for the result doc
        // id. Conversion happens off the request path because a large
        // file can take seconds to parse. Body cap matches the XLSX
        // import.
        .route("/import-job", post(import_job)
            .layer(axum::extract::DefaultBodyLimit::max(MAX_CONTENT_SIZE)))
        // Phase 5 M-P5 piece C: zip up to 100 docs into a single
        // download. Synchronous; partial failures surface in the
        // archive's `_manifest.json`. > 100 ids → 400.
        .route("/bulk/export", post(bulk_export))
        // Phase 5 M-P7 piece A — bulk soft-delete + bulk restore.
        // Up to 100 ids per call; per-id authz; HTTP 207
        // Multi-Status on partial failures.
        .route("/bulk/delete", post(bulk_delete))
        .route("/bulk/restore", post(bulk_restore))
        // Phase 5 M-P7 piece B — bulk move + bulk share.
        .route("/bulk/move", post(bulk_move))
        .route("/bulk/share", post(bulk_share))
        // Phase 5 M-P6 piece B — resolve a URL against the embed
        // allowlist. Returns the iframe-ready src + provider tag +
        // default height when the URL is accepted; 400 on reject.
        // The frontend insert UX calls this before mutating the
        // editor state so a rejected URL never lands in the doc.
        .route("/embeds/resolve", post(resolve_embed))
        // #144: favorites — list the caller's starred docs (static segment,
        // registered before `/{id}` so it isn't captured as a doc id).
        .route("/favorites", get(list_favorites))
        // #142: templates gallery — caller's workspace-visible templates.
        // Static segment, registered before `/{id}` to avoid capture.
        .route("/templates", get(list_templates))
        // #144: collections — static segments, before `/{id}` for the same reason.
        .route("/collections", get(list_collections))
        .route("/collections/{cid}", delete(delete_collection))
        .route("/{id}", get(get_document))
        .route("/{id}", patch(update_document))
        .route("/{id}", delete(delete_document))
        .route("/{id}/restore", post(restore_document))
        .route("/{id}/purge", delete(purge_document))
        .route("/{id}/content", get(get_content))
        // PUT /{id}/content accepts yrs binary state — up to
        // MAX_CONTENT_SIZE (10 MiB). The internal `body.len() >
        // MAX_CONTENT_SIZE` check in `put_content` returned 400
        // historically, but axum's default 2 MiB extractor cap
        // truncated the body before the check ever fired. Setting
        // an explicit per-route override here makes the intended
        // 10 MiB cap actually reachable and aligns with the user
        // direction "we need to support documents potentially in
        // 10MB+ not 256K" from the persist-on-refresh fix arc.
        // This also exempts /content from the 1 MiB global cap
        // installed in api_router (#42).
        .route("/{id}/content", put(put_content)
            .layer(axum::extract::DefaultBodyLimit::max(MAX_CONTENT_SIZE)))
        .route("/{id}/export/{format}", get(export_document))
        .route("/{id}/blobs", post(request_upload_url))
        .route("/{id}/blobs/{blob_id}", get(request_download_url))
        .route("/{id}/link-settings", get(get_link_settings))
        .route("/{id}/link-settings", patch(update_link_settings))
        .route("/{id}/lock", put(set_lock))
        // #142: mark / unmark a doc as a template; copy any source doc into
        // a new doc in the caller's Private folder.
        .route("/{id}/template", put(set_template))
        .route("/{id}/copy", post(copy_document))
        .route("/{id}/request-access", post(request_access))
        // #144: star / unstar a document for the current user.
        .route("/{id}/favorite", put(add_favorite).delete(remove_favorite))
        // #144: collection membership for a given doc.
        .route("/{id}/collections", get(list_doc_collections).post(create_doc_collection))
        .route(
            "/{id}/collections/{cid}",
            put(add_doc_to_collection).delete(remove_doc_from_collection),
        )
        // #149: multi-folder membership — list the doc's folders, add/remove
        // an additional folder.
        .route("/{id}/folders", get(list_doc_folders))
        .route(
            "/{id}/folders/{folder_id}",
            put(add_doc_to_folder).delete(remove_doc_from_folder),
        )
        .route("/{id}/import", post(import_file)
            .layer(axum::extract::DefaultBodyLimit::max(MAX_CONTENT_SIZE)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateDocumentRequest {
    #[serde(default = "default_title")]
    title: String,
    folder_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    doc_type: Option<DocType>,
}

fn default_title() -> String {
    "Untitled".to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentResponse {
    id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder_id: Option<String>,
    doc_type: String,
    created_at: i64,
    updated_at: i64,
    /// True when the document is in the caller's trash. Only surfaced for the
    /// owner — other endpoints continue to 404 on trashed docs.
    #[serde(default)]
    is_deleted: bool,
    /// #110: true only when the caller is a view-only user (not owner, no
    /// Edit) on a doc whose View-mode link has `allow_request_access` on.
    /// Drives the viewer-facing "Request edit access" affordance; the
    /// backend is the authority on eligibility, so the frontend just shows
    /// the button when this is set.
    #[serde(default)]
    can_request_access: bool,
    /// #111: the caller's effective write authority (Edit or Own). Drives
    /// the editor's read-only state — a View-only user gets a read-only
    /// editor even though the WS now delivers them live updates. Always set
    /// explicitly by every construction site (this is a response-only DTO).
    can_edit: bool,
    /// #144: whether the current user has starred this document.
    is_favorite: bool,
    /// #140: whether the document is locked for editing (doc-wide freeze).
    /// Drives the read-only editor + lock banner on the client. When true,
    /// `can_edit` is still the user's underlying grant; the editor reads
    /// `locked || !can_edit` to decide read-only.
    #[serde(default)]
    locked: bool,
    /// #140: whether the caller may toggle the lock — true iff they own the
    /// doc (the toggle requires `Own`). Lets the Format menu show the
    /// "Lock Edits" control only to the owner.
    #[serde(default)]
    can_manage: bool,
    /// #142: whether the document is marked as a template. Drives the
    /// Document-menu label ("Mark as Template" vs "Unmark Template") and the
    /// optional template badge in the header.
    #[serde(default)]
    is_template: bool,
}

/// Resolve the destination folder for a new document. An explicit
/// `folder_id` is access-checked at Edit level and returned as-is; when
/// absent, the caller's home folder is used.
///
/// A missing user row maps to `ApiError::Internal`, not `NotFound`:
/// authentication has already succeeded by the time this runs, so an absent
/// user record is a data-integrity failure (500), not a client-addressable
/// 404. Shared by `create_document`, `create_from_text`, and `import_job`,
/// which previously inlined this block verbatim.
async fn resolve_dest_folder(
    state: &AppState,
    folder_id: Option<&str>,
    user_id: &str,
) -> Result<String, ApiError> {
    match folder_id {
        Some(id) => {
            super::folders::check_folder_access(state, id, user_id, AccessLevel::Edit).await?;
            Ok(id.to_string())
        }
        None => {
            let user = state
                .user_repo
                .get_by_id(user_id)
                .await?
                .ok_or_else(|| ApiError::Internal("user record missing after auth".to_string()))?;
            Ok(user.home_folder_id)
        }
    }
}

/// POST /documents -- create a new document.
async fn create_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<CreateDocumentRequest>,
) -> Result<(StatusCode, Json<DocumentResponse>), ApiError> {
    let doc_id = new_id();
    let now = now_usec();

    // Resolve and verify target folder access
    let folder_id = resolve_dest_folder(&state, req.folder_id.as_deref(), &user_id).await?;

    // Create document with initial snapshot first.
    // A document without a folder reference is benign (orphaned doc);
    // a folder reference to a non-existent document is a persistent leak.
    let ogre_doc = OgreDoc::new();
    let snapshot = ogre_doc.to_state_bytes();

    let doc_type = req.doc_type.unwrap_or(DocType::Document);
    let meta = DocumentMeta {
        doc_id: doc_id.clone(),
        title: req.title.clone(),
        owner_id: user_id,
        folder_id: Some(folder_id.clone()),
        additional_folder_ids: Vec::new(),
        workspace_id: req.workspace_id,
        doc_type: doc_type.clone(),
        snapshot_version: 1,
        snapshot_s3_key: Some(format!("docs/{doc_id}/snapshots/1.bin")),
        is_deleted: false,
        deleted_at: None,
        link_sharing_mode: None,
        link_view_options: ogrenotes_storage::models::ViewOptions::default(),
        locked: false,
        is_template: false,
        created_at: now,
        updated_at: now,
    };

    state.doc_repo.create(&meta, &snapshot).await?;

    counter::inc(MetricKey::new(
        "doc.created_total",
        &[("doc_type", doc_type.as_str())],
    ));
    tracing::info!(
        event_type = "doc_created",
        doc_id = %doc_id,
        doc_type = doc_type.as_str(),
        "document created"
    );

    // Add to folder after document exists
    let child = FolderChild {
        folder_id: folder_id.clone(),
        child_id: doc_id.clone(),
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&child)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Index for search (fire-and-forget)
    spawn_index_document_from_bytes(&state, meta.clone(), snapshot);

    Ok((
        StatusCode::CREATED,
        Json(DocumentResponse {
            id: doc_id,
            title: req.title,
            folder_id: meta.folder_id.clone(),
            doc_type: doc_type.as_str().to_string(),
            created_at: now,
            updated_at: now,
            is_deleted: false,
            // The creator owns the new doc — never a request-access viewer,
            // always able to edit, and not yet favorited.
            can_request_access: false,
            can_edit: true,
            is_favorite: false,
            // A brand-new doc is unlocked; the creator owns it.
            locked: false,
            can_manage: true,
            // #142: a brand-new doc is never a template; the user marks it
            // explicitly via PUT /documents/:id/template.
            is_template: false,
        }),
    ))
}

/// Body shape for POST /documents/import — Phase 5 M-P5 piece A.
/// `format` is the wire-format identifier; `content` is the raw
/// source text (Markdown for v1; HTML lands in piece B).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportTextRequest {
    format: String,
    #[serde(default = "default_title")]
    title: String,
    content: String,
    #[serde(default)]
    folder_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
}

/// POST /documents/import — create a new document by parsing
/// a Markdown source string. v1 accepts `format = "markdown"`;
/// HTML lands in M-P5 piece B against the same endpoint shape.
async fn create_from_text(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<ImportTextRequest>,
) -> Result<(StatusCode, Json<DocumentResponse>), ApiError> {
    // Per-user rate limit — independent budget so legitimate
    // hand-driven imports don't have to share with the
    // content-write bucket.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "import",
        &user_id,
        state.config.rate_limit_import_per_min,
        60,
    )
    .await?;

    // Format gate. Both Markdown and HTML go through the same
    // endpoint for shape symmetry; the parser routing happens
    // inside `crates/collab/src/import.rs`. v1 limitation (both
    // formats): inline marks are dropped — see the module docs.
    let yrs_doc = match req.format.as_str() {
        "markdown" | "md" => ogrenotes_collab::import::from_markdown(&req.content),
        "html" => ogrenotes_collab::import::from_html(&req.content),
        other => {
            return Err(ApiError::BadRequest(format!(
                "unknown import format: {other}"
            )));
        }
    };

    let doc_id = new_id();
    let now = now_usec();

    // Same folder-resolution path as create_document.
    let folder_id = resolve_dest_folder(&state, req.folder_id.as_deref(), &user_id).await?;

    let snapshot = ogrenotes_collab::snapshot::doc_to_bytes(&yrs_doc);

    let meta = DocumentMeta {
        doc_id: doc_id.clone(),
        title: req.title.clone(),
        owner_id: user_id,
        folder_id: Some(folder_id.clone()),
        additional_folder_ids: Vec::new(),
        workspace_id: req.workspace_id,
        doc_type: DocType::Document,
        snapshot_version: 1,
        snapshot_s3_key: Some(format!("docs/{doc_id}/snapshots/1.bin")),
        is_deleted: false,
        deleted_at: None,
        link_sharing_mode: None,
        link_view_options: ogrenotes_storage::models::ViewOptions::default(),
        locked: false,
        is_template: false,
        created_at: now,
        updated_at: now,
    };

    state.doc_repo.create(&meta, &snapshot).await?;

    counter::inc(MetricKey::new(
        "doc.imported_total",
        &[("format", req.format.as_str())],
    ));
    tracing::info!(
        event_type = "doc_imported",
        doc_id = %doc_id,
        format = req.format.as_str(),
        "document imported"
    );

    let child = FolderChild {
        folder_id: folder_id.clone(),
        child_id: doc_id.clone(),
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&child)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    spawn_index_document_from_bytes(&state, meta.clone(), snapshot);

    Ok((
        StatusCode::CREATED,
        Json(DocumentResponse {
            id: doc_id,
            title: req.title,
            folder_id: meta.folder_id.clone(),
            doc_type: DocType::Document.as_str().to_string(),
            created_at: now,
            updated_at: now,
            is_deleted: false,
            // The creator owns the new doc — never a request-access viewer,
            // always able to edit, and not yet favorited.
            can_request_access: false,
            can_edit: true,
            is_favorite: false,
            // A brand-new doc is unlocked; the creator owns it.
            locked: false,
            can_manage: true,
            // #142: imports start as regular docs; the user marks them later.
            is_template: false,
        }),
    ))
}

/// Query params for the async import-job endpoint. The file rides in
/// the multipart body; its target folder + title come in the query so
/// we don't have to walk multiple multipart fields.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportJobParams {
    #[serde(default)]
    folder_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportJobResponse {
    job_id: String,
}

/// The binary import formats this route accepts. Both flow through the
/// same async job → poll path; the extension selects the parser the
/// worker runs.
#[derive(Clone, Copy)]
enum ImportFormat {
    Docx,
    Pdf,
}

impl ImportFormat {
    /// File extension (with dot) for the staging key.
    fn ext(self) -> &'static str {
        match self {
            ImportFormat::Docx => ".docx",
            ImportFormat::Pdf => ".pdf",
        }
    }

    /// Metric / log label.
    fn label(self) -> &'static str {
        match self {
            ImportFormat::Docx => "docx",
            ImportFormat::Pdf => "pdf",
        }
    }
}

/// POST /documents/import-job — stage a DOCX or PDF upload to S3 and
/// enqueue the matching import job. Returns `202 Accepted` with the job
/// id; the client polls `GET /api/v1/jobs/{id}` for the resulting doc
/// id. The folder is resolved + authorized here (in the request's auth
/// context); the worker writes the document with that already-trusted
/// folder.
async fn import_job(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Query(params): Query<ImportJobParams>,
    mut multipart: axum::extract::Multipart,
) -> Result<(StatusCode, Json<ImportJobResponse>), ApiError> {
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "import",
        &user_id,
        state.config.rate_limit_import_per_min,
        60,
    )
    .await?;

    let producer = state.job_producer.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Async import is not available".to_string())
    })?;

    // Read the single file field from the multipart body.
    let field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Multipart error: {e}")))?
        .ok_or_else(|| ApiError::BadRequest("No file in upload".to_string()))?;
    // Require a filename with a supported extension. A field with no
    // filename (or an unsupported one) is rejected before any side
    // effect — we don't want to stage junk to S3 or burn a queue slot.
    let filename = field.file_name().map(|s| s.to_string());
    let format = match filename.as_deref().map(|n| n.to_ascii_lowercase()) {
        Some(n) if n.ends_with(".docx") => ImportFormat::Docx,
        Some(n) if n.ends_with(".pdf") => ImportFormat::Pdf,
        _ => {
            return Err(ApiError::BadRequest(
                "Expected a .docx or .pdf file".to_string(),
            ));
        }
    };
    let filename = filename.expect("format match implies a filename");
    let data = field
        .bytes()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Failed to read upload: {e}")))?;
    if data.is_empty() {
        return Err(ApiError::BadRequest("Uploaded file is empty".to_string()));
    }
    if data.len() > MAX_CONTENT_SIZE {
        return Err(ApiError::BadRequest(format!(
            "File exceeds the {MAX_CONTENT_SIZE}-byte import limit"
        )));
    }

    // Resolve + authorize the destination folder up front, while we
    // still have the caller's identity.
    let folder_id = resolve_dest_folder(&state, params.folder_id.as_deref(), &user_id).await?;

    // Title precedence: explicit query param, else the upload's
    // filename minus its extension, else a generic default.
    let title = params
        .title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| {
            let base = filename.rsplit_once('.').map_or(filename.as_str(), |(s, _)| s);
            if base.is_empty() {
                "Imported document".to_string()
            } else {
                base.to_string()
            }
        });
    // Cap the title: it rides through the job envelope and lands in a
    // DynamoDB attribute, and the filename it can derive from is
    // attacker-controlled within the multipart body. Truncate on a char
    // boundary so an oversized filename can't blow the item-size limit.
    let title: String = title.chars().take(MAX_IMPORT_TITLE_CHARS).collect();

    // Stage the bytes to S3 under a per-user import key; the worker
    // fetches them by this key. Reuse the doc repo's S3 client.
    let s3_key = format!("imports/{user_id}/{}{}", new_id(), format.ext());
    state
        .doc_repo
        .s3()
        .put_object(&s3_key, data.to_vec())
        .await
        .map_err(|e| ApiError::Internal(format!("upload staging failed: {e}")))?;

    let job = match format {
        ImportFormat::Docx => ogrenotes_worker::Job::ImportDocx {
            s3_key: s3_key.clone(),
            title,
            folder_id: Some(folder_id),
            owner_id: user_id.clone(),
        },
        ImportFormat::Pdf => ogrenotes_worker::Job::ImportPdf {
            s3_key: s3_key.clone(),
            title,
            folder_id: Some(folder_id),
            owner_id: user_id.clone(),
        },
    };
    let job_id = match producer.enqueue(job).await {
        Ok(id) => id,
        Err(e) => {
            // The blob is staged but no job references it; best-effort
            // delete so a failed enqueue doesn't leak an orphan (no
            // reachability chain ever collects it otherwise).
            if let Err(del) = state.doc_repo.s3().delete_object(&s3_key).await {
                tracing::warn!(s3_key, error = %del, "import staging blob leaked after enqueue failure");
            }
            return Err(ApiError::ServiceUnavailable(format!(
                "could not enqueue import job: {e}"
            )));
        }
    };

    counter::inc(MetricKey::new(
        "doc.import_job_enqueued_total",
        &[("format", format.label())],
    ));
    tracing::info!(
        event_type = "doc_import_job_enqueued",
        job_id = %job_id,
        owner_id = %user_id,
        format = format.label(),
        "import job enqueued"
    );

    Ok((StatusCode::ACCEPTED, Json(ImportJobResponse { job_id })))
}

/// Fetch and verify a document belongs to the user and is not deleted.
/// Check that the user has at least the required access level to a document.
/// Returns the document metadata if access is granted.
///
/// Access is determined by (first match wins):
/// 1. Owner always has Own access.
/// 2. Direct document membership (DOC#/MEMBER# rows).
/// 3. Members of the document's parent folder inherit the folder's access level.
/// 4. Link sharing: workspace members get the link-level access if enabled.
/// 5. Otherwise, access is denied.
pub(crate) async fn check_doc_access(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
    required: ogrenotes_storage::models::AccessLevel,
) -> Result<DocumentMeta, ApiError> {
    let (meta, is_trashed) =
        check_doc_access_allow_deleted(state, doc_id, user_id, required).await?;
    if is_trashed {
        return Err(ApiError::NotFound("Document not found".to_string()));
    }
    Ok(meta)
}

/// The four possible outcomes of an access-control check, decoupled from
/// the HTTP error layer so the decision logic can be unit-tested without
/// any I/O. `check_doc_access_allow_deleted` translates these to
/// `Result<(DocumentMeta, bool), ApiError>`; tests assert on the variant
/// directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessDecision {
    /// Caller has the requested access level on a live (non-trashed) doc.
    Allowed,
    /// Doc exists but is soft-deleted and the caller is its owner — the
    /// "in-trash read-only" path. Non-owners on a trashed doc get
    /// `NotFound`.
    Trashed,
    /// Caller is on a trashed doc and is not its owner. Maps to 404
    /// rather than 403 so deletion is not a side-channel for existence
    /// probes.
    NotFound,
    /// Doc exists and is live, but no membership / inheritance / link-
    /// sharing branch grants the required level.
    Forbidden,
}

/// #149: one containing folder's contribution to the access decision.
/// `evaluate_doc_access` unions over every `FolderGrant` in its slice.
/// `member` is the caller's `FolderMember` row (None if not a member, or if
/// the folder is Restricted — the wrapper skips the member fetch for
/// Restricted folders as an optimisation). A `Restricted` folder can never
/// contribute a grant regardless of `member`.
pub(crate) struct FolderGrant {
    pub inherit_mode: ogrenotes_storage::models::InheritMode,
    pub member: Option<ogrenotes_storage::models::folder::FolderMember>,
}

/// Pure decision logic for `check_doc_access`. Takes the data the async
/// wrapper has already fetched, branches on it deterministically, and
/// returns one of the four `AccessDecision` outcomes. **Idempotent and
/// synchronous** — the unit tests at the bottom of this file and the proptest
/// matrix call this directly.
///
/// Inputs:
/// - `meta` — the document metadata. Always required.
/// - `user_id` — the caller's user id.
/// - `required` — the access level the operation needs.
/// - `direct_member` — Some(row) if a `DOC#/MEMBER#` row exists for the
///   caller. Not fetched when the caller is the owner.
/// - `folder_grants` — one entry per containing folder
///   (`{folder_id} ∪ additional_folder_ids`), each carrying its
///   `inherit_mode` and the caller's `FolderMember` (None for Restricted
///   folders or when the caller has no membership). A missing or failed
///   folder fetch produces no entry (fail-safe). See `fetch_folder_grants`.
/// - `workspace_member_for_link_sharing` — Some(row) when link sharing
///   applies (doc has `link_sharing_mode != None`, `workspace_id` is set, and
///   the caller is a workspace member). Presence is the signal; the function
///   does not re-validate the preconditions.
pub(crate) fn evaluate_doc_access(
    meta: &DocumentMeta,
    user_id: &str,
    required: AccessLevel,
    direct_member: Option<&DocMember>,
    folder_grants: &[FolderGrant],
    workspace_member_for_link_sharing: Option<&ogrenotes_storage::models::workspace::WorkspaceMember>,
) -> AccessDecision {
    if meta.is_deleted {
        if meta.owner_id == user_id {
            return AccessDecision::Trashed;
        }
        return AccessDecision::NotFound;
    }

    // 1. Owner always has full access.
    if meta.owner_id == user_id {
        return AccessDecision::Allowed;
    }

    // 2. Direct document membership.
    if let Some(member) = direct_member {
        if access_level_satisfies(&member.access_level, &required) {
            return AccessDecision::Allowed;
        }
    }

    // 3. Folder membership — most-permissive UNION across every containing
    //    folder (#149). Any folder that isn't Restricted and in which the
    //    caller has a satisfying membership grants access. A Restricted
    //    folder contributes no grant (it can't veto an open folder the doc is
    //    also in).
    for grant in folder_grants {
        if matches!(
            grant.inherit_mode,
            ogrenotes_storage::models::InheritMode::Restricted
        ) {
            continue;
        }
        if let Some(member) = &grant.member {
            if access_level_satisfies(&member.access_level, &required) {
                return AccessDecision::Allowed;
            }
        }
    }

    // 4. Link sharing — workspace members of a link-shared doc get the
    // link-level access. The caller pre-confirmed (a) link_sharing_mode
    // is Some(View|Edit), (b) workspace_id is Some, (c) the caller is a
    // workspace member; we just translate the mode to the level here.
    if workspace_member_for_link_sharing.is_some() {
        if let Some(mode) = &meta.link_sharing_mode {
            use ogrenotes_storage::models::LinkSharingMode;
            if *mode != LinkSharingMode::None {
                let link_level = match mode {
                    LinkSharingMode::Edit => AccessLevel::Edit,
                    LinkSharingMode::View => AccessLevel::View,
                    LinkSharingMode::None => unreachable!(),
                };
                if access_level_satisfies(&link_level, &required) {
                    return AccessDecision::Allowed;
                }
            }
        }
    }

    AccessDecision::Forbidden
}

/// #149: fetch the per-folder access grants for every folder a doc is in
/// ({`folder_id`} ∪ `additional_folder_ids`). For each folder: its
/// `inherit_mode` plus — only when not `Restricted` — the caller's
/// `FolderMember` row. Restricted folders are still included (with
/// `member: None`) so the union short-circuits them. A folder that fails to
/// load contributes no grant (matches the old code's silent fall-through on a
/// missing folder). Fan-out is bounded by the doc's folder count (typ. 1–3).
pub(crate) async fn fetch_folder_grants(
    state: &AppState,
    meta: &DocumentMeta,
    user_id: &str,
) -> Vec<FolderGrant> {
    let mut grants = Vec::new();
    for folder_id in meta.folder_id.iter().chain(meta.additional_folder_ids.iter()) {
        let Some(folder) = state.folder_repo.get(folder_id).await.ok().flatten() else {
            continue;
        };
        let inherit_mode = folder.inherit_mode.clone();
        let member = if matches!(
            inherit_mode,
            ogrenotes_storage::models::InheritMode::Restricted
        ) {
            None
        } else {
            state.folder_repo.get_member(folder_id, user_id).await.ok().flatten()
        };
        grants.push(FolderGrant { inherit_mode, member });
    }
    grants
}

/// Variant of `check_doc_access` that returns successfully for soft-deleted
/// documents — but only to the owner. Used by read endpoints that render the
/// read-only "in trash" view, and by the restore / purge handlers.
///
/// Returns `(meta, is_trashed)` where `is_trashed == true` means the doc is
/// currently in the caller's trash. Writing endpoints must use the strict
/// `check_doc_access` which 404s on trashed docs.
///
/// This is the I/O-bearing wrapper around `evaluate_doc_access`. It
/// fetches each piece of data conditionally to avoid wasted DDB reads
/// (e.g. `direct_member` is not fetched when the caller is the owner)
/// and then hands the data to the pure function for the decision.
pub(crate) async fn check_doc_access_allow_deleted(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
    required: ogrenotes_storage::models::AccessLevel,
) -> Result<(DocumentMeta, bool), ApiError> {
    let meta = state
        .doc_repo
        .get(doc_id)
        .await?
        .ok_or(ApiError::NotFound("Document not found".to_string()))?;
    check_doc_access_from_meta_allow_deleted(state, meta, user_id, required).await
}

/// Same as [`check_doc_access_allow_deleted`] but takes a pre-fetched
/// [`DocumentMeta`] instead of a doc id. Used by handlers that already
/// loaded the meta for another reason (e.g. `copy_document`'s samples
/// bypass) — avoids the second `doc_repo.get` the id-based variant does
/// internally.
pub(crate) async fn check_doc_access_from_meta_allow_deleted(
    state: &AppState,
    meta: DocumentMeta,
    user_id: &str,
    required: ogrenotes_storage::models::AccessLevel,
) -> Result<(DocumentMeta, bool), ApiError> {
    // Short-circuit on the trash + owner branches before issuing any
    // membership fetches.
    if meta.is_deleted {
        return match evaluate_doc_access(&meta, user_id, required, None, &[], None) {
            AccessDecision::Trashed => Ok((meta, true)),
            AccessDecision::NotFound => Err(ApiError::NotFound("Document not found".to_string())),
            // Unreachable for a deleted doc, but fall through safely.
            _ => Err(ApiError::NotFound("Document not found".to_string())),
        };
    }
    if meta.owner_id == user_id {
        return Ok((meta, false));
    }

    // 2. Direct document membership.
    let direct_member = state
        .doc_repo
        .get_doc_member(&meta.doc_id, user_id)
        .await
        .ok()
        .flatten();

    // 3. Folder membership — unioned across every folder the doc is in
    // (#149). Restricted folders contribute no grant.
    let folder_grants = fetch_folder_grants(state, &meta, user_id).await;

    // 4. Link sharing — only fetch the workspace-member row when the doc
    // actually has link sharing enabled, to avoid one DDB read per
    // unshared doc on the access-denied path.
    let workspace_member = if meta
        .link_sharing_mode
        .as_ref()
        .map(|m| !matches!(m, ogrenotes_storage::models::LinkSharingMode::None))
        .unwrap_or(false)
    {
        if let Some(ref ws_id) = meta.workspace_id {
            state
                .workspace_repo
                .get_member(ws_id, user_id)
                .await
                .ok()
                .flatten()
        } else {
            None
        }
    } else {
        None
    };

    // Clone `required` so we can re-inspect it after evaluate_doc_access
    // consumes its copy. Cheap — AccessLevel is just an enum tag.
    let required_for_metric = required.clone();
    // #149: the precedence metric below fires when a direct grant lets the
    // caller in while at least one containing folder is Restricted.
    let any_restricted_folder = folder_grants
        .iter()
        .any(|g| matches!(g.inherit_mode, ogrenotes_storage::models::InheritMode::Restricted));
    match evaluate_doc_access(
        &meta,
        user_id,
        required,
        direct_member.as_ref(),
        &folder_grants,
        workspace_member.as_ref(),
    ) {
        AccessDecision::Allowed => {
            // Audit-worthy precedence event: a direct DocMember grant
            // satisfied access on a folder whose inherit_mode is
            // Restricted. The behavior is correct (direct grants
            // override the folder-level lockdown), but the
            // configuration is the kind of overlap that confuses
            // operators — alerting on it lets us notice when a doc has
            // accidentally accumulated a direct grant inside a folder
            // that was meant to be locked down. The pure function
            // already encodes the precedence; this counter is purely
            // observational.
            if let Some(member) = direct_member.as_ref() {
                if any_restricted_folder
                    && access_level_satisfies(&member.access_level, &required_for_metric)
                {
                    ogrenotes_common::metrics::counter::inc(
                        ogrenotes_common::metrics::MetricKey::new(
                            "doc_access.direct_overrides_restricted_folder",
                            &[],
                        ),
                    );
                }
            }
            Ok((meta, false))
        }
        AccessDecision::Trashed => Ok((meta, true)),
        AccessDecision::NotFound => Err(ApiError::NotFound("Document not found".to_string())),
        AccessDecision::Forbidden => Err(ApiError::Forbidden),
    }
}

/// Check if `have` access level is at least as permissive as `need`.
/// Own > Edit > Comment > View.
pub(crate) fn access_level_satisfies(have: &AccessLevel, need: &AccessLevel) -> bool {
    fn rank(level: &AccessLevel) -> u8 {
        match level {
            AccessLevel::Own => 4,
            AccessLevel::Edit => 3,
            AccessLevel::Comment => 2,
            AccessLevel::View => 1,
        }
    }
    rank(have) >= rank(need)
}

// ─── Link view-option enforcement (Phase 2) ─────────────────────
//
// The view sub-options gate *extra* capabilities for callers whose only
// path into a doc is a View-mode link; durable members (owner / direct
// member / non-restricted folder member) and Edit-mode-link viewers are
// never constrained by them. Distinguishing the two is done by replaying
// the pure `evaluate_doc_access` decision with the link branch disabled
// (`workspace_member = None`): if it still grants View, the caller is
// durable. See design/linksharing.md §5.3.

/// True if `user_id` reaches `meta` through a durable (non-link) path —
/// owner, a direct doc-member, or a non-restricted folder membership.
/// Link-only viewers return `false`. Fetches the membership rows only
/// when the caller isn't the owner.
pub(crate) async fn has_durable_access(
    state: &AppState,
    meta: &DocumentMeta,
    user_id: &str,
) -> Result<bool, ApiError> {
    if meta.owner_id == user_id {
        return Ok(true);
    }
    let direct_member = state
        .doc_repo
        .get_doc_member(&meta.doc_id, user_id)
        .await
        .ok()
        .flatten();
    // #149: union over every folder the doc is in (Restricted ones grant
    // nothing). Bounded fan-out by folder count.
    let folder_grants = fetch_folder_grants(state, meta, user_id).await;
    // Replay the decision WITHOUT the link branch (workspace_member = None):
    // Allowed ⟺ the caller has a non-link grant of at least View.
    let decision = evaluate_doc_access(
        meta,
        user_id,
        AccessLevel::View,
        direct_member.as_ref(),
        &folder_grants,
        None,
    );
    Ok(matches!(decision, AccessDecision::Allowed))
}

/// Enforce a View-mode link sub-option for a *read* feature (edit
/// history, conversation). Call AFTER `check_doc_access(.., View)` has
/// granted access. No-op unless the doc has a View-mode link and the
/// caller is a link-only viewer; for that caller the feature is allowed
/// only when `enabled` (the relevant `link_view_options` flag).
pub(crate) async fn enforce_view_link_option(
    state: &AppState,
    meta: &DocumentMeta,
    user_id: &str,
    enabled: bool,
) -> Result<(), ApiError> {
    // Only View-mode links gate features. No link, or an Edit-mode link
    // (which already implies these capabilities), leaves them ungated.
    if meta.link_sharing_mode != Some(ogrenotes_storage::models::LinkSharingMode::View) {
        return Ok(());
    }
    // Durable members keep full access regardless of the flag.
    if has_durable_access(state, meta, user_id).await? {
        return Ok(());
    }
    if enabled {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Resolve comment-create access, honoring the `allow_comments`
/// View-mode link sub-option, and return the doc metadata. Allowed when
/// the caller has durable Comment+ access or an Edit-mode link (both via
/// `check_doc_access(Comment)`), OR a View-mode link whose
/// `allow_comments` option is on AND the caller is a member of the doc's
/// workspace (the link audience). Mirrors the §5.3 predicate.
pub(crate) async fn check_comment_access(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
) -> Result<DocumentMeta, ApiError> {
    match check_doc_access(state, doc_id, user_id, AccessLevel::Comment).await {
        Ok(meta) => Ok(meta),
        Err(ApiError::Forbidden) => {
            // No durable Comment+ / Edit-link grant. The only remaining
            // path is a View-mode link with comments enabled, for a
            // member of the doc's workspace.
            let meta = check_doc_access(state, doc_id, user_id, AccessLevel::View).await?;
            if meta.link_sharing_mode == Some(ogrenotes_storage::models::LinkSharingMode::View)
                && meta.link_view_options.allow_comments
            {
                if let Some(ref ws_id) = meta.workspace_id {
                    let is_member = state
                        .workspace_repo
                        .get_member(ws_id, user_id)
                        .await
                        .map_err(|e| ApiError::Internal(e.to_string()))?
                        .is_some();
                    if is_member {
                        return Ok(meta);
                    }
                }
            }
            Err(ApiError::Forbidden)
        }
        Err(e) => Err(e),
    }
}

/// Legacy compatibility wrapper — checks ownership only.
/// Used by endpoints that haven't been migrated to check_doc_access yet.
pub(crate) async fn get_verified_doc(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
) -> Result<DocumentMeta, ApiError> {
    check_doc_access(
        state,
        doc_id,
        user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await
}

/// GET /documents/:id -- get document metadata. Owner may read metadata for
/// a trashed document (used to render the read-only banner); everyone else
/// gets 404.
async fn get_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DocumentResponse>, ApiError> {
    let (meta, is_trashed) = check_doc_access_allow_deleted(
        &state, &id, &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    ).await?;

    // Record open receipt only for live docs — no point emitting "opened"
    // events for trashed docs the owner is reviewing.
    if !is_trashed && meta.owner_id != user_id {
        let doc_repo = state.doc_repo.clone();
        let notif_repo = state.notification_repo.clone();
        let activity_repo = state.activity_repo.clone();
        let email_service = state.email_service.clone();
        let open_doc_id = id.clone();
        let open_user_id = user_id.clone();
        let owner_id = meta.owner_id.clone();
        tokio::spawn(async move {
            let now = ogrenotes_common::time::now_usec();

            let open = ogrenotes_storage::models::document::DocOpen {
                doc_id: open_doc_id.clone(),
                user_id: open_user_id.clone(),
                first_opened_at: now,
            };
            if let Ok(true) = doc_repo.record_open(&open).await {
                // First open — record activity event
                let activity = ogrenotes_storage::models::activity::Activity {
                    activity_id: nanoid::nanoid!(16),
                    doc_id: open_doc_id.clone(),
                    event_type: ogrenotes_storage::models::activity::ActivityEventType::Open,
                    actor_id: open_user_id.clone(),
                    detail: "{}".to_string(),
                    created_at: now,
                };
                let _ = activity_repo.create(&activity).await;
                let notif = ogrenotes_storage::models::notification::Notification {
                    notif_id: nanoid::nanoid!(16),
                    user_id: owner_id,
                    notif_type: ogrenotes_storage::models::notification::NotifType::DocumentOpened,
                    doc_id: Some(open_doc_id),
                    thread_id: None,
                    actor_id: open_user_id,
                    message: "opened your document".to_string(),
                    preview: None,
                    block_id: None,
                    read: false,
                    created_at: now,
                };
                let _ = notif_repo.create(&notif).await;
                // Opens are not direct events — only fires under `All`.
                email_service.spawn_for_notification(notif, false);
            }
        });
    }

    // The caller's effective write authority on this doc. One extra access
    // evaluation at Edit level (the View check above only proved read
    // access). Drives both the editor read-only state (#111) and the
    // request-access affordance (#110).
    let can_edit = check_doc_access(
        &state,
        &id,
        &user_id,
        ogrenotes_storage::models::AccessLevel::Edit,
    )
    .await
    .is_ok();

    // #110: offer the viewer a "request edit access" affordance only when
    // they're view-only (no Edit, and a non-owner can never have Edit-less
    // ownership) AND the View-mode link explicitly invites requests.
    // Mirrors the eligibility gate in `request_access`.
    let can_request_access = !can_edit
        && meta.owner_id != user_id
        && meta.link_sharing_mode == Some(ogrenotes_storage::models::LinkSharingMode::View)
        && meta.link_view_options.allow_request_access;

    // #144: reflect the star state so the header toggle renders correctly on
    // open. Best-effort — a lookup failure just shows un-starred.
    let is_favorite = state
        .doc_repo
        .is_favorite(&user_id, &id)
        .await
        .unwrap_or(false);

    // #140: only the owner may toggle the lock; surface the current lock state
    // so the editor renders read-only + a banner for everyone.
    let can_manage = meta.owner_id == user_id;
    let locked = meta.locked;

    let is_template = meta.is_template;
    Ok(Json(DocumentResponse {
        id: meta.doc_id,
        title: meta.title,
        folder_id: meta.folder_id,
        doc_type: meta.doc_type.as_str().to_string(),
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        is_deleted: is_trashed,
        can_request_access,
        can_edit,
        is_favorite,
        locked,
        can_manage,
        is_template,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDocumentRequest {
    title: Option<String>,
}

/// PATCH /documents/:id -- update document metadata.
async fn update_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateDocumentRequest>,
) -> Result<StatusCode, ApiError> {
    // Require Edit access to update metadata
    let meta = check_doc_access(&state, &id, &user_id, ogrenotes_storage::models::AccessLevel::Edit).await?;

    let now = now_usec();
    state
        .doc_repo
        .update_metadata(&id, req.title.as_deref(), now)
        .await?;

    // Re-index with updated metadata
    let mut updated_meta = meta;
    if let Some(ref new_title) = req.title {
        updated_meta.title = new_title.clone();
    }
    updated_meta.updated_at = now;
    spawn_index_document(&state, updated_meta);

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteDocumentRequest {
    /// The folder the document is currently in (so we can remove it).
    source_folder_id: Option<String>,
}

/// DELETE /documents/:id -- soft delete and move to trash.
async fn delete_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    delete_req: Option<Json<DeleteDocumentRequest>>,
) -> Result<StatusCode, ApiError> {
    // Require Own access to delete (only the owner can delete)
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::Own).await?;

    // Already deleted -- idempotent
    if meta.is_deleted {
        return Ok(StatusCode::NO_CONTENT);
    }

    let now = now_usec();
    state.doc_repo.soft_delete(&id, now).await?;

    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::Internal("User not found".to_string()))?;

    // #149: remove the doc from EVERY folder it's in — the primary plus all
    // additional memberships — not just one source. A multi-folder doc must
    // leave all locations when trashed. Also honor an explicit
    // source_folder_id if it isn't already covered, then clear the additional
    // set so a later restore (which re-homes only the primary) can't
    // resurrect stale memberships.
    let mut purge: Vec<String> = meta
        .folder_id
        .iter()
        .chain(meta.additional_folder_ids.iter())
        .cloned()
        .collect();
    if let Some(Json(req)) = delete_req {
        if let Some(src) = req.source_folder_id {
            if !purge.contains(&src) {
                purge.push(src);
            }
        }
    }
    for folder_id in &purge {
        let _ = state.folder_repo.remove_child(folder_id, &id).await;
    }
    if !meta.additional_folder_ids.is_empty() {
        let _ = state.doc_repo.clear_additional_folders(&id, now).await;
    }

    // Add to trash folder
    let trash_child = FolderChild {
        folder_id: user.trash_folder_id,
        child_id: id.clone(),
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&trash_child)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    counter::inc(MetricKey::new(
        "doc.deleted_total",
        &[("doc_type", meta.doc_type.as_str())],
    ));

    // M-E6: durable audit of the soft-delete. `hard = false` so a
    // future reader can distinguish trash moves from the trash
    // worker's hard purges (which write `hard = true` when M-E7
    // wires the worker side).
    record_security_event(
        &state,
        &user_id,
        SecurityAuditAction::DocDeleted {
            doc_id: id.clone(),
            hard: false,
        },
    );

    // Activity feed row — survives the soft-delete because the doc
    // row itself stays under PK=DOC#<id> (just marked is_deleted).
    // hard_delete (purge_document / M-E7 trash worker) sweeps every
    // row under this PK including activity rows, so we deliberately
    // do NOT mirror this write from the hard-delete paths — the
    // SecurityAudit row carries the durable trail there.
    {
        let activity_repo = state.activity_repo.clone();
        let act_doc_id = id.clone();
        let act_user_id = user_id.clone();
        tokio::spawn(async move {
            let activity = ogrenotes_storage::models::activity::Activity {
                activity_id: nanoid::nanoid!(16),
                doc_id: act_doc_id,
                event_type: ogrenotes_storage::models::activity::ActivityEventType::Delete,
                actor_id: act_user_id,
                detail: serde_json::json!({}).to_string(),
                created_at: ogrenotes_common::time::now_usec(),
            };
            let _ = activity_repo.create(&activity).await;
        });
    }

    // Remove from search index
    spawn_delete_from_index(&state, id);

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreDocumentRequest {
    target_folder_id: String,
}

/// POST /documents/:id/restore -- un-trash a document into a target folder.
///
/// Caller must own the doc, the doc must currently be trashed, and the target
/// must be a user folder the caller owns or can edit (restoring into a System
/// folder other than Home/Private is refused).
async fn restore_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<RestoreDocumentRequest>,
) -> Result<StatusCode, ApiError> {
    let (mut meta, is_trashed) =
        check_doc_access_allow_deleted(&state, &id, &user_id, AccessLevel::Own).await?;
    if !is_trashed {
        return Err(ApiError::BadRequest(
            "Document is not in trash".to_string(),
        ));
    }

    // Verify target folder access and that it is not Trash (or another
    // non-Home/Private system folder — users should not be able to drop docs
    // back into system folders they can't otherwise write to).
    let target = super::folders::check_folder_access(
        &state, &req.target_folder_id, &user_id, AccessLevel::Edit,
    ).await?;

    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::Internal("User not found".to_string()))?;

    if matches!(target.folder_type, FolderType::System)
        && target.folder_id != user.home_folder_id
        && target.folder_id != user.private_folder_id
    {
        return Err(ApiError::BadRequest(
            "Cannot restore into a system folder".to_string(),
        ));
    }

    let now = now_usec();
    state
        .doc_repo
        .restore(&id, &req.target_folder_id, now)
        .await?;

    // Swap folder memberships: remove from trash, add to target.
    let _ = state
        .folder_repo
        .remove_child(&user.trash_folder_id, &id)
        .await;

    let new_child = FolderChild {
        folder_id: req.target_folder_id.clone(),
        child_id: id.clone(),
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&new_child)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Re-index with refreshed metadata.
    meta.folder_id = Some(req.target_folder_id);
    meta.is_deleted = false;
    meta.deleted_at = None;
    meta.updated_at = now;
    counter::inc(MetricKey::new(
        "doc.restored_total",
        &[("doc_type", meta.doc_type.as_str())],
    ));
    spawn_index_document(&state, meta);

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /documents/:id/purge -- permanently delete a trashed document.
///
/// Caller must own the doc and it must already be in the trash (force users
/// through the two-step soft-delete flow). Hard-deletes the doc row + all
/// associated rows, S3 blobs, reverse-relationships on other docs, and the
/// search index entry.
async fn purge_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (meta, is_trashed) =
        check_doc_access_allow_deleted(&state, &id, &user_id, AccessLevel::Own).await?;
    if !is_trashed {
        return Err(ApiError::Forbidden);
    }
    counter::inc(MetricKey::new(
        "doc.purged_total",
        &[("doc_type", meta.doc_type.as_str())],
    ));

    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::Internal("User not found".to_string()))?;

    // Clean up reverse-relationships on other docs before we wipe this one.
    let reverse_rels = state
        .doc_repo
        .list_reverse_relationships(&id, None)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    for rel in reverse_rels {
        let _ = state
            .doc_repo
            .delete_relationship(&rel.source_doc_id, &rel.relation_type, &rel.target_doc_id)
            .await;
    }

    state.doc_repo.hard_delete(&id).await?;

    let _ = state
        .folder_repo
        .remove_child(&user.trash_folder_id, &id)
        .await;

    // M-E6: durable audit of the user-initiated hard-delete. The
    // doc row itself is gone after hard_delete, so this audit row
    // (PK=USER#<actor>) is the only forensic trail. `hard = true`
    // pairs with the `hard = false` row from the prior soft-delete
    // so a reader can reconstruct the full lifecycle.
    record_security_event(
        &state,
        &user_id,
        SecurityAuditAction::DocDeleted {
            doc_id: id.clone(),
            hard: true,
        },
    );

    spawn_delete_from_index(&state, id);

    Ok(StatusCode::NO_CONTENT)
}

/// Load the caller-visible Y.Doc state for `doc_id`: the S3 snapshot with
/// every pending `UPDATE#` row applied on top. This is the shape the client
/// expects at `GET /content` — used everywhere the server needs to inspect
/// (export), duplicate (copy), or scan (mail-merge placeholders) a doc's
/// current content, not just its last-compacted snapshot.
///
/// Loading only the snapshot would miss content typed since the last
/// `put_content` (the common case for a freshly-marked template that
/// hasn't been compacted yet), leading to false-empty exports and blank
/// copies. This helper collapses the pattern that was open-coded across
/// `get_content`, `export_document`, `bulk_export`, `list_templates`, and
/// `copy_document` — Phase 2's own bug fix was exactly this merge in two
/// of those sites, and any future defensive change here now lands once.
///
/// Returns `Internal("Snapshot not found")` when the metadata has no
/// `snapshot_s3_key` at all (a corrupted row — every doc gets an initial
/// snapshot at create time). Update-decode errors surface as
/// `Internal` — a bad UPDATE# row indicates op-log corruption or a
/// version-skew bug, not a caller mistake.
async fn load_current_doc_state(state: &AppState, doc_id: &str) -> Result<OgreDoc, ApiError> {
    let snapshot = state
        .doc_repo
        .load_snapshot(doc_id)
        .await?
        .ok_or_else(|| ApiError::Internal("Snapshot not found".to_string()))?;
    let mut doc = OgreDoc::from_state_bytes(&snapshot)?;
    let updates = state
        .doc_repo
        .get_pending_updates(doc_id, state.config.max_pending_updates_bytes)
        .await?;
    for update in &updates {
        doc.apply_update(&update.update_bytes)?;
    }
    Ok(doc)
}

/// GET /documents/:id/content -- load Y.Doc state as binary. Owner may read
/// content for a trashed document (so the read-only banner view can render);
/// everyone else gets 404 via `check_doc_access_allow_deleted`.
async fn get_content(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<(HeaderMap, Bytes), ApiError> {
    let (_meta, _is_trashed) = check_doc_access_allow_deleted(
        &state, &id, &user_id,
        ogrenotes_storage::models::AccessLevel::View,
    ).await?;

    let doc = load_current_doc_state(&state, &id).await?;
    let state_bytes = doc.to_state_bytes();
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/octet-stream".parse().unwrap());

    Ok((headers, Bytes::from(state_bytes)))
}

/// PUT /documents/:id/content -- save Y.Doc state as binary.
async fn put_content(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    // M-E7 item 10: per-user content-write rate limit. Real
    // autosave is debounced, so legitimate traffic stays well
    // under the default cap; this bounds compromised-token write
    // floods. Runs before the body-size check so an oversize+
    // high-rate spray pays the rate-limit cost before the size
    // check fails (size check is allocation-free anyway).
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "content_write",
        &user_id,
        state.config.rate_limit_content_write_per_min,
        60,
    )
    .await?;
    // Enforce body size limit
    if body.len() > MAX_CONTENT_SIZE {
        return Err(ApiError::BadRequest(format!(
            "Content too large: {} bytes (max {})",
            body.len(),
            MAX_CONTENT_SIZE
        )));
    }

    // Require Edit access to save content
    let meta = check_doc_access(&state, &id, &user_id, ogrenotes_storage::models::AccessLevel::Edit).await?;

    // #140: a locked document is a doc-wide freeze — reject the REST write for
    // everyone, including the owner (who unlocks via PUT /documents/{id}/lock,
    // not by writing content). Server-enforced sibling of the WS read-only
    // token downgrade in create_ws_token; mirrors the #111 boundary.
    if meta.locked {
        counter::inc(MetricKey::new("doc.locked_write_rejected_total", &[("path", "rest")]));
        return Err(ApiError::ForbiddenMsg(
            "Document is locked for editing".to_string(),
        ));
    }

    histogram::record(
        MetricKey::new("doc.content_bytes", &[]),
        body.len() as f64,
    );

    // Validate that the bytes are a valid Y.Doc state
    let _doc = OgreDoc::from_state_bytes(&body)?;

    // Phase 2a — LiveApp attribute gate. Walk the reconstructed
    // doc and enforce block-attribute schema in the same three
    // states as the WS path (off / log / reject). REST full-state
    // uploads are lower-cadence than WS incremental updates, so
    // the walk cost is not on any hot path.
    //
    // gap-001 exemption: same escape hatch as the WS handler — if
    // this doc's id is in the operator-set exempt list, skip the
    // gate entirely for this write. Lets an operator repair a
    // doc whose current attrs would block every WS write.
    let liveapp_mode = if state.config.liveapp_gate_exempt_doc_ids.contains(&id) {
        counter::inc(MetricKey::new(
            "liveapp.gate_exempted_total",
            &[("path", "rest"), ("doc_id", id.as_str())],
        ));
        ogrenotes_collab::blocks::LiveAppValidationMode::Off
    } else {
        ogrenotes_collab::blocks::LiveAppValidationMode::from_env_value(
            Some(state.config.liveapp_strict_validation.as_str()),
        )
    };
    if liveapp_mode != ogrenotes_collab::blocks::LiveAppValidationMode::Off {
        // The REST path uploads a whole new state, not an
        // incremental update. There is no "changed" subtree to
        // scope down to — every element of the reconstructed doc
        // is effectively new from the server's perspective. Use
        // walk_doc unconditionally, ignoring the WalkScope knob
        // (which is only meaningful when the update is a delta).
        let violations = ogrenotes_collab::blocks::walk_liveapp_violations(_doc.inner());
        if let Some(first) = ogrenotes_collab::blocks::emit_violations_and_should_reject(
            &violations,
            liveapp_mode,
            &[("path", "rest")],
        ) {
            return Err(ApiError::BadRequest(format!(
                "liveapp validation rejected {} content: {}: {}",
                first.node_type.tag_name(),
                first.field,
                first.reason,
            )));
        }
    }

    // Optimistic-locked snapshot write: bump the version only if no
    // concurrent writer advanced it. The repo owns the S3 write, the
    // conditional DynamoDB bump, and the best-effort SNAPSHOT# row.
    let new_version = meta.snapshot_version + 1;
    let outcome = state
        .doc_repo
        .save_snapshot_conditional(
            &id,
            &body,
            meta.snapshot_version,
            new_version,
            now_usec(),
            &user_id,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if outcome == SnapshotWrite::VersionConflict {
        return Err(ApiError::Conflict(
            "Document was modified concurrently -- reload and retry".to_string(),
        ));
    }

    // Record edit activity — deduped against WS-path edits via the
    // process-shared debouncer so a user autosaving over REST while
    // also pushing CRDT updates doesn't spam the activity feed.
    if state.edit_activity_debouncer.try_record_now(&id, &user_id) {
        let activity_repo = state.activity_repo.clone();
        let act_doc_id = id.clone();
        let act_user_id = user_id.clone();
        tokio::spawn(async move {
            let activity = ogrenotes_storage::models::activity::Activity {
                activity_id: nanoid::nanoid!(16),
                doc_id: act_doc_id,
                event_type: ogrenotes_storage::models::activity::ActivityEventType::Edit,
                actor_id: act_user_id,
                detail: serde_json::json!({ "version": new_version }).to_string(),
                created_at: ogrenotes_common::time::now_usec(),
            };
            let _ = activity_repo.create(&activity).await;
        });
    }

    // Re-index with new content
    spawn_index_document_from_bytes(&state, meta, body.to_vec());

    Ok(StatusCode::NO_CONTENT)
}

/// GET /documents/:id/export/:format -- export as html or markdown.
async fn export_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, format)): Path<(String, String)>,
) -> Result<axum::response::Response, ApiError> {
    let _meta = get_verified_doc(&state, &id, &user_id).await?;

    let doc = load_current_doc_state(&state, &id).await?;

    match format.as_str() {
        "html" | "markdown" | "md" | "csv" => {
            counter::inc(MetricKey::new("doc.export_total", &[("format", format.as_str())]));
            let (content_type, content) = match format.as_str() {
                "html" => ("text/html; charset=utf-8", export::to_html(doc.inner())),
                "markdown" | "md" => ("text/markdown; charset=utf-8", export::to_markdown(doc.inner())),
                "csv" => ("text/csv; charset=utf-8", export::to_csv(doc.inner())),
                _ => unreachable!(),
            };
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", content_type.parse().unwrap());
            Ok((headers, content).into_response())
        }
        "xlsx" => {
            counter::inc(MetricKey::new("doc.export_total", &[("format", "xlsx")]));
            let bytes = export::to_xlsx(doc.inner());
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".parse().unwrap());
            headers.insert("Content-Disposition", "attachment; filename=\"export.xlsx\"".parse().unwrap());
            Ok((headers, bytes).into_response())
        }
        "docx" => {
            counter::inc(MetricKey::new("doc.export_total", &[("format", "docx")]));
            let bytes = export::to_docx(doc.inner());
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", "application/vnd.openxmlformats-officedocument.wordprocessingml.document".parse().unwrap());
            headers.insert("Content-Disposition", "attachment; filename=\"export.docx\"".parse().unwrap());
            Ok((headers, bytes).into_response())
        }
        #[cfg(feature = "pdf")]
        "pdf" => {
            counter::inc(MetricKey::new("doc.export_total", &[("format", "pdf")]));
            let bytes = export::to_pdf(doc.inner());
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", "application/pdf".parse().unwrap());
            headers.insert("Content-Disposition", "attachment; filename=\"export.pdf\"".parse().unwrap());
            Ok((headers, bytes).into_response())
        }
        #[cfg(not(feature = "pdf"))]
        "pdf" => Err(ApiError::BadRequest(
            "PDF export not compiled into this build".into(),
        )),
        _ => Err(ApiError::BadRequest(format!("Unsupported export format: {format}"))),
    }
}

// ─── Bulk delete / restore (Phase 5 M-P7 piece A) ────────────────

/// Shared cap across every `POST /documents/bulk/*` route. Larger
/// batches need the Phase 6 async-worker subsystem.
const BULK_OP_MAX: usize = 100;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkDeleteRequest {
    doc_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkRestoreRequest {
    doc_ids: Vec<String>,
    /// All restored docs land in this folder. Same shape as the
    /// single-doc /{id}/restore endpoint; per-id target folders
    /// aren't supported in v1.
    target_folder_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BulkOpResultEntry {
    doc_id: String,
    /// 200 on success; 403/404 mirror the equivalent single-doc
    /// HTTP status; 500 on an unexpected backend error during the
    /// per-id work.
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BulkOpResponse {
    results: Vec<BulkOpResultEntry>,
    /// How many of the input ids landed at status=200. The client
    /// can use this for a "deleted N of M" toast without re-counting
    /// the results array.
    succeeded: usize,
    failed: usize,
}

/// HTTP status code mapper for the bulk response. All-success
/// → 200; any failure → 207 Multi-Status. The body shape is the
/// same in both cases so the client doesn't need to branch on
/// status code to parse.
fn bulk_status(succeeded: usize, total: usize) -> StatusCode {
    if succeeded == total {
        StatusCode::OK
    } else {
        StatusCode::MULTI_STATUS
    }
}

/// POST /documents/bulk/delete — soft-delete up to 100 docs.
/// Per-id authz: requires Own access on each doc (matches the
/// single-doc DELETE). Partial failures don't abort the batch.
async fn bulk_delete(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<BulkDeleteRequest>,
) -> Result<axum::response::Response, ApiError> {
    if req.doc_ids.len() > BULK_OP_MAX {
        return Err(ApiError::BadRequest(format!(
            "bulk delete limit is {BULK_OP_MAX} ids; got {}",
            req.doc_ids.len()
        )));
    }

    crate::middleware::rate_limit::enforce(
        &state.redis,
        "bulk_op",
        &user_id,
        state.config.rate_limit_bulk_op_per_min,
        60,
    )
    .await?;

    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::Internal("User not found".to_string()))?;
    let trash_folder_id = user.trash_folder_id.clone();

    let mut results: Vec<BulkOpResultEntry> = Vec::with_capacity(req.doc_ids.len());
    let mut succeeded: usize = 0;

    for doc_id in &req.doc_ids {
        match try_soft_delete_one(&state, doc_id, &user_id, &trash_folder_id).await {
            Ok(()) => {
                results.push(BulkOpResultEntry {
                    doc_id: doc_id.clone(),
                    status: 200,
                    error: None,
                });
                succeeded += 1;
            }
            Err(BulkOpError::NotFound) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 404,
                error: Some("not found".to_string()),
            }),
            Err(BulkOpError::Forbidden) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 403,
                error: Some("access denied".to_string()),
            }),
            Err(BulkOpError::Internal(msg)) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 500,
                error: Some(msg),
            }),
        }
    }

    let failed = results.len() - succeeded;
    let status = bulk_status(succeeded, req.doc_ids.len());
    counter::inc(MetricKey::new(
        "doc.bulk_delete_total",
        &[("succeeded", &succeeded.to_string())],
    ));

    use axum::response::IntoResponse;
    Ok((
        status,
        Json(BulkOpResponse { results, succeeded, failed }),
    )
        .into_response())
}

/// POST /documents/bulk/restore — un-trash up to 100 docs into a
/// single target folder. Mirrors the single-doc /restore semantics
/// per id.
async fn bulk_restore(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<BulkRestoreRequest>,
) -> Result<axum::response::Response, ApiError> {
    if req.doc_ids.len() > BULK_OP_MAX {
        return Err(ApiError::BadRequest(format!(
            "bulk restore limit is {BULK_OP_MAX} ids; got {}",
            req.doc_ids.len()
        )));
    }
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "bulk_op",
        &user_id,
        state.config.rate_limit_bulk_op_per_min,
        60,
    )
    .await?;

    // Verify the target folder is accessible to the caller before
    // walking the doc list. A bad folder id fails the whole batch
    // up-front (a 400, not per-id) — keeps the failure mode
    // unambiguous for the multi-select UI.
    let _ = super::folders::check_folder_access(
        &state,
        &req.target_folder_id,
        &user_id,
        AccessLevel::Edit,
    )
    .await?;

    let mut results: Vec<BulkOpResultEntry> = Vec::with_capacity(req.doc_ids.len());
    let mut succeeded: usize = 0;

    for doc_id in &req.doc_ids {
        match try_restore_one(&state, doc_id, &user_id, &req.target_folder_id).await {
            Ok(()) => {
                results.push(BulkOpResultEntry {
                    doc_id: doc_id.clone(),
                    status: 200,
                    error: None,
                });
                succeeded += 1;
            }
            Err(BulkOpError::NotFound) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 404,
                error: Some("not found".to_string()),
            }),
            Err(BulkOpError::Forbidden) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 403,
                error: Some("access denied".to_string()),
            }),
            Err(BulkOpError::Internal(msg)) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 500,
                error: Some(msg),
            }),
        }
    }

    let failed = results.len() - succeeded;
    let status = bulk_status(succeeded, req.doc_ids.len());
    counter::inc(MetricKey::new(
        "doc.bulk_restore_total",
        &[("succeeded", &succeeded.to_string())],
    ));

    use axum::response::IntoResponse;
    Ok((
        status,
        Json(BulkOpResponse { results, succeeded, failed }),
    )
        .into_response())
}

enum BulkOpError {
    NotFound,
    Forbidden,
    Internal(String),
}

/// Inner per-doc soft-delete, factored so bulk_delete can collect
/// per-id outcomes without losing the audit / activity-feed /
/// folder-membership steps the single-doc handler performs.
async fn try_soft_delete_one(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
    trash_folder_id: &str,
) -> Result<(), BulkOpError> {
    let meta = match check_doc_access(state, doc_id, user_id, AccessLevel::Own).await {
        Ok(m) => m,
        Err(ApiError::NotFound(_)) => return Err(BulkOpError::NotFound),
        Err(ApiError::Forbidden) => return Err(BulkOpError::Forbidden),
        Err(e) => return Err(BulkOpError::Internal(e.to_string())),
    };
    if meta.is_deleted {
        // Idempotent — count as a success without rewriting state.
        return Ok(());
    }
    let now = now_usec();
    state
        .doc_repo
        .soft_delete(doc_id, now)
        .await
        .map_err(|e| BulkOpError::Internal(e.to_string()))?;

    // #149: leave every folder (primary + additional), then clear the
    // additional set — same purge-all-locations semantics as single-doc
    // delete.
    for folder_id in meta.folder_id.iter().chain(meta.additional_folder_ids.iter()) {
        let _ = state.folder_repo.remove_child(folder_id, doc_id).await;
    }
    if !meta.additional_folder_ids.is_empty() {
        let _ = state.doc_repo.clear_additional_folders(doc_id, now).await;
    }
    let trash_child = FolderChild {
        folder_id: trash_folder_id.to_string(),
        child_id: doc_id.to_string(),
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&trash_child)
        .await
        .map_err(|e| BulkOpError::Internal(e.to_string()))?;

    counter::inc(MetricKey::new(
        "doc.deleted_total",
        &[("doc_type", meta.doc_type.as_str())],
    ));
    record_security_event(
        state,
        user_id,
        SecurityAuditAction::DocDeleted {
            doc_id: doc_id.to_string(),
            hard: false,
        },
    );

    let activity_repo = state.activity_repo.clone();
    let act_doc_id = doc_id.to_string();
    let act_user_id = user_id.to_string();
    tokio::spawn(async move {
        let activity = ogrenotes_storage::models::activity::Activity {
            activity_id: nanoid::nanoid!(16),
            doc_id: act_doc_id,
            event_type: ogrenotes_storage::models::activity::ActivityEventType::Delete,
            actor_id: act_user_id,
            detail: serde_json::json!({}).to_string(),
            created_at: now_usec(),
        };
        let _ = activity_repo.create(&activity).await;
    });

    spawn_delete_from_index(state, doc_id.to_string());
    Ok(())
}

/// Inner per-doc restore. Caller has already verified the target
/// folder's access level once (up-front) so we only re-check doc
/// access here.
async fn try_restore_one(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
    target_folder_id: &str,
) -> Result<(), BulkOpError> {
    let (mut meta, is_trashed) = match check_doc_access_allow_deleted(
        state,
        doc_id,
        user_id,
        AccessLevel::Own,
    )
    .await
    {
        Ok(pair) => pair,
        Err(ApiError::NotFound(_)) => return Err(BulkOpError::NotFound),
        Err(ApiError::Forbidden) => return Err(BulkOpError::Forbidden),
        Err(e) => return Err(BulkOpError::Internal(e.to_string())),
    };
    if !is_trashed {
        // Idempotent — already in a normal folder; treat as success.
        return Ok(());
    }

    let now = now_usec();
    state
        .doc_repo
        .restore(doc_id, target_folder_id, now)
        .await
        .map_err(|e| BulkOpError::Internal(e.to_string()))?;

    // Remove from trash folder + add to target.
    let user = match state.user_repo.get_by_id(user_id).await {
        Ok(Some(u)) => u,
        Ok(None) => return Err(BulkOpError::Internal("user not found".to_string())),
        Err(e) => return Err(BulkOpError::Internal(e.to_string())),
    };
    let _ = state.folder_repo.remove_child(&user.trash_folder_id, doc_id).await;
    let target_child = FolderChild {
        folder_id: target_folder_id.to_string(),
        child_id: doc_id.to_string(),
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&target_child)
        .await
        .map_err(|e| BulkOpError::Internal(e.to_string()))?;

    meta.folder_id = Some(target_folder_id.to_string());
    counter::inc(MetricKey::new(
        "doc.restored_total",
        &[("doc_type", meta.doc_type.as_str())],
    ));

    let activity_repo = state.activity_repo.clone();
    let act_doc_id = doc_id.to_string();
    let act_user_id = user_id.to_string();
    let act_dest = target_folder_id.to_string();
    tokio::spawn(async move {
        let activity = ogrenotes_storage::models::activity::Activity {
            activity_id: nanoid::nanoid!(16),
            doc_id: act_doc_id,
            event_type: ogrenotes_storage::models::activity::ActivityEventType::Restore,
            actor_id: act_user_id,
            detail: serde_json::json!({ "targetFolderId": act_dest }).to_string(),
            created_at: now_usec(),
        };
        let _ = activity_repo.create(&activity).await;
    });

    Ok(())
}

// ─── Embed URL resolver (Phase 5 M-P6 piece B) ───────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveEmbedRequest {
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolveEmbedResponse {
    /// Provider tag, in the `provider.to_attr()` shape. The
    /// frontend stores it verbatim on the embed node.
    provider: String,
    /// Iframe-ready URL — may differ from the input when the
    /// allowlist rewrites watch URLs / share URLs into embed
    /// URLs. The frontend stores it as the embed's `url`
    /// attribute (the cell input becomes the iframe `src`).
    src: String,
    /// Provider-specific default height for a fresh insert.
    /// Frontend clamps the in-DOM height to [200, 1200].
    height: u32,
}

async fn resolve_embed(
    State(state): State<AppState>,
    AuthUser { user_id: _, .. }: AuthUser,
    Json(req): Json<ResolveEmbedRequest>,
) -> Result<Json<ResolveEmbedResponse>, ApiError> {
    // v1: workspace-allowlisted-domains is unconfigured (the
    // model field isn't wired through admin yet), so Generic
    // URLs fall through to UnknownProvider. Once admin grows the
    // setting we'll thread the per-workspace allowlist in here.
    let allowed: std::collections::HashSet<String> = std::collections::HashSet::new();
    match crate::embed_allowlist::validate_url(&req.url, &allowed) {
        Ok((provider, src)) => Ok(Json(ResolveEmbedResponse {
            height: provider.default_height(),
            provider: provider.to_attr(),
            src,
        })),
        Err(crate::embed_allowlist::EmbedRejection::NotHttps) => Err(ApiError::BadRequest(
            "embed URL must use https://".to_string(),
        )),
        Err(crate::embed_allowlist::EmbedRejection::UnknownProvider) => Err(ApiError::BadRequest(
            "unsupported embed provider — allowed: YouTube, Vimeo, Figma, Loom, CodeSandbox".to_string(),
        )),
    }
}

// ─── Bulk move / share (Phase 5 M-P7 piece B) ────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkMoveRequest {
    doc_ids: Vec<String>,
    dest_folder_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkShareRequest {
    doc_ids: Vec<String>,
    member_id: String,
    /// Access level for the new membership row. Same semantics as
    /// the single-doc /sharing add endpoint — `Own` is rejected;
    /// `Edit | Comment | View` accepted.
    access_level: AccessLevel,
}

/// POST /documents/bulk/move — move up to 100 docs to a single
/// destination folder. Per-doc Edit access required; up-front
/// folder-access check on the destination. Per-id failures land
/// in the 207 manifest.
async fn bulk_move(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<BulkMoveRequest>,
) -> Result<axum::response::Response, ApiError> {
    if req.doc_ids.len() > BULK_OP_MAX {
        return Err(ApiError::BadRequest(format!(
            "bulk move limit is {BULK_OP_MAX} ids; got {}",
            req.doc_ids.len()
        )));
    }
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "bulk_op",
        &user_id,
        state.config.rate_limit_bulk_op_per_min,
        60,
    )
    .await?;

    // Dest folder check up-front; same rationale as bulk_restore —
    // every id moves into the same target so a single check is
    // unambiguous, and a bad folder id should fail the whole batch
    // with 4xx, not generate N identical 403s.
    let _ = super::folders::check_folder_access(
        &state,
        &req.dest_folder_id,
        &user_id,
        AccessLevel::Edit,
    )
    .await?;

    let mut results: Vec<BulkOpResultEntry> = Vec::with_capacity(req.doc_ids.len());
    let mut succeeded: usize = 0;

    for doc_id in &req.doc_ids {
        match try_move_one(&state, doc_id, &user_id, &req.dest_folder_id).await {
            Ok(()) => {
                results.push(BulkOpResultEntry {
                    doc_id: doc_id.clone(),
                    status: 200,
                    error: None,
                });
                succeeded += 1;
            }
            Err(BulkOpError::NotFound) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 404,
                error: Some("not found".to_string()),
            }),
            Err(BulkOpError::Forbidden) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 403,
                error: Some("access denied".to_string()),
            }),
            Err(BulkOpError::Internal(msg)) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 500,
                error: Some(msg),
            }),
        }
    }

    let failed = results.len() - succeeded;
    let status = bulk_status(succeeded, req.doc_ids.len());
    counter::inc(MetricKey::new(
        "doc.bulk_move_total",
        &[("succeeded", &succeeded.to_string())],
    ));

    use axum::response::IntoResponse;
    Ok((
        status,
        Json(BulkOpResponse { results, succeeded, failed }),
    )
        .into_response())
}

/// POST /documents/bulk/share — add the same recipient as a
/// member on up to 100 docs. Per-doc Own access required (matches
/// the single-doc add-member endpoint).
async fn bulk_share(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<BulkShareRequest>,
) -> Result<axum::response::Response, ApiError> {
    if req.doc_ids.len() > BULK_OP_MAX {
        return Err(ApiError::BadRequest(format!(
            "bulk share limit is {BULK_OP_MAX} ids; got {}",
            req.doc_ids.len()
        )));
    }
    if req.access_level == AccessLevel::Own {
        return Err(ApiError::BadRequest(
            "Cannot grant Own access via sharing".to_string(),
        ));
    }
    if req.member_id == user_id {
        return Err(ApiError::BadRequest(
            "Cannot share with yourself".to_string(),
        ));
    }

    crate::middleware::rate_limit::enforce(
        &state.redis,
        "bulk_op",
        &user_id,
        state.config.rate_limit_bulk_op_per_min,
        60,
    )
    .await?;

    // Recipient existence check up-front; per-doc work shouldn't
    // re-query the same user record N times.
    if state.user_repo.get_by_id(&req.member_id).await?.is_none() {
        return Err(ApiError::NotFound("User not found".to_string()));
    }

    let mut results: Vec<BulkOpResultEntry> = Vec::with_capacity(req.doc_ids.len());
    let mut succeeded: usize = 0;

    for doc_id in &req.doc_ids {
        match try_share_one(
            &state,
            doc_id,
            &user_id,
            &req.member_id,
            req.access_level.clone(),
        )
        .await
        {
            Ok(()) => {
                results.push(BulkOpResultEntry {
                    doc_id: doc_id.clone(),
                    status: 200,
                    error: None,
                });
                succeeded += 1;
            }
            Err(BulkOpError::NotFound) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 404,
                error: Some("not found".to_string()),
            }),
            Err(BulkOpError::Forbidden) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 403,
                error: Some("access denied".to_string()),
            }),
            Err(BulkOpError::Internal(msg)) => results.push(BulkOpResultEntry {
                doc_id: doc_id.clone(),
                status: 500,
                error: Some(msg),
            }),
        }
    }

    let failed = results.len() - succeeded;
    let status = bulk_status(succeeded, req.doc_ids.len());
    counter::inc(MetricKey::new(
        "doc.bulk_share_total",
        &[("succeeded", &succeeded.to_string())],
    ));

    use axum::response::IntoResponse;
    Ok((
        status,
        Json(BulkOpResponse { results, succeeded, failed }),
    )
        .into_response())
}

async fn try_move_one(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
    dest_folder_id: &str,
) -> Result<(), BulkOpError> {
    let meta = match check_doc_access(state, doc_id, user_id, AccessLevel::Edit).await {
        Ok(m) => m,
        Err(ApiError::NotFound(_)) => return Err(BulkOpError::NotFound),
        Err(ApiError::Forbidden) => return Err(BulkOpError::Forbidden),
        Err(e) => return Err(BulkOpError::Internal(e.to_string())),
    };
    // Idempotent: already in the target folder.
    if meta.folder_id.as_deref() == Some(dest_folder_id) {
        return Ok(());
    }

    let now = now_usec();
    state
        .doc_repo
        .set_folder(doc_id, dest_folder_id, now)
        .await
        .map_err(|e| BulkOpError::Internal(e.to_string()))?;

    // Folder-side bookkeeping: remove from source, add to dest.
    if let Some(ref source) = meta.folder_id {
        let _ = state.folder_repo.remove_child(source, doc_id).await;
    }
    let child = FolderChild {
        folder_id: dest_folder_id.to_string(),
        child_id: doc_id.to_string(),
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&child)
        .await
        .map_err(|e| BulkOpError::Internal(e.to_string()))?;

    let activity_repo = state.activity_repo.clone();
    let act_doc_id = doc_id.to_string();
    let act_user_id = user_id.to_string();
    let act_dest = dest_folder_id.to_string();
    let act_source = meta.folder_id.clone();
    tokio::spawn(async move {
        let activity = ogrenotes_storage::models::activity::Activity {
            activity_id: nanoid::nanoid!(16),
            doc_id: act_doc_id,
            event_type: ogrenotes_storage::models::activity::ActivityEventType::Move,
            actor_id: act_user_id,
            detail: serde_json::json!({
                "sourceFolderId": act_source,
                "destFolderId": act_dest,
            })
            .to_string(),
            created_at: now_usec(),
        };
        let _ = activity_repo.create(&activity).await;
    });

    Ok(())
}

async fn try_share_one(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
    member_id: &str,
    access_level: AccessLevel,
) -> Result<(), BulkOpError> {
    let _meta = match check_doc_access(state, doc_id, user_id, AccessLevel::Own).await {
        Ok(m) => m,
        Err(ApiError::NotFound(_)) => return Err(BulkOpError::NotFound),
        Err(ApiError::Forbidden) => return Err(BulkOpError::Forbidden),
        Err(e) => return Err(BulkOpError::Internal(e.to_string())),
    };

    // The single-doc add_member endpoint caps members per doc;
    // mirror that check here so a bulk share doesn't sneak past
    // the per-doc cap.
    let existing = state
        .doc_repo
        .list_doc_members(doc_id)
        .await
        .map_err(|e| BulkOpError::Internal(e.to_string()))?;
    if existing.iter().any(|m| m.user_id == member_id) {
        // Idempotent: already a member — treat as success without
        // re-writing. v1 doesn't change the access level here; a
        // future "promote/demote" UI uses the update-member endpoint.
        return Ok(());
    }
    if existing.len() >= state.config.max_members_per_doc {
        return Err(BulkOpError::Internal(format!(
            "doc {doc_id} reached membership cap"
        )));
    }

    let member = ogrenotes_storage::models::document::DocMember {
        doc_id: doc_id.to_string(),
        user_id: member_id.to_string(),
        access_level,
        added_at: now_usec(),
    };
    state
        .doc_repo
        .add_doc_member(&member)
        .await
        .map_err(|e| BulkOpError::Internal(e.to_string()))?;

    let activity_repo = state.activity_repo.clone();
    let act_doc_id = doc_id.to_string();
    let act_user_id = user_id.to_string();
    let act_target = member_id.to_string();
    tokio::spawn(async move {
        let activity = ogrenotes_storage::models::activity::Activity {
            activity_id: nanoid::nanoid!(16),
            doc_id: act_doc_id,
            event_type: ogrenotes_storage::models::activity::ActivityEventType::Share,
            actor_id: act_user_id,
            detail: serde_json::json!({ "sharedWith": act_target }).to_string(),
            created_at: now_usec(),
        };
        let _ = activity_repo.create(&activity).await;
    });

    Ok(())
}

// ─── Bulk export (Phase 5 M-P5 piece C) ──────────────────────────

/// Max docs per bulk-export call. Above this the request rejects
/// with 400 rather than degrading silently. Larger exports wait
/// for the Phase 6 async-worker subsystem.
const BULK_EXPORT_MAX: usize = 100;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkExportRequest {
    doc_ids: Vec<String>,
    /// "markdown" | "md" | "html". Other formats (xlsx, csv) need
    /// per-format binary handling and aren't part of v1 bulk.
    format: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BulkExportManifestEntry {
    doc_id: String,
    /// HTTP-style status code per id. 200 = exported into the
    /// archive; 403 = access denied; 404 = not found / trashed.
    status: u16,
    /// File name within the zip for successful entries; absent for
    /// failures.
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    /// Human-readable error reason when status != 200.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// POST /documents/bulk/export — zip up to 100 docs into one
/// download. Per-id authz is checked individually; one denied or
/// missing doc doesn't fail the whole batch. The archive embeds a
/// `_manifest.json` listing every requested id with its status.
///
/// Response shape:
///   - At least one success → 200 OK, body = application/zip.
///   - Zero successes      → 207 Multi-Status, body = JSON listing
///                            the per-id failures. (Saves the
///                            client from unzipping a manifest-only
///                            archive when nothing succeeded.)
///   - > 100 ids or unknown format → 400.
async fn bulk_export(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<BulkExportRequest>,
) -> Result<axum::response::Response, ApiError> {
    if req.doc_ids.len() > BULK_EXPORT_MAX {
        return Err(ApiError::BadRequest(format!(
            "bulk export limit is {BULK_EXPORT_MAX} ids per request; got {} — use async export when shipped in Phase 6",
            req.doc_ids.len()
        )));
    }

    // Resolve format up-front so we can fail fast on garbage input
    // before opening Redis for the rate limit.
    let (ext, content_kind) = match req.format.as_str() {
        "markdown" | "md" => ("md", BulkFormat::Markdown),
        "html" => ("html", BulkFormat::Html),
        other => {
            return Err(ApiError::BadRequest(format!(
                "unsupported bulk export format: {other}"
            )));
        }
    };

    crate::middleware::rate_limit::enforce(
        &state.redis,
        "bulk_export",
        &user_id,
        state.config.rate_limit_bulk_export_per_min,
        60,
    )
    .await?;

    let mut zip_buf: Vec<u8> = Vec::new();
    let mut manifest: Vec<BulkExportManifestEntry> = Vec::with_capacity(req.doc_ids.len());
    let mut success_count: usize = 0;

    {
        let cursor = std::io::Cursor::new(&mut zip_buf);
        let mut writer = zip::ZipWriter::new(cursor);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // Track filename collisions inside the archive — two docs
        // titled "Untitled" would otherwise overwrite each other.
        // Suffix with "-<n>" on repeat. v1 keeps it dumb; Phase 6's
        // async exporter can do something fancier.
        let mut filename_dedup: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();

        for doc_id in &req.doc_ids {
            match try_export_one(&state, doc_id, &user_id, content_kind).await {
                Ok((title, body)) => {
                    let stem = sanitize_filename(&title);
                    let key = format!("{stem}.{ext}");
                    let entry_count = filename_dedup.entry(key.clone()).or_insert(0);
                    *entry_count += 1;
                    let unique = if *entry_count == 1 {
                        key.clone()
                    } else {
                        let n = *entry_count - 1;
                        format!("{stem}-{n}.{ext}")
                    };
                    // Mid-zip write failures are exceptional — bail
                    // with 500 rather than continue with a partial
                    // archive. `start_file` returns ZipResult<()>;
                    // ZipWriter then implements Write for the body.
                    writer
                        .start_file::<_, ()>(&unique, options)
                        .map_err(|e| ApiError::Internal(e.to_string()))?;
                    std::io::Write::write_all(&mut writer, body.as_bytes())
                        .map_err(|e| ApiError::Internal(e.to_string()))?;
                    manifest.push(BulkExportManifestEntry {
                        doc_id: doc_id.clone(),
                        status: 200,
                        filename: Some(unique),
                        error: None,
                    });
                    success_count += 1;
                }
                Err(BulkExportError::NotFound) => {
                    manifest.push(BulkExportManifestEntry {
                        doc_id: doc_id.clone(),
                        status: 404,
                        filename: None,
                        error: Some("not found".to_string()),
                    });
                }
                Err(BulkExportError::Forbidden) => {
                    manifest.push(BulkExportManifestEntry {
                        doc_id: doc_id.clone(),
                        status: 403,
                        filename: None,
                        error: Some("access denied".to_string()),
                    });
                }
                Err(BulkExportError::Internal(msg)) => {
                    manifest.push(BulkExportManifestEntry {
                        doc_id: doc_id.clone(),
                        status: 500,
                        filename: None,
                        error: Some(msg),
                    });
                }
            }
        }

        // Always write the manifest, even on all-failure runs — the
        // client gets a consistent shape and can correlate
        // doc_id → outcome without server-side state.
        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        writer
            .start_file::<_, ()>("_manifest.json", options)
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        std::io::Write::write_all(&mut writer, &manifest_json)
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        writer
            .finish()
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }

    counter::inc(MetricKey::new(
        "doc.bulk_export_total",
        &[("format", ext)],
    ));
    histogram::record(
        MetricKey::new("doc.bulk_export_doc_count", &[]),
        req.doc_ids.len() as f64,
    );

    use axum::response::IntoResponse;
    if success_count == 0 {
        // Nothing succeeded — return the manifest JSON only with a
        // 207 Multi-Status status. Avoids the awkward zip-with-only-
        // a-manifest-inside response shape.
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        return Ok((
            StatusCode::MULTI_STATUS,
            headers,
            serde_json::to_vec(&manifest).map_err(|e| ApiError::Internal(e.to_string()))?,
        )
            .into_response());
    }

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/zip".parse().unwrap());
    headers.insert(
        "Content-Disposition",
        "attachment; filename=\"ogrenotes-export.zip\""
            .parse()
            .unwrap(),
    );
    Ok((StatusCode::OK, headers, zip_buf).into_response())
}

#[derive(Clone, Copy)]
enum BulkFormat {
    Markdown,
    Html,
}

enum BulkExportError {
    NotFound,
    Forbidden,
    Internal(String),
}

async fn try_export_one(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
    format: BulkFormat,
) -> Result<(String, String), BulkExportError> {
    let meta = match check_doc_access(
        state,
        doc_id,
        user_id,
        ogrenotes_storage::models::AccessLevel::View,
    )
    .await
    {
        Ok(m) => m,
        Err(ApiError::NotFound(_)) => return Err(BulkExportError::NotFound),
        Err(ApiError::Forbidden) => return Err(BulkExportError::Forbidden),
        Err(e) => return Err(BulkExportError::Internal(e.to_string())),
    };

    let doc = load_current_doc_state(state, doc_id)
        .await
        .map_err(|e| BulkExportError::Internal(e.to_string()))?;
    let body = match format {
        BulkFormat::Markdown => export::to_markdown(doc.inner()),
        BulkFormat::Html => export::to_html(doc.inner()),
    };
    Ok((meta.title, body))
}

/// Strip filesystem-unsafe characters and length-cap a title so it
/// can ride as a zip entry name on Windows / macOS / Linux clients.
/// Conservative: only ASCII alphanumerics, dashes, and underscores
/// survive; everything else collapses to "_". An empty result falls
/// back to "untitled". Length cap matches Windows MAX_PATH-ish
/// realism.
fn sanitize_filename(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for ch in title.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => out.push(ch),
            ' ' => out.push('_'),
            _ => out.push('_'),
        }
    }
    // Trim leading underscores from the run of replacements at the
    // start so "// my doc" doesn't become "____my_doc".
    let trimmed = out.trim_matches('_');
    let body: String = if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.chars().take(100).collect()
    };
    body
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadRequest {
    filename: String,
    content_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadResponse {
    upload_url: String,
    blob_id: String,
    key: String,
}

/// POST /documents/:id/blobs -- request presigned upload URL.
async fn request_upload_url(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UploadRequest>,
) -> Result<Json<UploadResponse>, ApiError> {
    let _meta = get_verified_doc(&state, &id, &user_id).await?;

    if !is_allowed_content_type(&req.content_type) {
        return Err(ApiError::BadRequest(format!(
            "Content type not allowed: {}",
            req.content_type
        )));
    }

    // Sanitize filename to prevent S3 path traversal.
    let safe_filename: String = req
        .filename
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .collect();
    if safe_filename.is_empty() || safe_filename.starts_with('.') {
        return Err(ApiError::BadRequest("Invalid filename".to_string()));
    }

    let blob_id = new_id();
    let key = format!("blobs/{id}/{blob_id}/{safe_filename}");

    let url = state
        .doc_repo
        .s3()
        .presigned_put_url(&key, &req.content_type, 900)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(UploadResponse {
        upload_url: url,
        blob_id,
        key,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadRequest {
    key: String,
}

/// GET /documents/:id/blobs/:blob_id -- request presigned download URL.
/// Requires the `key` query parameter (returned from the upload response).
async fn request_download_url(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, blob_id)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<DownloadRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _meta = get_verified_doc(&state, &id, &user_id).await?;

    // Verify the key belongs to this document and blob
    let expected_prefix = format!("blobs/{id}/{blob_id}/");
    if !query.key.starts_with(&expected_prefix) {
        return Err(ApiError::BadRequest("Invalid blob key".to_string()));
    }

    let url = state
        .doc_repo
        .s3()
        .presigned_get_url(&query.key, 14400)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "downloadUrl": url
    })))
}

// ─── Link sharing settings ──────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkSettingsResponse {
    link_sharing_mode: Option<ogrenotes_storage::models::LinkSharingMode>,
    view_options: ogrenotes_storage::models::ViewOptions,
    /// Whether the caller may change these settings — true iff they own
    /// the doc (PATCH requires `Own`, and `Own` is non-transferable).
    /// Lets the share dialog render an editable vs. read-only view
    /// without optimistically showing controls that would 403 on save.
    can_manage: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateLinkSettingsRequest {
    link_sharing_mode: Option<ogrenotes_storage::models::LinkSharingMode>,
    /// View-mode sub-options. Omitted ⇒ left unchanged.
    #[serde(default)]
    view_options: Option<ogrenotes_storage::models::ViewOptions>,
}

/// Apply a link-settings change and emit the audit row. Shared by the
/// owner endpoint (below) and the admin override in `routes::admin`. The
/// caller must have **already authorized**; `actor_id` is whoever made
/// the change (the owner on the self-service path, an admin on the
/// override), while the audit row is keyed on the doc owner as subject —
/// so every link-sharing change lands in one `LinkSharingChanged` trail
/// with `actor_id` recording who did it.
pub(crate) async fn apply_link_settings(
    state: &AppState,
    meta: &DocumentMeta,
    req: &UpdateLinkSettingsRequest,
    actor_id: &str,
) -> Result<(), ApiError> {
    state
        .doc_repo
        .update_link_settings(
            &meta.doc_id,
            req.link_sharing_mode.as_ref(),
            req.view_options.as_ref(),
            now_usec(),
        )
        .await?;

    // Audit any link-settings change — both a mode change AND a
    // sub-option change (enabling allow_comments / show_history etc. is a
    // permission change and must leave a trail). A no-op PATCH (neither
    // field present) logs nothing. The row records the RESULTING state
    // (mode + full view-options) keyed on the doc owner as subject; a
    // reader diffs against the prior row to see exactly what moved.
    if req.link_sharing_mode.is_some() || req.view_options.is_some() {
        use ogrenotes_storage::models::LinkSharingMode;
        // Resulting mode: the requested one if set, else the doc's current.
        // The `None` variant means "disabled" → audit `None`.
        let resulting_mode = req
            .link_sharing_mode
            .clone()
            .or_else(|| meta.link_sharing_mode.clone())
            .filter(|m| *m != LinkSharingMode::None);
        // Resulting view-options: the requested set if present, else current.
        let resulting_view_options = req
            .view_options
            .clone()
            .unwrap_or_else(|| meta.link_view_options.clone());
        record_security_event_by_actor(
            state,
            &meta.owner_id,
            actor_id,
            SecurityAuditAction::LinkSharingChanged {
                doc_id: meta.doc_id.clone(),
                mode: resulting_mode,
                view_options: resulting_view_options,
            },
        );
    }
    Ok(())
}

/// GET /documents/:id/link-settings
async fn get_link_settings(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<LinkSettingsResponse>, ApiError> {
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::View).await?;
    let can_manage = meta.owner_id == user_id;
    Ok(Json(LinkSettingsResponse {
        link_sharing_mode: meta.link_sharing_mode,
        view_options: meta.link_view_options,
        can_manage,
    }))
}

/// PATCH /documents/:id/link-settings
async fn update_link_settings(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateLinkSettingsRequest>,
) -> Result<StatusCode, ApiError> {
    // Only owner can change link settings.
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::Own).await?;
    // Per-user sharing rate limit (parity with member-share mutations and
    // the admin override) — bounds churn on the doc partition and the audit
    // log from a compromised/automated owner token. After auth so it counts
    // only authorized requests.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "sharing",
        &user_id,
        state.config.rate_limit_sharing_per_min,
        60,
    )
    .await?;
    // Apply + audit (self-event: the owner is both subject and actor).
    apply_link_settings(&state, &meta, &req, &user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── Edit lock (#140) ───────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetLockRequest {
    locked: bool,
}

/// PUT /documents/:id/lock — toggle the document's edit-lock (#140).
///
/// Owner-only (`Own` is non-transferable), mirroring `update_link_settings`:
/// a locked doc is read-only for *everyone* including editors, so allowing a
/// mere editor to unlock would make the lock meaningless. Enforcement of the
/// resulting state lives on the write paths — `put_content` (REST) and
/// `create_ws_token` → `read_only_permits_frame` (WS) — so this endpoint only
/// records the flag and the audit trail.
async fn set_lock(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<SetLockRequest>,
) -> Result<StatusCode, ApiError> {
    // Only the owner can toggle the lock.
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::Own).await?;
    // Per-user sharing rate limit (parity with link-settings) — bounds churn
    // on the doc partition and the audit log. After auth so it counts only
    // authorized requests.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "sharing",
        &user_id,
        state.config.rate_limit_sharing_per_min,
        60,
    )
    .await?;

    // No-op toggle: don't write or audit a redundant transition.
    if meta.locked == req.locked {
        return Ok(StatusCode::NO_CONTENT);
    }

    state
        .doc_repo
        .set_locked(&id, req.locked, now_usec())
        .await?;

    // Audit the lock transition — a doc-wide write-authority change. Self-event
    // today (owner-only), so subject == actor.
    record_security_event(
        &state,
        &meta.owner_id,
        SecurityAuditAction::DocLockToggled {
            doc_id: id,
            locked: req.locked,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

// ─── Templates (#142) ───────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetTemplateRequest {
    is_template: bool,
}

/// PUT /documents/:id/template — mark or unmark a document as a template (#142).
///
/// Gated on `Edit`: any editor can promote a doc into the workspace template
/// gallery. (Owner-only was considered and rejected — `Mark as Template` lives
/// in the Document menu next to `Duplicate`, which is also editor-visible.)
///
/// The doc stays editable after marking. An earlier revision auto-locked on
/// mark (to prevent accidental edits to the template) but real usage found it
/// more surprising than helpful — the doc
/// suddenly turned read-only mid-flow. Owners can still lock deliberately
/// via `PUT /documents/:id/lock`. No `SecurityAudit` row: this is not in the
/// identity / sharing / destructive set.
async fn set_template(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<SetTemplateRequest>,
) -> Result<StatusCode, ApiError> {
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::Edit).await?;

    // No-op toggle: skip the write so a redundant click doesn't churn updated_at.
    if meta.is_template == req.is_template {
        return Ok(StatusCode::NO_CONTENT);
    }

    state
        .doc_repo
        .set_is_template(&id, req.is_template, now_usec())
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TemplateItem {
    id: String,
    title: String,
    owner_id: String,
    doc_type: String,
    updated_at: i64,
    /// Phase 2 mail merge — unique placeholder keys the template body uses
    /// (e.g. `["name", "user.email"]`), sorted. The picker uses this to
    /// decide whether to prompt for values before copying; empty → no
    /// prompt, straight copy. Computed by scanning the snapshot per row
    /// (one S3 GET per template), which is acceptable at v1 workspace
    /// scale; a `template_placeholders` DDB cache is the v2 lever if
    /// listing latency becomes observable.
    #[serde(default)]
    placeholders: Vec<String>,
    /// Phase 3 — which gallery this row belongs to on the picker UI:
    /// `mine` (caller owns it), `shared` (workspace peer's template
    /// visible to the caller), or `sample` (seeded sample-template
    /// from the SAMPLES_WORKSPACE_ID). The frontend groups rows into
    /// sections by this tag.
    gallery: Gallery,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Gallery {
    /// The caller is the owner of this template.
    Mine,
    /// A template shared with the caller via workspace membership.
    Shared,
    /// A seeded sample template (SAMPLES_WORKSPACE_ID / system user).
    Sample,
    /// Phase 4 — a template that appears via one of the caller's
    /// workspace's admin-curated Company galleries. Carries the
    /// gallery's id and display name so the picker groups rows by
    /// their gallery. A doc that lives in multiple galleries is
    /// listed once per gallery (dedupe keys on `(doc_id, gallery_id)`).
    ///
    /// `rename_all = "camelCase"` on the variant explicitly: the
    /// enum-level `rename_all` renames variant NAMES only, not the
    /// inner struct-variant fields — without this the wire ships
    /// `gallery_id`/`gallery_name` snake_case in violation of the
    /// project-wide camelCase DTO convention.
    #[serde(rename_all = "camelCase")]
    Company {
        gallery_id: String,
        gallery_name: String,
    },
}

/// GET /documents/templates — templates the caller can use (#142).
///
/// Unions four queries and dedupes:
/// - `GSI1-owner-updated` for the caller's own docs — surfaces owner-marked
///   templates regardless of workspace. Necessary because `POST /documents`
///   does not yet default `workspace_id` from the user's default workspace
///   (per design intent it should; that fix is a separate #142 follow-up),
///   so a bare-created template has `workspace_id = null` and would be
///   invisible to a workspace-only query.
/// - `GSI3-workspace-updated` for the caller's default workspace — surfaces
///   templates shared workspace-wide by other members.
/// - `GSI3-workspace-updated` on `SAMPLES_WORKSPACE_ID` — Phase 3 seeded
///   sample templates. Unconditional query with no ACL gate: samples are
///   meant to be visible to every user, so we skip `check_doc_access` for
///   them.
/// - Phase 4 Company galleries for the caller's default workspace — one
///   fan-out per gallery membership, each per-doc View-gated.
///
/// The owner+workspace+company branches filter `is_template && !is_deleted`
/// and per-row re-check `View` access (link sharing, folder inheritance,
/// direct membership). Stale rows the caller has since lost access to drop
/// silently — same posture as `list_favorites`. Caller without a default
/// workspace still gets their own templates + samples; all empty → empty list.
async fn list_templates(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<Json<Vec<TemplateItem>>, ApiError> {
    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await?
        .ok_or_else(|| ApiError::Internal("user record missing after auth".to_string()))?;

    // Fire the four independent DDB queries concurrently — the previous
    // shape awaited them in sequence, so total gallery latency was the sum
    // of the RTTs instead of the slowest single one. All failures
    // propagate with a uniform posture — a partial gallery is more
    // misleading than an error the client can retry.
    let owner_fut = state.doc_repo.query_docs_by_owner(&user_id);
    let workspace_fut = async {
        match user.default_workspace_id.as_deref() {
            Some(ws) => state.doc_repo.query_docs_by_workspace(ws).await,
            None => Ok(Vec::new()),
        }
    };
    let samples_fut = state
        .doc_repo
        .query_docs_by_workspace(crate::seed::SAMPLES_WORKSPACE_ID);
    // Phase 4: pull company galleries for the caller's workspace. Each
    // gallery references an arbitrary set of doc ids; we fetch metadata
    // per doc below (bounded by MAX_GALLERY_DOC_IDS per gallery).
    let company_galleries_fut = async {
        match user.default_workspace_id.as_deref() {
            Some(ws) => state.template_gallery_repo.list_for_workspace(ws).await,
            None => Ok(Vec::new()),
        }
    };
    let (owner_res, workspace_res, samples_res, company_galleries_res) =
        tokio::join!(owner_fut, workspace_fut, samples_fut, company_galleries_fut);

    // Propagate a query failure on any branch with the same posture:
    // a partial gallery is more misleading than an error the client can
    // retry. Previously the samples query was soft (warn + Vec::new())
    // while the sibling queries were hard; the split was a debugging
    // trap where "missing rows" could mean either "you own nothing" or
    // "DDB flaked on one specific index."
    let mut metas = owner_res.map_err(|e| ApiError::Internal(e.to_string()))?;
    metas.extend(workspace_res.map_err(|e| ApiError::Internal(e.to_string()))?);
    let sample_metas = samples_res.map_err(|e| ApiError::Internal(e.to_string()))?;
    let company_galleries =
        company_galleries_res.map_err(|e| ApiError::Internal(e.to_string()))?;

    // Dedupe key is `(doc_id, gallery_dedupe_key)` rather than doc_id
    // alone: a doc that's both the caller's own AND in an admin-curated
    // company gallery should surface in BOTH sections of the picker.
    // Only within a single gallery bucket (Mine, or a specific Company
    // gallery id) do we dedupe by doc_id.
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut allowed: Vec<(ogrenotes_storage::models::document::DocumentMeta, Gallery)> = Vec::new();

    for m in sample_metas {
        // Require the samples system user as the owner — `create_document`
        // does not gate workspace membership on the target `workspaceId`,
        // so without this filter any authenticated user could `POST
        // /documents {workspaceId: "samples-workspace"}` and inject a fake
        // row into every user's gallery.
        if m.owner_id != crate::seed::SAMPLES_SYSTEM_USER_ID {
            continue;
        }
        if !m.is_template || m.is_deleted {
            continue;
        }
        if !seen.insert((m.doc_id.clone(), "sample".to_string())) {
            continue;
        }
        allowed.push((m, Gallery::Sample));
    }
    for m in metas {
        if !m.is_template || m.is_deleted {
            continue;
        }
        let bucket = if m.owner_id == user_id { "mine" } else { "shared" };
        if !seen.insert((m.doc_id.clone(), bucket.to_string())) {
            continue;
        }
        // Per-row View gate: handles link sharing, folder inheritance, direct
        // membership, and trash visibility uniformly. Stale rows drop silently.
        if check_doc_access(&state, &m.doc_id, &user_id, AccessLevel::View)
            .await
            .is_err()
        {
            continue;
        }
        let gallery = if m.owner_id == user_id {
            Gallery::Mine
        } else {
            Gallery::Shared
        };
        allowed.push((m, gallery));
    }

    // Phase 4: fold in company galleries. Each gallery is a curated list
    // of doc ids owned by the admin who added them; the docs themselves
    // live in whatever workspace they were created. Per-doc metadata
    // fetches happen concurrently — one GetItem per unique doc — and
    // then the same View ACL gate applies (a stale membership referring
    // to a doc the caller cannot see drops silently).
    if !company_galleries.is_empty() {
        // Collect the unique doc ids across all galleries so we fetch
        // metadata once even if a doc appears in multiple galleries.
        let mut unique_doc_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for g in &company_galleries {
            for id in &g.doc_ids {
                unique_doc_ids.insert(id.clone());
            }
        }
        let fetch_futs = unique_doc_ids.into_iter().map(|doc_id| {
            let repo = state.doc_repo.clone();
            async move {
                let meta = repo.get(&doc_id).await.ok().flatten();
                (doc_id, meta)
            }
        });
        // Cap concurrency at 16 — MAX_GALLERY_DOC_IDS is 500 and a
        // workspace can hold multiple galleries, so an unbounded
        // `join_all` could fire thousands of concurrent GetItems and
        // slam the DDB partition. 16 keeps the DDB connection pool
        // happy without hurting p50 gallery-open latency in practice.
        use futures_util::StreamExt;
        let fetched: std::collections::HashMap<String, ogrenotes_storage::models::document::DocumentMeta> =
            futures_util::stream::iter(fetch_futs)
                .buffer_unordered(16)
                .filter_map(|(id, m)| async move { m.map(|m| (id, m)) })
                .collect()
                .await;

        // Filter to template+non-trashed and run ONE View ACL check per
        // unique doc, concurrently — a doc that appears in N galleries
        // has the same access answer for every appearance, so evaluating
        // it once and caching the (meta, allowed) verdict avoids
        // N-1 redundant check_doc_access_from_meta_allow_deleted calls
        // per doc (each of which does up to 3 additional DDB reads for
        // membership/folder-grant/link-sharing lookups).
        //
        // Pre-fetched meta goes directly into the ACL helper so the
        // Phase-3 double-DDB-read fix (copy_document uses the same
        // helper) applies here too. Concurrency is capped at 16 —
        // MAX_GALLERY_DOC_IDS times N galleries could otherwise fire
        // hundreds of concurrent DDB round-trips.
        let acl_candidates = fetched
            .into_iter()
            .filter(|(_, meta)| meta.is_template && !meta.is_deleted);
        let acl_cache: std::collections::HashMap<String, DocumentMeta> =
            futures_util::stream::iter(acl_candidates)
                .map(|(doc_id, meta)| {
                    let state = state.clone();
                    let user_id = user_id.clone();
                    async move {
                        check_doc_access_from_meta_allow_deleted(
                            &state,
                            meta,
                            &user_id,
                            AccessLevel::View,
                        )
                        .await
                        .ok()
                        .map(|(meta, _)| (doc_id, meta))
                    }
                })
                .buffer_unordered(16)
                .filter_map(|opt| async move { opt })
                .collect()
                .await;

        for g in company_galleries {
            for doc_id in &g.doc_ids {
                let Some(meta) = acl_cache.get(doc_id) else {
                    continue;
                };
                let bucket = format!("company:{}", g.gallery_id);
                if !seen.insert((doc_id.clone(), bucket)) {
                    continue;
                }
                allowed.push((
                    meta.clone(),
                    Gallery::Company {
                        gallery_id: g.gallery_id.clone(),
                        gallery_name: g.name.clone(),
                    },
                ));
            }
        }
    }

    // Phase 2: scan each template's snapshot for mail-merge placeholders.
    // One S3 GET per template — done concurrently via `futures::join_all` so
    // the wall-clock is dominated by the slowest single fetch, not the sum.
    // On failure to read a snapshot, surface an empty placeholder list rather
    // than error the whole gallery (the caller still gets the template card
    // and can retry on click).
    let placeholder_futures = allowed.iter().map(|(m, gallery)| {
        let state = state.clone();
        let doc_id = m.doc_id.clone();
        let is_sample = matches!(gallery, Gallery::Sample);
        async move {
            // Samples are immutable between seeds — the placeholder set is
            // the same on every request. Serve from a process-wide cache
            // (computed once from the compile-time HTML fixtures) instead
            // of the S3 snapshot + pending-updates dance the fallback path
            // takes, which would do a wasteful GET per sample per open.
            if is_sample {
                if let Some(keys) = crate::seed::sample_placeholders(&doc_id) {
                    return keys.clone();
                }
            }
            // Same merge as get_content — the caller-visible state includes
            // pending UPDATE# rows on top of the last-persisted snapshot.
            match load_current_doc_state(&state, &doc_id).await {
                Ok(og) => ogrenotes_collab::mail_merge::scan_ydoc(og.inner()),
                Err(_) => Vec::new(),
            }
        }
    });
    let placeholders_per_row: Vec<Vec<String>> =
        futures_util::future::join_all(placeholder_futures).await;

    let items: Vec<TemplateItem> = allowed
        .into_iter()
        .zip(placeholders_per_row)
        .map(|((m, gallery), placeholders)| TemplateItem {
            id: m.doc_id,
            title: m.title,
            owner_id: m.owner_id,
            doc_type: m.doc_type.as_str().to_string(),
            updated_at: m.updated_at,
            placeholders,
            gallery,
        })
        .collect();

    Ok(Json(items))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopyDocumentRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    folder_id: Option<String>,
    /// Phase 2 — mail merge values. Object keyed by placeholder name; nested
    /// objects are supported (`[[user.name]]` looks up `values.user.name`).
    /// Absent or empty → the copy is a plain byte-passthrough. See
    /// `crates/collab/src/mail_merge.rs` for the substitution rules.
    #[serde(default)]
    values: Option<serde_json::Value>,
}

/// POST /documents/:id/copy — duplicate a document (#142).
///
/// The caller needs `View` on the source; any doc you can read you can fork into
/// your own copy (a read-and-fork model, not a permissions-changing
/// operation). The copy is
/// always owned by the caller and never inherits the `is_template` flag — a
/// copy of a template is a normal doc the user starts editing.
///
/// Default destination is the caller's Private folder, mirroring the
/// design-doc behavior ("Copy created in user's Private folder by default").
/// An explicit `folderId` is gated on `Edit` access via `resolve_dest_folder`.
async fn copy_document(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(src_id): Path<String>,
    Json(req): Json<CopyDocumentRequest>,
) -> Result<(StatusCode, Json<DocumentResponse>), ApiError> {
    // Sample templates live in a well-known workspace with no per-user ACL.
    // `list_templates` already surfaces them to every user; the copy path
    // has to match, otherwise the row appears in the picker but Create 403s
    // (the standard `check_doc_access` correctly rejects a non-member of the
    // samples workspace). Bypass the ACL only for docs that:
    // (a) live in SAMPLES_WORKSPACE_ID,
    // (b) are owned by the samples system user (otherwise anyone could
    //     plant a doc in the workspace and inherit the bypass — see the
    //     paired filter in `list_templates`),
    // (c) are actually marked `is_template`, and
    // (d) aren't deleted.
    // The meta is fetched once and reused by the non-sample branch below,
    // so a non-sample copy pays exactly one DDB GetItem for authorization
    // instead of two.
    let raw = state
        .doc_repo
        .get(&src_id)
        .await?
        .ok_or(ApiError::NotFound("Document not found".to_string()))?;
    let is_sample = raw.workspace_id.as_deref()
        == Some(crate::seed::SAMPLES_WORKSPACE_ID)
        && raw.owner_id == crate::seed::SAMPLES_SYSTEM_USER_ID
        && raw.is_template
        && !raw.is_deleted;
    let src_meta = if is_sample {
        raw
    } else {
        let (meta, is_trashed) =
            check_doc_access_from_meta_allow_deleted(&state, raw, &user_id, AccessLevel::View)
                .await?;
        // Trashed docs are not copyable (matches the id-based
        // check_doc_access wrapper which 404s on trash).
        if is_trashed {
            return Err(ApiError::NotFound("Document not found".to_string()));
        }
        meta
    };

    // Merge snapshot + pending UPDATE# rows into the caller-visible state
    // (same shape as get_content) so a copy captures everything typed since
    // the last put_content — not just the last-persisted snapshot. Without
    // the merge, copying a doc whose edits still live in UPDATE# rows would
    // produce a blank new doc even without mail merge.
    let mut og = load_current_doc_state(&state, &src_id).await?;

    // Apply mail-merge substitution before re-encoding, if requested.
    if let Some(values) = req.values.as_ref().filter(|v| !v.is_null()) {
        ogrenotes_collab::mail_merge::substitute_ydoc(og.inner_mut(), values);
    }
    let snapshot = og.to_state_bytes();

    // Resolve destination: explicit folder gets the standard Edit check;
    // absent defaults to the caller's Private folder (per design-doc behavior).
    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await?
        .ok_or_else(|| ApiError::Internal("user record missing after auth".to_string()))?;
    let folder_id = match req.folder_id.as_deref() {
        Some(id) => {
            super::folders::check_folder_access(&state, id, &user_id, AccessLevel::Edit).await?;
            id.to_string()
        }
        None => user.private_folder_id,
    };

    let new_doc_id = new_id();
    let now = now_usec();
    let title = req
        .title
        .unwrap_or_else(|| format!("Copy of {}", src_meta.title));

    let meta = DocumentMeta {
        doc_id: new_doc_id.clone(),
        title: title.clone(),
        owner_id: user_id.clone(),
        folder_id: Some(folder_id.clone()),
        // #149: copies land in the destination folder only — no multi-folder
        // membership carried forward from the source.
        additional_folder_ids: Vec::new(),
        // Inherit the source's workspace so the copy lives in the caller's
        // tenant (the caller is already a member; we re-derive from the user
        // row rather than carrying forward `src_meta.workspace_id`, which
        // could be a workspace the caller doesn't belong to).
        workspace_id: user.default_workspace_id.clone(),
        doc_type: src_meta.doc_type.clone(),
        snapshot_version: 1,
        snapshot_s3_key: Some(format!("docs/{new_doc_id}/snapshots/1.bin")),
        is_deleted: false,
        deleted_at: None,
        link_sharing_mode: None,
        link_view_options: ogrenotes_storage::models::ViewOptions::default(),
        locked: false,
        // #142: a copy of a template is itself a normal doc — the user edits
        // it, and a derived template would be marked explicitly afterward.
        is_template: false,
        created_at: now,
        updated_at: now,
    };

    state.doc_repo.create(&meta, &snapshot).await?;

    // Add to the destination folder (mirrors create_document's bookkeeping).
    let child = FolderChild {
        folder_id: folder_id.clone(),
        child_id: new_doc_id.clone(),
        child_type: ChildType::Doc,
        added_at: now,
    };
    state
        .folder_repo
        .add_child(&child)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    spawn_index_document_from_bytes(&state, meta.clone(), snapshot);

    Ok((
        StatusCode::CREATED,
        Json(DocumentResponse {
            id: new_doc_id,
            title,
            folder_id: Some(folder_id),
            doc_type: meta.doc_type.as_str().to_string(),
            created_at: now,
            updated_at: now,
            is_deleted: false,
            can_request_access: false,
            can_edit: true,
            is_favorite: false,
            locked: false,
            can_manage: true,
            is_template: false,
        }),
    ))
}

/// POST /documents/:id/request-access — a viewer asks the owner for edit
/// access. Offered only when the doc has a View-mode link with
/// `allow_request_access` on (so the owner opted into receiving
/// requests). Notifies the owner; grants nothing on its own.
async fn request_access(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Authorize first: the requester must at least be able to view the
    // doc. This yields the metadata and rejects trashed/missing (404) and
    // no-access (403) docs *before* the rate-limit bucket is touched, so
    // an unauthorized caller can't probe doc existence via bucket state.
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::View).await?;

    // The owner already has full access — nothing to request. Checked
    // before the rate-limit buckets so an owner can't burn their own
    // sharing quota self-requesting.
    if meta.owner_id == user_id {
        return Err(ApiError::BadRequest(
            "You already own this document".to_string(),
        ));
    }

    // Two-tier rate limit (§5.4), both after auth so only authorized,
    // non-owner requests count toward either budget.
    //
    // (1) The existing per-user sharing limiter — bounds a single account
    //     fanning requests out across many docs/owners (shares the bucket
    //     with other sharing mutations, as in routes::sharing).
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "sharing",
        &user_id,
        state.config.rate_limit_sharing_per_min,
        60,
    )
    .await?;
    // (2) A per-(doc, requester) cap — bounds repeatedly pinging one owner
    //     about one doc.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "sharing",
        &format!("reqaccess:{id}:{user_id}"),
        state.config.rate_limit_sharing_per_min,
        60,
    )
    .await?;

    // Only offered when the View-mode link explicitly enables requests.
    let offered = meta.link_sharing_mode
        == Some(ogrenotes_storage::models::LinkSharingMode::View)
        && meta.link_view_options.allow_request_access;
    if !offered {
        return Err(ApiError::Forbidden);
    }

    // Notify the owner (best-effort), mirroring the share-invite path.
    let notif_repo = state.notification_repo.clone();
    let email_service = state.email_service.clone();
    let owner = meta.owner_id.clone();
    let actor = user_id.clone();
    let doc_id = id.clone();
    tokio::spawn(async move {
        let notif = Notification {
            notif_id: nanoid::nanoid!(16),
            user_id: owner,
            notif_type: NotifType::RequestAccess,
            doc_id: Some(doc_id),
            thread_id: None,
            actor_id: actor,
            message: "requested edit access to your document".to_string(),
            preview: None,
            block_id: None,
            read: false,
            created_at: now_usec(),
        };
        let _ = notif_repo.create(&notif).await;
        // Direct request to the owner — always delivered.
        email_service.spawn_for_notification(notif, true);
    });

    Ok(StatusCode::NO_CONTENT)
}

/// PUT /documents/:id/favorite — star a document for the current user (#144).
/// Requires View access (you can favorite anything you can open); the
/// favorite is a per-user marker and never moves the doc.
async fn add_favorite(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    check_doc_access(&state, &id, &user_id, AccessLevel::View).await?;
    state
        .doc_repo
        .add_favorite(&Favorite {
            user_id,
            doc_id: id,
            added_at: now_usec(),
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /documents/:id/favorite — unstar (#144). No access check beyond
/// auth: a user may always remove their own favorite, even on a doc they've
/// since lost access to.
async fn remove_favorite(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .doc_repo
        .remove_favorite(&user_id, &id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FavoriteItem {
    id: String,
    title: String,
    doc_type: String,
    updated_at: i64,
}

/// GET /documents/favorites — the caller's starred docs (#144). Each favorite
/// is re-access-checked and trashed/inaccessible ones are skipped, so a stale
/// star (revoked access, deleted doc) silently drops out of the list rather
/// than erroring the whole response.
async fn list_favorites(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<Json<Vec<FavoriteItem>>, ApiError> {
    let ids = state
        .doc_repo
        .list_favorite_doc_ids(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(meta) = check_doc_access(&state, &id, &user_id, AccessLevel::View).await {
            items.push(FavoriteItem {
                id: meta.doc_id,
                title: meta.title,
                doc_type: meta.doc_type.as_str().to_string(),
                updated_at: meta.updated_at,
            });
        }
    }
    Ok(Json(items))
}

// ─── Collections (#144) — named per-user groups within Favorites ──

/// Max length of a collection name. Bounds the row size and the UI.
const MAX_COLLECTION_NAME: usize = 100;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionWithItems {
    id: String,
    name: String,
    items: Vec<FavoriteItem>,
}

/// GET /documents/collections — the caller's collections, each with its
/// accessible docs inlined (drives the sidebar). Inaccessible/trashed docs are
/// skipped per-item, mirroring `list_favorites`.
async fn list_collections(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<Json<Vec<CollectionWithItems>>, ApiError> {
    let colls = state
        .doc_repo
        .list_collections(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut out = Vec::with_capacity(colls.len());
    for c in colls {
        let ids = state
            .doc_repo
            .list_collection_doc_ids(&user_id, &c.collection_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        let mut items = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(meta) = check_doc_access(&state, &id, &user_id, AccessLevel::View).await {
                items.push(FavoriteItem {
                    id: meta.doc_id,
                    title: meta.title,
                    doc_type: meta.doc_type.as_str().to_string(),
                    updated_at: meta.updated_at,
                });
            }
        }
        out.push(CollectionWithItems { id: c.collection_id, name: c.name, items });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionMembership {
    id: String,
    name: String,
    /// Whether the doc in the path is currently in this collection.
    contains: bool,
}

/// GET /documents/:id/collections — every collection plus whether this doc is
/// in it (drives the star dropdown's checkmarks).
async fn list_doc_collections(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<CollectionMembership>>, ApiError> {
    check_doc_access(&state, &id, &user_id, AccessLevel::View).await?;
    let colls = state
        .doc_repo
        .list_collections(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let member_ids: std::collections::HashSet<String> = state
        .doc_repo
        .list_collection_ids_for_doc(&user_id, &id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .into_iter()
        .collect();
    Ok(Json(
        colls
            .into_iter()
            .map(|c| CollectionMembership {
                contains: member_ids.contains(&c.collection_id),
                id: c.collection_id,
                name: c.name,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCollectionRequest {
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateCollectionResponse {
    id: String,
    name: String,
}

/// POST /documents/:id/collections — create a new collection containing this
/// doc (the "New Collection…" menu item). Per-user; the doc must be viewable.
async fn create_doc_collection(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<CreateCollectionRequest>,
) -> Result<Json<CreateCollectionResponse>, ApiError> {
    check_doc_access(&state, &id, &user_id, AccessLevel::View).await?;
    let name = req.name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_COLLECTION_NAME {
        return Err(ApiError::BadRequest(format!(
            "Collection name must be 1..={MAX_COLLECTION_NAME} characters"
        )));
    }
    let collection_id = nanoid::nanoid!(16);
    let now = now_usec();
    state
        .doc_repo
        .create_collection(&ogrenotes_storage::models::document::Collection {
            user_id: user_id.clone(),
            collection_id: collection_id.clone(),
            name: name.clone(),
            created_at: now,
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    state
        .doc_repo
        .add_to_collection(&ogrenotes_storage::models::document::CollectionItem {
            user_id,
            collection_id: collection_id.clone(),
            doc_id: id,
            added_at: now,
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(CreateCollectionResponse { id: collection_id, name }))
}

/// PUT /documents/:id/collections/:cid — add this doc to an existing
/// collection. 404 if the collection isn't the caller's.
async fn add_doc_to_collection(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, cid)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    check_doc_access(&state, &id, &user_id, AccessLevel::View).await?;
    if !state
        .doc_repo
        .collection_exists(&user_id, &cid)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        return Err(ApiError::NotFound("Collection not found".to_string()));
    }
    state
        .doc_repo
        .add_to_collection(&ogrenotes_storage::models::document::CollectionItem {
            user_id,
            collection_id: cid,
            doc_id: id,
            added_at: now_usec(),
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /documents/:id/collections/:cid — remove this doc from a collection.
/// No access check beyond auth (a user may always curate their own grouping).
async fn remove_doc_from_collection(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, cid)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    state
        .doc_repo
        .remove_from_collection(&user_id, &cid, &id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── #149: multi-folder membership ──────────────────────────────────

/// One folder a document belongs to. `is_primary` marks the `folder_id`
/// pointer (breadcrumb/default home); the rest are additional memberships.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocFolderMembership {
    id: String,
    title: String,
    is_primary: bool,
}

/// GET /documents/:id/folders — every folder this doc is in, primary first.
/// Source of truth for the frontend's location chips + the add/remove picker.
async fn list_doc_folders(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<DocFolderMembership>>, ApiError> {
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::View).await?;
    let mut out = Vec::new();
    for folder_id in meta.folder_id.iter().chain(meta.additional_folder_ids.iter()) {
        // Best-effort title; silently skip a folder that no longer exists.
        if let Some(folder) = state.folder_repo.get(folder_id).await.ok().flatten() {
            out.push(DocFolderMembership {
                is_primary: meta.folder_id.as_deref() == Some(folder_id.as_str()),
                id: folder_id.clone(),
                title: folder.title,
            });
        }
    }
    Ok(Json(out))
}

/// PUT /documents/:id/folders/:folder_id — add this doc to an additional
/// folder (#149). Requires Edit on the doc (same authorization as Move; folder
/// visibility is enforced by the picker, which only surfaces the caller's
/// folders). No-op if it's already the primary; 404 if the target is missing.
async fn add_doc_to_folder(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, folder_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::Edit).await?;
    if meta.folder_id.as_deref() == Some(folder_id.as_str()) {
        return Ok(StatusCode::NO_CONTENT); // already the primary
    }
    // The caller must be a member of the destination folder (any level; the
    // owner is a member via folder_repo::create). Without this, a user with
    // only Edit on the DOC could add it to ANY folder they can name and grant
    // that folder's members access through the access union — a privilege
    // escalation invisible to the doc owner. Return NotFound (not Forbidden)
    // so the endpoint can't be used as a folder-id oracle; get_member also
    // returns None for a missing folder, subsuming the existence check.
    if state
        .folder_repo
        .get_member(&folder_id, &user_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return Err(ApiError::NotFound("Folder not found".to_string()));
    }
    let now = now_usec();
    state
        .doc_repo
        .add_doc_folder(&id, &folder_id, now)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    // Mirror the listing edge so the doc shows under the folder.
    state
        .folder_repo
        .add_child(&ogrenotes_storage::models::folder::FolderChild {
            folder_id: folder_id.clone(),
            child_id: id.clone(),
            child_type: ChildType::Doc,
            added_at: now,
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /documents/:id/folders/:folder_id — remove this doc from an
/// additional folder (#149). Refuses to remove the primary (use Move).
async fn remove_doc_from_folder(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path((id, folder_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::Edit).await?;
    if meta.folder_id.as_deref() == Some(folder_id.as_str()) {
        return Err(ApiError::BadRequest(
            "Cannot remove a document from its primary folder; move it instead.".to_string(),
        ));
    }
    let now = now_usec();
    state
        .doc_repo
        .remove_doc_folder(&id, &folder_id, now)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    // Propagate (unlike Move's best-effort remove): this is an explicit
    // "remove from folder", so a silent listing-edge failure would leave the
    // doc visible in the folder after its access was already revoked. Retry is
    // safe — both ops are idempotent.
    state
        .folder_repo
        .remove_child(&folder_id, &id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /documents/collections/:cid — delete a whole collection (and its
/// membership rows). Idempotent.
async fn delete_collection(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(cid): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .doc_repo
        .delete_collection(&user_id, &cid)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /documents/:id/import — import an XLSX or CSV file as document content.
async fn import_file(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(id): Path<String>,
    mut multipart: axum::extract::Multipart,
) -> Result<StatusCode, ApiError> {
    let meta = check_doc_access(&state, &id, &user_id, AccessLevel::Edit).await?;

    // #140: an import replaces the document's CRDT state wholesale — it is a
    // content write, so the doc-wide freeze must block it just like
    // `put_content`. Without this the lock would be trivially bypassable by
    // importing a file into a locked doc.
    if meta.locked {
        counter::inc(MetricKey::new("doc.locked_write_rejected_total", &[("path", "import")]));
        return Err(ApiError::ForbiddenMsg(
            "Document is locked for editing".to_string(),
        ));
    }

    // Read the first file field from the multipart body
    let field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Multipart error: {e}")))?
        .ok_or(ApiError::BadRequest("No file uploaded".to_string()))?;

    let filename = field.file_name().unwrap_or("unknown").to_string();
    let data = field
        .bytes()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Failed to read file: {e}")))?;

    if data.len() > MAX_CONTENT_SIZE {
        return Err(ApiError::BadRequest(format!(
            "File too large: {} bytes (max {})",
            data.len(),
            MAX_CONTENT_SIZE
        )));
    }

    // Detect format from filename extension
    let (doc, fmt) = if filename.ends_with(".xlsx") {
        let d = import_spreadsheet::from_xlsx(&data)
            .map_err(|e| ApiError::BadRequest(format!("Invalid XLSX file: {e}")))?;
        (d, "xlsx")
    } else if filename.ends_with(".csv") {
        let text = String::from_utf8(data.to_vec())
            .map_err(|_| ApiError::BadRequest("CSV file is not valid UTF-8".to_string()))?;
        (import_spreadsheet::from_csv(&text), "csv")
    } else {
        return Err(ApiError::BadRequest(
            "Unsupported file format. Use .xlsx or .csv".to_string(),
        ));
    };
    counter::inc(MetricKey::new(
        "doc.import_total",
        &[("format", fmt)],
    ));

    // Convert the imported doc to state bytes for storage
    let state_bytes = ogrenotes_collab::snapshot::doc_to_bytes(&doc);

    let now = now_usec();
    let new_version = meta.snapshot_version + 1;
    state
        .doc_repo
        .save_snapshot(&id, &state_bytes, new_version, now, &user_id)
        .await?;

    // Delete pending updates to prevent stale CRDT ops from corrupting the import
    let _ = state.doc_repo.delete_updates_before(&id, now).await;

    // Index imported content
    let mut updated_meta = meta;
    updated_meta.updated_at = now;
    spawn_index_document_from_bytes(&state, updated_meta, state_bytes);

    Ok(StatusCode::NO_CONTENT)
}

fn is_allowed_content_type(ct: &str) -> bool {
    let ct_lower = ct.to_lowercase();
    ct_lower.starts_with("image/")
        || ct_lower.starts_with("application/pdf")
        || ct_lower == "text/plain"
        || ct_lower == "text/csv"
        || ct_lower == "text/markdown"
        || ct_lower == "text/tab-separated-values"
        || ct_lower == "application/octet-stream"
}

// ─── Search + embedding index helpers ──────────────────────────

/// Extract plain text from Y.Doc body bytes.
fn extract_plain_text(body: &[u8]) -> Option<String> {
    let doc = OgreDoc::from_state_bytes(body).ok()?;
    Some(export::to_plain_text(doc.inner()))
}

/// Build a SearchDocument from metadata and pre-extracted plain text.
fn build_search_doc(meta: &DocumentMeta, plain_text: &str) -> SearchDocument {
    SearchDocument {
        doc_id: meta.doc_id.clone(),
        title: meta.title.clone(),
        body: plain_text.to_string(),
        owner_id: meta.owner_id.clone(),
        doc_type: meta.doc_type.as_str().to_string(),
        folder_id: meta.folder_id.clone(),
        workspace_id: meta.workspace_id.clone(),
        updated_at: meta.updated_at,
        created_at: meta.created_at,
    }
}

/// Fire-and-forget: embed and store vectors for a document.
/// No-op if the embedding pipeline is not configured.
fn spawn_embed_document(state: &AppState, meta: DocumentMeta, plain_text: String) {
    let Some(pipeline) = state.embedding_pipeline.clone() else {
        return;
    };
    let doc_id = meta.doc_id.clone();
    tokio::spawn(async move {
        let metadata = ogrenotes_embeddings::PointMetadata {
            doc_id: meta.doc_id.clone(),
            owner_id: meta.owner_id.clone(),
            doc_type: meta.doc_type.as_str().to_string(),
            folder_id: meta.folder_id.clone(),
            workspace_id: meta.workspace_id.clone(),
            title: meta.title.clone(),
            updated_at: meta.updated_at,
        };
        if let Err(e) = pipeline
            .index_document(&meta.doc_id, &meta.title, &plain_text, metadata)
            .await
        {
            tracing::error!(doc_id = %doc_id, error = %e, "failed to embed document");
        }
    });
}

/// Fire-and-forget: remove document vectors from Qdrant.
fn spawn_delete_embeddings(state: &AppState, doc_id: String) {
    let Some(pipeline) = state.embedding_pipeline.clone() else {
        return;
    };
    let did = doc_id.clone();
    tokio::spawn(async move {
        if let Err(e) = pipeline.delete_document(&doc_id).await {
            tracing::error!(doc_id = %did, error = %e, "failed to delete embeddings");
        }
    });
}

/// Fire-and-forget: index a document in the search index.
/// Loads the latest snapshot + pending updates, extracts text, and indexes
/// in both Tantivy (BM25) and the embedding pipeline (vectors).
pub(crate) fn spawn_index_document(state: &AppState, meta: DocumentMeta) {
    let doc_repo = state.doc_repo.clone();
    let search_index = state.search_index.clone();
    let state_for_embed = state.clone();
    let meta_for_embed = meta.clone();
    let max_pending_bytes = state.config.max_pending_updates_bytes;
    tokio::spawn(async move {
        let snapshot = match doc_repo.load_snapshot(&meta.doc_id).await {
            Ok(Some(s)) => s,
            _ => return,
        };

        let mut ogre_doc = match OgreDoc::from_state_bytes(&snapshot) {
            Ok(d) => d,
            Err(_) => return,
        };

        // #91: search reindex is best-effort. If the pending tail
        // exceeds the cap, log and skip — letting one giant doc
        // poison the search index is worse than indexing slightly-
        // stale snapshot text. Compaction will eventually catch up.
        // The explicit `match` (rather than `if let Ok`) is the
        // emit-log channel: an over-budget doc is the operator
        // signal that something needs compacting before the next
        // GET /content 503s on it.
        match doc_repo
            .get_pending_updates(&meta.doc_id, max_pending_bytes)
            .await
        {
            Ok(updates) => {
                for u in &updates {
                    let _ = ogre_doc.apply_update(&u.update_bytes);
                }
            }
            Err(ogrenotes_storage::repo::RepoError::TooLarge { what, actual, cap }) => {
                tracing::warn!(
                    doc_id = %meta.doc_id,
                    actual,
                    cap,
                    "spawn_index_document: pending tail too large ({what}) — \
                     indexing snapshot only; doc needs compaction"
                );
            }
            Err(e) => {
                tracing::warn!(
                    doc_id = %meta.doc_id,
                    error = %e,
                    "spawn_index_document: failed to load pending updates; \
                     indexing snapshot only"
                );
            }
        }

        let plain_text = export::to_plain_text(ogre_doc.inner());
        let search_doc = build_search_doc(&meta, &plain_text);
        if let Err(e) = search_index.index_document(&search_doc) {
            tracing::error!(doc_id = %meta.doc_id, error = %e, "failed to index document");
        }

        spawn_embed_document(&state_for_embed, meta_for_embed, plain_text);
    });
}

/// Fire-and-forget: index a document from raw state bytes (no snapshot load needed).
/// Indexes in both Tantivy and the embedding pipeline.
pub(crate) fn spawn_index_document_from_bytes(state: &AppState, meta: DocumentMeta, body: Vec<u8>) {
    let search_index = state.search_index.clone();
    let state_for_embed = state.clone();
    let meta_for_embed = meta.clone();
    tokio::spawn(async move {
        match extract_plain_text(&body) {
            Some(plain_text) => {
                let search_doc = build_search_doc(&meta, &plain_text);
                if let Err(e) = search_index.index_document(&search_doc) {
                    tracing::error!(doc_id = %meta.doc_id, error = %e, "failed to index document");
                }
                spawn_embed_document(&state_for_embed, meta_for_embed, plain_text);
            }
            None => {
                tracing::warn!(doc_id = %meta.doc_id, "skipped indexing: failed to extract text from document");
            }
        }
    });
}

/// Fire-and-forget: remove a document from both the search index and vector store.
pub(crate) fn spawn_delete_from_index(state: &AppState, doc_id: String) {
    let search_index = state.search_index.clone();
    let did = doc_id.clone();
    tokio::spawn(async move {
        if let Err(e) = search_index.delete_document(&doc_id) {
            tracing::error!(doc_id = %doc_id, error = %e, "failed to remove document from search index");
        }
    });
    spawn_delete_embeddings(state, did);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The router builds without a matchit conflict panic. Guards the static
    /// `/collections*` routes against the param `/{id}*` routes (#144) — a
    /// conflict would panic here at `.route()` time, before any request.
    #[test]
    fn router_builds_without_route_conflicts() {
        let _ = router();
    }

    use ogrenotes_storage::models::folder::FolderMember;
    use ogrenotes_storage::models::workspace::WorkspaceMember;
    use ogrenotes_storage::models::{
        document::DocMember, AccessLevel, DocType, InheritMode, LinkSharingMode, WorkspaceRole,
    };

    // ─── evaluate_doc_access ─────────────────────────────────────
    //
    // Pure-function tests: no DDB / S3, no AppState. Each test
    // constructs the minimal DocumentMeta + (optional) membership rows
    // and asserts the AccessDecision matches the documented branch.

    fn meta(owner: &str, deleted: bool) -> DocumentMeta {
        DocumentMeta {
            doc_id: "doc-1".to_string(),
            title: "T".to_string(),
            owner_id: owner.to_string(),
            folder_id: None,
            additional_folder_ids: Vec::new(),
            workspace_id: None,
            doc_type: DocType::Document,
            snapshot_version: 1,
            snapshot_s3_key: None,
            is_deleted: deleted,
            deleted_at: None,
            link_sharing_mode: None,
            link_view_options: ogrenotes_storage::models::ViewOptions::default(),
            locked: false,
            is_template: false,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn meta_with_link(
        owner: &str,
        workspace: Option<&str>,
        mode: Option<LinkSharingMode>,
    ) -> DocumentMeta {
        DocumentMeta {
            workspace_id: workspace.map(String::from),
            link_sharing_mode: mode,
            ..meta(owner, false)
        }
    }

    fn doc_member(user: &str, level: AccessLevel) -> DocMember {
        DocMember {
            doc_id: "doc-1".to_string(),
            user_id: user.to_string(),
            access_level: level,
            added_at: 0,
        }
    }

    fn folder_member(user: &str, level: AccessLevel) -> FolderMember {
        FolderMember {
            folder_id: "folder-1".to_string(),
            user_id: user.to_string(),
            access_level: level,
            added_at: 0,
        }
    }

    fn workspace_member(user: &str) -> WorkspaceMember {
        WorkspaceMember {
            workspace_id: "ws-1".to_string(),
            user_id: user.to_string(),
            role: WorkspaceRole::Member,
            joined_at: 0,
        }
    }

    #[test]
    fn evaluate_returns_trashed_for_owner_on_deleted_doc() {
        let m = meta("alice", true);
        let decision = evaluate_doc_access(
            &m, "alice", AccessLevel::View, None, &[], None,
        );
        assert_eq!(decision, AccessDecision::Trashed);
    }

    #[test]
    fn evaluate_returns_not_found_for_non_owner_on_deleted_doc() {
        // Trash + non-owner → 404, not 403. Deletion must not be a
        // side-channel for existence probes.
        let m = meta("alice", true);
        let direct = doc_member("bob", AccessLevel::Edit);
        let decision = evaluate_doc_access(
            &m,
            "bob",
            AccessLevel::View,
            Some(&direct),
            &[],
            None,
        );
        assert_eq!(decision, AccessDecision::NotFound);
    }

    #[test]
    fn evaluate_owner_short_circuits_to_allowed() {
        let m = meta("alice", false);
        let decision = evaluate_doc_access(
            &m, "alice", AccessLevel::Own, None, &[], None,
        );
        assert_eq!(decision, AccessDecision::Allowed);
    }

    #[test]
    fn evaluate_doc_member_at_required_level_allows() {
        let m = meta("alice", false);
        let direct = doc_member("bob", AccessLevel::Edit);
        let decision = evaluate_doc_access(
            &m,
            "bob",
            AccessLevel::Comment, // Edit ≥ Comment
            Some(&direct),
            &[],
            None,
        );
        assert_eq!(decision, AccessDecision::Allowed);
    }

    #[test]
    fn evaluate_doc_member_below_required_level_falls_through_to_forbidden() {
        let m = meta("alice", false);
        let direct = doc_member("bob", AccessLevel::View);
        let decision = evaluate_doc_access(
            &m,
            "bob",
            AccessLevel::Edit, // View < Edit
            Some(&direct),
            &[],
            None,
        );
        assert_eq!(decision, AccessDecision::Forbidden);
    }

    #[test]
    fn evaluate_folder_member_when_inherit_is_open_allows() {
        let m = DocumentMeta {
            folder_id: Some("folder-1".to_string()),
            ..meta("alice", false)
        };
        let grants = [FolderGrant {
            inherit_mode: InheritMode::Inherit,
            member: Some(folder_member("bob", AccessLevel::Edit)),
        }];
        let decision = evaluate_doc_access(&m, "bob", AccessLevel::View, None, &grants, None);
        assert_eq!(decision, AccessDecision::Allowed);
    }

    #[test]
    fn evaluate_folder_member_blocked_when_restricted() {
        // Restricted folders ignore folder-level membership entirely —
        // only direct doc membership applies on a restricted folder.
        let m = DocumentMeta {
            folder_id: Some("folder-1".to_string()),
            ..meta("alice", false)
        };
        let grants = [FolderGrant {
            inherit_mode: InheritMode::Restricted,
            member: Some(folder_member("bob", AccessLevel::Own)),
        }];
        let decision = evaluate_doc_access(&m, "bob", AccessLevel::View, None, &grants, None);
        assert_eq!(decision, AccessDecision::Forbidden);
    }

    // ─── #149: multi-folder union (most-permissive) ───────────────────

    #[test]
    fn evaluate_unions_across_folders_allows_via_any_open_folder() {
        // Doc in two open folders; the caller is a member of only the second.
        let m = meta("alice", false);
        let grants = [
            FolderGrant { inherit_mode: InheritMode::Inherit, member: None },
            FolderGrant {
                inherit_mode: InheritMode::Inherit,
                member: Some(folder_member("bob", AccessLevel::View)),
            },
        ];
        let decision = evaluate_doc_access(&m, "bob", AccessLevel::View, None, &grants, None);
        assert_eq!(decision, AccessDecision::Allowed);
    }

    #[test]
    fn evaluate_restricted_plus_inherit_unions_to_allowed() {
        // Doc in a Restricted folder AND an Inherit folder where the caller is
        // a member → allowed. Most-permissive: the Restricted folder simply
        // contributes no grant; it can't veto the open folder.
        let m = meta("alice", false);
        let grants = [
            FolderGrant {
                inherit_mode: InheritMode::Restricted,
                member: Some(folder_member("bob", AccessLevel::Own)),
            },
            FolderGrant {
                inherit_mode: InheritMode::Inherit,
                member: Some(folder_member("bob", AccessLevel::View)),
            },
        ];
        let decision = evaluate_doc_access(&m, "bob", AccessLevel::View, None, &grants, None);
        assert_eq!(decision, AccessDecision::Allowed);
    }

    #[test]
    fn evaluate_restricted_only_folders_never_grant() {
        // Member of several Restricted folders, no direct grant → forbidden.
        let m = meta("alice", false);
        let grants = [
            FolderGrant {
                inherit_mode: InheritMode::Restricted,
                member: Some(folder_member("bob", AccessLevel::Own)),
            },
            FolderGrant {
                inherit_mode: InheritMode::Restricted,
                member: Some(folder_member("bob", AccessLevel::Edit)),
            },
        ];
        let decision = evaluate_doc_access(&m, "bob", AccessLevel::View, None, &grants, None);
        assert_eq!(decision, AccessDecision::Forbidden);
    }

    #[test]
    fn evaluate_link_sharing_grants_access_to_workspace_member() {
        let m = meta_with_link("alice", Some("ws-1"), Some(LinkSharingMode::View));
        let wm = workspace_member("bob");
        let decision = evaluate_doc_access(
            &m,
            "bob",
            AccessLevel::View,
            None,
            &[],
            Some(&wm),
        );
        assert_eq!(decision, AccessDecision::Allowed);
    }

    #[test]
    fn evaluate_link_sharing_view_does_not_satisfy_edit_request() {
        // Mode=View, required=Edit. The link-sharing branch grants
        // exactly what the mode says — it must not promote View to Edit.
        let m = meta_with_link("alice", Some("ws-1"), Some(LinkSharingMode::View));
        let wm = workspace_member("bob");
        let decision = evaluate_doc_access(
            &m,
            "bob",
            AccessLevel::Edit,
            None,
            &[],
            Some(&wm),
        );
        assert_eq!(decision, AccessDecision::Forbidden);
    }

    #[test]
    fn evaluate_link_sharing_denies_non_workspace_user() {
        // No workspace_member row → caller is not in the doc's
        // workspace; link sharing branch is skipped, fall through
        // to default deny.
        let m = meta_with_link("alice", Some("ws-1"), Some(LinkSharingMode::Edit));
        let decision = evaluate_doc_access(
            &m,
            "carol",
            AccessLevel::View,
            None,
            &[],
            None, // not a workspace member
        );
        assert_eq!(decision, AccessDecision::Forbidden);
    }

    #[test]
    fn evaluate_default_deny_when_no_membership_anywhere() {
        let m = meta("alice", false);
        let decision = evaluate_doc_access(
            &m, "bob", AccessLevel::View, None, &[], None,
        );
        assert_eq!(decision, AccessDecision::Forbidden);
    }

    // ─── access_level_satisfies ─────────────────────────────────

    #[test]
    fn test_access_own_satisfies_all() {
        assert!(access_level_satisfies(&AccessLevel::Own, &AccessLevel::Own));
        assert!(access_level_satisfies(&AccessLevel::Own, &AccessLevel::Edit));
        assert!(access_level_satisfies(&AccessLevel::Own, &AccessLevel::Comment));
        assert!(access_level_satisfies(&AccessLevel::Own, &AccessLevel::View));
    }

    #[test]
    fn test_access_edit_satisfies_edit_and_below() {
        assert!(!access_level_satisfies(&AccessLevel::Edit, &AccessLevel::Own));
        assert!(access_level_satisfies(&AccessLevel::Edit, &AccessLevel::Edit));
        assert!(access_level_satisfies(&AccessLevel::Edit, &AccessLevel::Comment));
        assert!(access_level_satisfies(&AccessLevel::Edit, &AccessLevel::View));
    }

    #[test]
    fn test_access_comment_satisfies_comment_and_view() {
        assert!(!access_level_satisfies(&AccessLevel::Comment, &AccessLevel::Own));
        assert!(!access_level_satisfies(&AccessLevel::Comment, &AccessLevel::Edit));
        assert!(access_level_satisfies(&AccessLevel::Comment, &AccessLevel::Comment));
        assert!(access_level_satisfies(&AccessLevel::Comment, &AccessLevel::View));
    }

    #[test]
    fn test_access_view_satisfies_only_view() {
        assert!(!access_level_satisfies(&AccessLevel::View, &AccessLevel::Own));
        assert!(!access_level_satisfies(&AccessLevel::View, &AccessLevel::Edit));
        assert!(!access_level_satisfies(&AccessLevel::View, &AccessLevel::Comment));
        assert!(access_level_satisfies(&AccessLevel::View, &AccessLevel::View));
    }

    #[test]
    fn test_access_same_level_satisfies() {
        for level in &[AccessLevel::Own, AccessLevel::Edit, AccessLevel::Comment, AccessLevel::View] {
            assert!(access_level_satisfies(level, level), "{level:?} should satisfy itself");
        }
    }

    // ─── is_allowed_content_type ────────────────────────────────

    #[test]
    fn test_allowed_image_types() {
        assert!(is_allowed_content_type("image/png"));
        assert!(is_allowed_content_type("image/jpeg"));
        assert!(is_allowed_content_type("image/gif"));
        assert!(is_allowed_content_type("image/webp"));
    }

    #[test]
    fn test_allowed_pdf() {
        assert!(is_allowed_content_type("application/pdf"));
    }

    #[test]
    fn test_allowed_text_types() {
        assert!(is_allowed_content_type("text/plain"));
        assert!(is_allowed_content_type("text/csv"));
        assert!(is_allowed_content_type("text/markdown"));
        assert!(is_allowed_content_type("text/tab-separated-values"));
    }

    #[test]
    fn test_blocked_text_types() {
        // text/html and text/javascript are XSS vectors via presigned S3 URLs
        assert!(!is_allowed_content_type("text/html"));
        assert!(!is_allowed_content_type("text/javascript"));
    }

    #[test]
    fn test_allowed_octet_stream() {
        assert!(is_allowed_content_type("application/octet-stream"));
    }

    #[test]
    fn test_disallowed_types() {
        assert!(!is_allowed_content_type("application/javascript"));
        assert!(!is_allowed_content_type("application/zip"));
        assert!(!is_allowed_content_type("video/mp4"));
    }

    #[test]
    fn test_content_type_case_insensitive() {
        assert!(is_allowed_content_type("IMAGE/PNG"));
        assert!(is_allowed_content_type("Application/PDF"));
        assert!(is_allowed_content_type("Text/Plain"));
    }
}

// ─── Property test: full evaluate_doc_access matrix ─────────────
//
// Enumerates every meaningful (membership-kind × granted-level ×
// required-level × is-owner × is-trashed) combination and asserts
// evaluate_doc_access matches a hand-derived expected() function. The
// expected() function is the executable spec of the access matrix; if
// the implementation drifts from it the proptest catches it immediately.
//
// Cardinality: ~5 (kinds) × 4 (granted) × 4 (required) × 2 (owner) × 2
// (trashed) = 320 deterministic combinations. Each call is microseconds
// of pure function work, so a full sweep is <1s on CI.
#[cfg(test)]
mod proptests {
    use super::*;
    use ogrenotes_storage::models::folder::FolderMember;
    use ogrenotes_storage::models::workspace::WorkspaceMember;
    use ogrenotes_storage::models::{
        document::DocMember, AccessLevel, DocType, InheritMode, LinkSharingMode, WorkspaceRole,
    };
    use proptest::prelude::*;
    use proptest::sample::select;

    /// Source of the caller's access, if any. Mirrors the four branches
    /// in `evaluate_doc_access` plus a "no membership at all" case.
    #[derive(Debug, Clone, Copy)]
    enum Membership {
        None,
        DocMember,
        FolderMemberOpen,
        FolderMemberRestricted,
        LinkSharingWorkspaceMember,
    }

    fn membership_strategy() -> impl Strategy<Value = Membership> {
        select(vec![
            Membership::None,
            Membership::DocMember,
            Membership::FolderMemberOpen,
            Membership::FolderMemberRestricted,
            Membership::LinkSharingWorkspaceMember,
        ])
    }

    fn level_strategy() -> impl Strategy<Value = AccessLevel> {
        select(vec![
            AccessLevel::View,
            AccessLevel::Comment,
            AccessLevel::Edit,
            AccessLevel::Own,
        ])
    }

    /// Build the inputs to `evaluate_doc_access` for a given combination,
    /// then call expected() to derive the decision the spec demands.
    /// The proptest body asserts both decisions agree.
    fn case(
        kind: Membership,
        granted: AccessLevel,
        required: AccessLevel,
        is_owner: bool,
        is_trashed: bool,
    ) -> (
        DocumentMeta,
        Option<DocMember>,
        Option<InheritMode>,
        Option<FolderMember>,
        Option<WorkspaceMember>,
        AccessDecision,
    ) {
        let owner_id = "alice";
        let user_id = if is_owner { "alice" } else { "bob" };
        let mut meta = DocumentMeta {
            doc_id: "doc-1".to_string(),
            title: "T".to_string(),
            owner_id: owner_id.to_string(),
            folder_id: None,
            additional_folder_ids: Vec::new(),
            workspace_id: None,
            doc_type: DocType::Document,
            snapshot_version: 1,
            snapshot_s3_key: None,
            is_deleted: is_trashed,
            deleted_at: None,
            link_sharing_mode: None,
            link_view_options: ogrenotes_storage::models::ViewOptions::default(),
            locked: false,
            is_template: false,
            created_at: 0,
            updated_at: 0,
        };

        let mut direct: Option<DocMember> = None;
        let mut inherit: Option<InheritMode> = None;
        let mut fm: Option<FolderMember> = None;
        let mut wm: Option<WorkspaceMember> = None;

        match kind {
            Membership::None => {}
            Membership::DocMember => {
                direct = Some(DocMember {
                    doc_id: "doc-1".to_string(),
                    user_id: user_id.to_string(),
                    access_level: granted.clone(),
                    added_at: 0,
                });
            }
            Membership::FolderMemberOpen => {
                meta.folder_id = Some("folder-1".to_string());
                inherit = Some(InheritMode::Inherit);
                fm = Some(FolderMember {
                    folder_id: "folder-1".to_string(),
                    user_id: user_id.to_string(),
                    access_level: granted.clone(),
                    added_at: 0,
                });
            }
            Membership::FolderMemberRestricted => {
                meta.folder_id = Some("folder-1".to_string());
                inherit = Some(InheritMode::Restricted);
                fm = Some(FolderMember {
                    folder_id: "folder-1".to_string(),
                    user_id: user_id.to_string(),
                    access_level: granted.clone(),
                    added_at: 0,
                });
            }
            Membership::LinkSharingWorkspaceMember => {
                meta.workspace_id = Some("ws-1".to_string());
                meta.link_sharing_mode = Some(match granted {
                    AccessLevel::Edit | AccessLevel::Own => LinkSharingMode::Edit,
                    _ => LinkSharingMode::View,
                });
                wm = Some(WorkspaceMember {
                    workspace_id: "ws-1".to_string(),
                    user_id: user_id.to_string(),
                    role: WorkspaceRole::Member,
                    joined_at: 0,
                });
            }
        }

        let expected =
            expected_decision(required, kind, granted, is_owner, is_trashed);
        (meta, direct, inherit, fm, wm, expected)
    }

    /// The executable spec. Encodes:
    ///   trash + owner → Trashed
    ///   trash + non-owner → NotFound
    ///   live + owner → Allowed
    ///   live + non-owner: walk DocMember → FolderMember (when open) →
    ///     LinkSharing (workspace-member only); each branch grants only
    ///     when the granted level satisfies the required level.
    ///   Restricted folder skips the FolderMember branch entirely.
    ///   LinkSharing's effective level is View when mode=View, Edit
    ///     when mode=Edit; it must NOT be promoted above the mode.
    fn expected_decision(
        required: AccessLevel,
        kind: Membership,
        granted: AccessLevel,
        is_owner: bool,
        is_trashed: bool,
    ) -> AccessDecision {
        if is_trashed {
            return if is_owner {
                AccessDecision::Trashed
            } else {
                AccessDecision::NotFound
            };
        }
        if is_owner {
            return AccessDecision::Allowed;
        }
        match kind {
            Membership::None => AccessDecision::Forbidden,
            Membership::DocMember => {
                if satisfies_level(&granted, &required) {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Forbidden
                }
            }
            Membership::FolderMemberOpen => {
                if satisfies_level(&granted, &required) {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Forbidden
                }
            }
            Membership::FolderMemberRestricted => AccessDecision::Forbidden,
            Membership::LinkSharingWorkspaceMember => {
                // Effective level = the mode (View or Edit) — link
                // sharing must NEVER promote View to Edit/Own.
                let effective = match granted {
                    AccessLevel::Edit | AccessLevel::Own => AccessLevel::Edit,
                    _ => AccessLevel::View,
                };
                if satisfies_level(&effective, &required) {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Forbidden
                }
            }
        }
    }

    fn rank(level: &AccessLevel) -> u8 {
        match level {
            AccessLevel::Own => 4,
            AccessLevel::Edit => 3,
            AccessLevel::Comment => 2,
            AccessLevel::View => 1,
        }
    }
    fn satisfies_level(grant: &AccessLevel, need: &AccessLevel) -> bool {
        rank(grant) >= rank(need)
    }

    proptest! {
        // 320 cases is well within proptest's default sample budget;
        // bump explicitly so the matrix is fully covered every run.
        #![proptest_config(ProptestConfig {
            cases: 1024,
            ..ProptestConfig::default()
        })]

        #[test]
        fn evaluate_doc_access_matches_documented_matrix(
            kind in membership_strategy(),
            granted in level_strategy(),
            required in level_strategy(),
            is_owner in any::<bool>(),
            is_trashed in any::<bool>(),
        ) {
            let (meta, direct, inherit, fm, wm, expected) =
                case(kind, granted.clone(), required.clone(), is_owner, is_trashed);
            let user_id = if is_owner { "alice" } else { "bob" };
            let label = format!(
                "kind={:?} granted={:?} required={:?} is_owner={} is_trashed={}",
                kind, granted, required, is_owner, is_trashed,
            );
            // #149: the single (inherit_mode, member) the matrix models maps
            // to a one-folder grant set (None inherit = doc has no folder).
            let grants: Vec<FolderGrant> = match inherit {
                Some(mode) => vec![FolderGrant { inherit_mode: mode, member: fm }],
                None => vec![],
            };
            let actual = evaluate_doc_access(
                &meta,
                user_id,
                required,
                direct.as_ref(),
                &grants,
                wm.as_ref(),
            );
            prop_assert_eq!(actual, expected, "mismatch for {}", label);
        }
    }

    // ── Multi-membership precedence ─────────────────────────────
    //
    // The single-branch matrix above varies one membership at a time.
    // This narrow proptest crosses *direct* and *folder* memberships
    // (the two paths that can both fire for the same caller) and pins
    // the load-bearing precedence rule:
    //
    //   "A direct DocMember grant always wins, even when the doc's
    //    folder is Restricted."
    //
    // The Restricted folder rule is: only direct doc/folder grants
    // pass — folder-membership-via-inheritance is suppressed. A bug
    // where Restricted *also* suppresses direct grants would silently
    // revoke access for every user with a direct grant on a doc inside
    // a Restricted folder. That regression would not be caught by the
    // single-membership matrix.
    //
    // Cardinality: 5 (direct ∈ None ∪ AccessLevel) × 5 (folder same)
    // × 2 (restricted) × 4 (required) = 200 deterministic cells.

    fn level_or_none_strategy() -> impl Strategy<Value = Option<AccessLevel>> {
        select(vec![
            None,
            Some(AccessLevel::View),
            Some(AccessLevel::Comment),
            Some(AccessLevel::Edit),
            Some(AccessLevel::Own),
        ])
    }

    /// Documented precedence for the direct × folder cross product.
    /// Direct grant wins if it satisfies; otherwise the folder grant
    /// wins iff the folder is open. Restricted folders skip the folder
    /// grant entirely. This is the executable spec — the property test
    /// asserts evaluate_doc_access matches.
    fn expected_direct_x_folder(
        direct: Option<AccessLevel>,
        folder: Option<AccessLevel>,
        restricted: bool,
        required: AccessLevel,
    ) -> AccessDecision {
        let satisfies = |grant: &AccessLevel, need: &AccessLevel| -> bool {
            fn rank(level: &AccessLevel) -> u8 {
                match level {
                    AccessLevel::Own => 4,
                    AccessLevel::Edit => 3,
                    AccessLevel::Comment => 2,
                    AccessLevel::View => 1,
                }
            }
            rank(grant) >= rank(need)
        };
        if let Some(d) = direct.as_ref() {
            if satisfies(d, &required) {
                return AccessDecision::Allowed;
            }
        }
        if !restricted {
            if let Some(f) = folder.as_ref() {
                if satisfies(f, &required) {
                    return AccessDecision::Allowed;
                }
            }
        }
        AccessDecision::Forbidden
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 1024,
            ..ProptestConfig::default()
        })]

        #[test]
        fn direct_member_overrides_folder_restriction(
            direct in level_or_none_strategy(),
            folder in level_or_none_strategy(),
            restricted in any::<bool>(),
            required in level_strategy(),
        ) {
            let mut meta = DocumentMeta {
                doc_id: "doc-1".to_string(),
                title: "T".to_string(),
                owner_id: "alice".to_string(),
                folder_id: Some("folder-1".to_string()),
                additional_folder_ids: Vec::new(),
                workspace_id: None,
                doc_type: DocType::Document,
                snapshot_version: 1,
                snapshot_s3_key: None,
                is_deleted: false,
                deleted_at: None,
                link_sharing_mode: None,
                link_view_options: ogrenotes_storage::models::ViewOptions::default(),
                locked: false,
                is_template: false,
                created_at: 0,
                updated_at: 0,
            };
            // The caller is *not* the owner — owner short-circuits past
            // the membership branches. We're testing precedence below
            // the owner shortcut.
            let user_id = "bob";

            // Build the optional membership rows.
            let direct_member = direct.as_ref().map(|level| DocMember {
                doc_id: meta.doc_id.clone(),
                user_id: user_id.to_string(),
                access_level: level.clone(),
                added_at: 0,
            });
            let folder_member = folder.as_ref().map(|level| FolderMember {
                folder_id: "folder-1".to_string(),
                user_id: user_id.to_string(),
                access_level: level.clone(),
                added_at: 0,
            });
            let inherit = if restricted {
                InheritMode::Restricted
            } else {
                InheritMode::Inherit
            };

            // Sanity: meta is non-trashed and folder_id is set so the
            // wrapper would have actually fetched the folder + member.
            meta.workspace_id = None;

            let expected = expected_direct_x_folder(
                direct.clone(),
                folder.clone(),
                restricted,
                required.clone(),
            );
            let label = format!(
                "direct={:?} folder={:?} restricted={} required={:?}",
                direct, folder, restricted, required,
            );
            // #149: this proptest always sets folder_id → exactly one folder
            // grant.
            let grants = [FolderGrant {
                inherit_mode: inherit,
                member: folder_member,
            }];
            let actual = evaluate_doc_access(
                &meta,
                user_id,
                required,
                direct_member.as_ref(),
                &grants,
                None,
            );
            prop_assert_eq!(actual, expected, "precedence mismatch for {}", label);
        }
    }
}
