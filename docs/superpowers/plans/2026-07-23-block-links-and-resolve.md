# Block Links + Mention-Resolve Endpoint (Mentions Plan 1 of 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Stages 1–2 of the Document & Anchor Mentions spec (`docs/superpowers/specs/2026-07-23-document-mentions-design.md`): shareable `#b=<blockId>` block links (producer + consumer) and the batch `POST /api/v1/mentions/resolve` endpoint with access-gated title/snippet resolution.

**Architecture:** Frontend adds a "Copy Link to Block" context-menu command (clipboard-writes `{origin}{path}#b=<blockId>`) and a document-page effect that consumes the fragment on load (scroll + transient highlight, or a "linked section no longer exists" toast). Backend adds a `block_plain_text` extractor to `crates/collab` and a new `routes/mentions.rs` module whose per-target resolution reuses `check_doc_access` and collapses Forbidden/NotFound into one indistinguishable per-target `notFound`. Plan 2 (element + paste conversion, Stages 3–4) is written after this lands.

**Tech Stack:** Backend Axum + yrs (`crates/api`, `crates/collab`); frontend Leptos 0.7 CSR/WASM (`frontend/`, outside the workspace); Fluent i18n.

## Global Constraints

- **Frontend is outside the cargo workspace** — `cd frontend/` before building/testing it. Editor code is wasm-gated: verify with BOTH `cargo check` AND `cargo build --target wasm32-unknown-unknown`.
- **Block fragment format:** `#b=<blockId>`, blockId charset `[A-Za-z0-9_-]+`.
- **Snippet cap:** 120 characters (`SNIPPET_MAX_CHARS`), truncated on a char boundary.
- **Indistinguishability (spec §4):** the resolve endpoint returns HTTP 200 with per-target statuses; no-access and nonexistent targets both yield per-target `{"status":"notFound"}` with byte-identical serialization. This deliberately diverges from the document endpoints' 403-vs-404 policy (documented in `crates/api/tests/test_documents.rs:831-873`) and is scoped to this endpoint only.
- **i18n:** every user-facing string added to ALL SIX catalogs `frontend/locales/{en-US,ar,es,it,fr,de}/main.ftl` with real translations.
- **Existing tests are immutable.** New tests only.
- **Public-API note:** Task 1 adds a new `pub fn` to `crates/collab` — additive, deliberate, required by Task 2. No existing signatures change.
- **Do not `git add -A`** — stage exact paths only.
- Line numbers below are from exploration on 2026-07-23 — locate anchors by CONTENT if drifted.

---

## File Structure

**Backend:**
- Modify `crates/collab/src/diff.rs` — add `pub fn block_plain_text` (reuses the private walkers already there).
- Create `crates/api/src/routes/mentions.rs` — resolve endpoint.
- Modify `crates/api/src/routes/mod.rs` — register module + nest.
- Modify `crates/api/src/routes/documents.rs` — only if `load_current_doc_state` needs `pub(crate)`.
- Create `crates/api/tests/test_mentions_resolve.rs`.

**Frontend (`cd frontend/`):**
- Modify `frontend/src/components/editor_context_menu.rs` — `CopyBlockLink` command + menu entry.
- Modify `frontend/src/components/editor_component.rs` — command handler + clipboard helper.
- Modify `frontend/src/pages/document.rs` — fragment parse helper (+ unit tests) + consume-on-load effect + missing-block toast.
- Modify `frontend/style/main.css` — `.block-link-flash` highlight animation.
- Modify `frontend/locales/*/main.ftl` (×6) — two new keys.

---

## Task 1: `block_plain_text` in crates/collab

**Files:**
- Modify: `crates/collab/src/diff.rs` (private walkers `extract_blocks`/`read_block` at ~lines 149-220; `RichBlock` at ~73)

**Interfaces:**
- Consumes: existing private `extract_blocks(doc: &Doc) -> Vec<RichBlock>`; `RichBlock { block_id: Option<String>, inline: Vec<InlineRun>, children: Vec<RichBlock>, .. }`, `InlineRun { text: String, .. }`.
- Produces: `pub fn block_plain_text(doc: &yrs::Doc, block_id: &str, max_chars: usize) -> Option<String>` — the plain text of the block with that id (own inline runs, then children depth-first), char-boundary-truncated to `max_chars`; `None` if no block has that id. Task 2 calls it as `ogrenotes_collab::diff::block_plain_text`.

- [ ] **Step 1: Write the failing unit tests**

Add to the `#[cfg(test)] mod tests` in `crates/collab/src/diff.rs`. **Confirm the doc-construction idiom against the existing tests in this module first** (they build docs and feed the same walkers) and copy that idiom for `doc_with_blocks`; the assertions below are the contract:

```rust
#[test]
fn block_plain_text_finds_block_and_truncates() {
    // Build a doc (existing test idiom in this module) containing:
    //  - paragraph blockId="blk-alpha" text "Hello mention world"
    //  - paragraph blockId="blk-long" text of 200 'x' chars
    let doc = doc_with_blocks(&[("blk-alpha", "Hello mention world"),
                                ("blk-long", &"x".repeat(200))]);
    assert_eq!(
        block_plain_text(&doc, "blk-alpha", 120).as_deref(),
        Some("Hello mention world")
    );
    let long = block_plain_text(&doc, "blk-long", 120).unwrap();
    assert_eq!(long.chars().count(), 120);
}

#[test]
fn block_plain_text_missing_id_is_none() {
    let doc = doc_with_blocks(&[("blk-alpha", "Hello")]);
    assert!(block_plain_text(&doc, "no-such-block", 120).is_none());
}

#[test]
fn block_plain_text_truncates_on_char_boundary() {
    // Multibyte content must not panic or split a char.
    let doc = doc_with_blocks(&[("blk-uni", &"é".repeat(200))]);
    let s = block_plain_text(&doc, "blk-uni", 120).unwrap();
    assert_eq!(s.chars().count(), 120);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ogrenotes-collab block_plain_text`
Expected: FAIL — `block_plain_text` not found (compile error).

- [ ] **Step 3: Implement**

Add to `crates/collab/src/diff.rs` (below the private walkers):

```rust
/// Plain text of the block with the given `blockId` — the block's own
/// inline runs followed by its children, depth-first — truncated to
/// `max_chars` characters (char-boundary safe). `None` if no block in
/// the document carries that id. Public: consumed by the mentions
/// resolve endpoint (`crates/api`) for anchor-mention snippets.
pub fn block_plain_text(doc: &Doc, block_id: &str, max_chars: usize) -> Option<String> {
    fn find<'a>(blocks: &'a [RichBlock], id: &str) -> Option<&'a RichBlock> {
        for b in blocks {
            if b.block_id.as_deref() == Some(id) {
                return Some(b);
            }
            if let Some(hit) = find(&b.children, id) {
                return Some(hit);
            }
        }
        None
    }
    fn collect(b: &RichBlock, out: &mut String) {
        for run in &b.inline {
            out.push_str(&run.text);
        }
        for child in &b.children {
            if !out.is_empty() {
                out.push(' ');
            }
            collect(child, out);
        }
    }
    let blocks = extract_blocks(doc);
    let target = find(&blocks, block_id)?;
    let mut text = String::new();
    collect(target, &mut text);
    Some(text.chars().take(max_chars).collect())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ogrenotes-collab block_plain_text`
Expected: 3 tests PASS. Then `cargo test -p ogrenotes-collab` — full crate still green.

- [ ] **Step 5: Commit**

```bash
git add crates/collab/src/diff.rs
git commit -m "feat(collab): block_plain_text extractor for mention snippets"
```

---

## Task 2: `POST /api/v1/mentions/resolve`

**Files:**
- Create: `crates/api/src/routes/mentions.rs`
- Modify: `crates/api/src/routes/mod.rs` (module list + nest block at ~lines 247-261)
- Modify: `crates/api/src/routes/documents.rs` (ONLY if `load_current_doc_state` at ~1595 is private — make it `pub(crate)`; `check_doc_access` at ~636 is already `pub(crate)`)
- Test: `crates/api/tests/test_mentions_resolve.rs`

**Interfaces:**
- Consumes: `ogrenotes_collab::diff::block_plain_text` (Task 1); `check_doc_access(&state, &doc_id, &user_id, AccessLevel::View) -> Result<DocumentMeta, ApiError>` (`DocumentMeta.title` carries the title — no doc load needed for title-only targets); `load_current_doc_state(&state, &doc_id) -> Result<OgreDoc, ApiError>`; `OgreDoc::inner() -> &Doc`.
- Produces: `POST /api/v1/mentions/resolve` — request `{"targets":[{"docId":"…","blockId":"…"?}]}`, response `{"results":[…]}` same order; per-target `{"status":"ok","title":…,"blockFound":bool,"snippet":…?}` or `{"status":"notFound"}`. Consumed by Plan 2's paste/refresh paths.

- [ ] **Step 1: Write the failing integration tests**

Create `crates/api/tests/test_mentions_resolve.rs`. **Pattern-match the harness idioms** from `test_embed_resolve.rs` (auth-gate shape) and `test_documents.rs:341-378` (doc-with-content via `OgreDoc` + `bytes_request` PUT) — copy imports/setup verbatim from those files. For the block-content test, build the doc with a paragraph carrying a known `blockId` the same way `test_documents.rs`'s content-builder helpers do (e.g. the `build_kanban_doc_with_card_color` idiom at ~505, simplified to one paragraph):

```rust
mod common;

use axum::http::Method;
use serde_json::json;

#[tokio::test]
async fn resolve_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::spawn().await;
    let (status, _) = app
        .json_request(Method::POST, "/api/v1/mentions/resolve", None,
            Some(&json!({ "targets": [{ "docId": "whatever" }] })))
        .await;
    assert_eq!(status, 401);
    app.cleanup().await;
}

#[tokio::test]
async fn resolve_doc_only_returns_title() {
    common::require_infra!();
    let app = common::TestApp::spawn().await;
    let token = app.create_user_token("alice-mr1@test.com").await;
    let doc_id = app.create_doc(&token, "Resolve Me", None).await;
    let (status, body) = app
        .json_request(Method::POST, "/api/v1/mentions/resolve", Some(&token),
            Some(&json!({ "targets": [{ "docId": doc_id }] })))
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["results"][0]["status"], "ok");
    assert_eq!(body["results"][0]["title"], "Resolve Me");
    assert_eq!(body["results"][0]["blockFound"], false);
    assert!(body["results"][0].get("snippet").is_none());
    app.cleanup().await;
}

#[tokio::test]
async fn resolve_block_returns_snippet_and_dangling_is_flagged() {
    common::require_infra!();
    let app = common::TestApp::spawn().await;
    let token = app.create_user_token("alice-mr2@test.com").await;
    let doc_id = app.create_doc(&token, "Has Blocks", None).await;
    // PUT content containing a paragraph with blockId "blk-target" and
    // text "Snippet source text" — build with OgreDoc per the
    // test_documents.rs content-builder idiom, then:
    //   app.bytes_request(Method::PUT, &format!("/api/v1/documents/{doc_id}/content"),
    //       Some(&token), doc.to_state_bytes(), "application/octet-stream").await;
    let (status, body) = app
        .json_request(Method::POST, "/api/v1/mentions/resolve", Some(&token),
            Some(&json!({ "targets": [
                { "docId": doc_id, "blockId": "blk-target" },
                { "docId": doc_id, "blockId": "blk-does-not-exist" }
            ] })))
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["results"][0]["status"], "ok");
    assert_eq!(body["results"][0]["blockFound"], true);
    assert_eq!(body["results"][0]["snippet"], "Snippet source text");
    assert_eq!(body["results"][1]["status"], "ok");   // doc resolves…
    assert_eq!(body["results"][1]["blockFound"], false); // …block dangles
    assert!(body["results"][1].get("snippet").is_none());
    app.cleanup().await;
}

#[tokio::test]
async fn resolve_no_access_is_byte_identical_to_nonexistent() {
    common::require_infra!();
    let app = common::TestApp::spawn().await;
    let owner = app.create_user_token("owner-mr3@test.com").await;
    let stranger = app.create_user_token("stranger-mr3@test.com").await;
    let private_doc = app.create_doc(&owner, "Secret", None).await;

    let (s1, forbidden_body) = app
        .json_request(Method::POST, "/api/v1/mentions/resolve", Some(&stranger),
            Some(&json!({ "targets": [{ "docId": private_doc }] })))
        .await;
    let (s2, missing_body) = app
        .json_request(Method::POST, "/api/v1/mentions/resolve", Some(&stranger),
            Some(&json!({ "targets": [{ "docId": "doc-does-not-exist" }] })))
        .await;
    assert_eq!(s1, 200);
    assert_eq!(s2, 200);
    assert_eq!(forbidden_body["results"][0]["status"], "notFound");
    // The indistinguishability contract (spec §4): the two per-target
    // results must serialize identically — no title, no extra fields.
    assert_eq!(forbidden_body["results"][0], missing_body["results"][0]);
    app.cleanup().await;
}

#[tokio::test]
async fn resolve_batch_caps_targets() {
    common::require_infra!();
    let app = common::TestApp::spawn().await;
    let token = app.create_user_token("alice-mr4@test.com").await;
    let targets: Vec<_> = (0..101).map(|i| json!({ "docId": format!("d{i}") })).collect();
    let (status, _) = app
        .json_request(Method::POST, "/api/v1/mentions/resolve", Some(&token),
            Some(&json!({ "targets": targets })))
        .await;
    assert_eq!(status, 400);
    app.cleanup().await;
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ogrenotes-api --test test_mentions_resolve -- --nocapture`
Expected: FAIL — 404s (route doesn't exist) or compile error. (If infra isn't running locally, `docker compose up -d` per the harness; tests are gated on `require_infra!`.)

- [ ] **Step 3: Implement the endpoint**

Create `crates/api/src/routes/mentions.rs` (**confirm the exact import paths for `AuthUser`, `ApiError`, `AppState` against the top of `documents.rs`** and match them):

```rust
//! Mention resolution (design: docs/superpowers/specs/2026-07-23-document-mentions-design.md §4).
//!
//! Batch-resolves mention targets to live titles/snippets. Access gating
//! runs BEFORE any lookup, and — deliberately unlike the document
//! endpoints' 403-vs-404 policy — an inaccessible target is per-target
//! byte-identical to a nonexistent one, so mention resolution can never
//! leak a document's title or its existence.

use axum::{extract::State, routing::post, Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::error::ApiError;
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

#[derive(Serialize)]
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
        Self { status: "notFound", title: None, block_found: None, snippet: None }
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
            match ogrenotes_collab::diff::block_plain_text(
                doc.inner(),
                block_id,
                SNIPPET_MAX_CHARS,
            ) {
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
```

Note: `resolve_doc_only_returns_title` expects `blockFound: false` for a no-fragment target — the `None => (false, None)` arm provides that. If `ApiError::Forbidden` has different variant names, match what `error.rs:12-36` actually defines.

- [ ] **Step 4: Register the module**

In `crates/api/src/routes/mod.rs`: add `pub mod mentions;` to the module list, and in the nest block (~lines 247-261) add:

```rust
        .nest("/api/v1/mentions", mentions::router())
```

If `load_current_doc_state` in `documents.rs` (~1595) is private, change `async fn` to `pub(crate) async fn` (and same for its return-type visibility if the compiler asks). `check_doc_access` (~636) is already `pub(crate)`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ogrenotes-api --test test_mentions_resolve -- --nocapture`
Expected: 5 tests PASS.

- [ ] **Step 6: Regression check**

Run: `cargo build --workspace && cargo test -p ogrenotes-api --test test_documents`
Expected: clean build; existing document tests untouched and green.

- [ ] **Step 7: Commit**

```bash
git add crates/api/src/routes/mentions.rs crates/api/src/routes/mod.rs crates/api/src/routes/documents.rs crates/api/tests/test_mentions_resolve.rs
git commit -m "feat(api): batch mention-resolve endpoint with access-gated titles/snippets"
```

---

## Task 3: "Copy Link to Block" (frontend producer)

**Files:**
- Modify: `frontend/src/components/editor_context_menu.rs` (command enum ~22-51; entries list ~73-121)
- Modify: `frontend/src/components/editor_component.rs` (command-dispatch Effect ~2759-2825)
- Modify: `frontend/locales/{en-US,ar,es,it,fr,de}/main.ftl`

**Interfaces:**
- Consumes: `EditorContextCommand` dispatch pattern (`item(label_key, cmd)` in the menu; handler arms in `editor_component.rs`'s drain Effect); `state.doc.block_id_at(pos: usize) -> Option<String>` (`model.rs:526`); the `copy_doc_link` clipboard-reflection pattern (`sidebar.rs:185-197`).
- Produces: `EditorContextCommand::CopyBlockLink` menu action that writes `{origin}{pathname}#b=<blockId>` to the clipboard for the block containing the cursor/right-click selection.

- [ ] **Step 1: Add the command variant and menu entry**

In `editor_context_menu.rs`, add to `EditorContextCommand` (~22-51):

```rust
    /// Copy a `{origin}{path}#b=<blockId>` deep link to the block at the
    /// selection (mentions spec §1).
    CopyBlockLink,
```

Add to the entries list (place after the Copy/Cut group, before the Comment entry, matching the list's grouping — pattern from ~84-86):

```rust
            item("menu-copy-block-link", EditorContextCommand::CopyBlockLink),
```

(Not `.disabled_when(empty)` — a collapsed cursor still sits in a block.)

- [ ] **Step 2: Add i18n keys (all six catalogs)**

`frontend/locales/en-US/main.ftl` (near the other `menu-*` keys):
```
menu-copy-block-link = Copy Link to Block
```
`es`: `menu-copy-block-link = Copiar enlace al bloque`
`it`: `menu-copy-block-link = Copia link al blocco`
`fr`: `menu-copy-block-link = Copier le lien du bloc`
`de`: `menu-copy-block-link = Link zum Block kopieren`
`ar`: `menu-copy-block-link = نسخ رابط الكتلة`

- [ ] **Step 3: Add the clipboard helper + handler arm**

In `editor_component.rs`, add a module-level helper (mirror `copy_doc_link`'s reflection pattern exactly — `sidebar.rs:185-197`):

```rust
/// Copy a deep link to `block_id` in the CURRENT document to the
/// clipboard: `{origin}{pathname}#b=<blockId>`. Uses the same
/// `navigator.clipboard.writeText` reflection pattern as
/// `sidebar::copy_doc_link`.
fn copy_block_link(block_id: &str) {
    let Some(window) = web_sys::window() else { return };
    let Ok(origin) = window.location().origin() else { return };
    let Ok(path) = window.location().pathname() else { return };
    let href = format!("{origin}{path}#b={block_id}");
    let write_text = js_sys::Reflect::get(&window.navigator(), &"clipboard".into())
        .and_then(|clip| js_sys::Reflect::get(&clip, &"writeText".into()))
        .and_then(|func| func.dyn_into::<js_sys::Function>());
    if let Ok(write_text) = write_text {
        let clip = js_sys::Reflect::get(&window.navigator(), &"clipboard".into())
            .unwrap_or(wasm_bindgen::JsValue::NULL);
        let _ = write_text.call1(&clip, &href.into());
    }
}
```

Then add a `CopyBlockLink` arm to the command-dispatch Effect (~2759-2825), in the group that borrows the view (**match the surrounding arms' actual bindings for the view borrow and selection access** — the shape below is representative):

```rust
            EditorContextCommand::CopyBlockLink => {
                // Block containing the selection head; block_id_at walks to
                // the innermost block carrying a blockId.
                let view = view_rc.borrow();
                let state = view.state();
                let pos = state.selection.from;
                if let Some(block_id) = state.doc.block_id_at(pos) {
                    copy_block_link(&block_id);
                }
            }
```

- [ ] **Step 4: Build (native + wasm)**

Run: `cd frontend && cargo check && cargo build --target wasm32-unknown-unknown`
Expected: both clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/editor_context_menu.rs frontend/src/components/editor_component.rs frontend/locales/en-US/main.ftl frontend/locales/ar/main.ftl frontend/locales/es/main.ftl frontend/locales/it/main.ftl frontend/locales/fr/main.ftl frontend/locales/de/main.ftl
git commit -m "feat(editor): Copy Link to Block context-menu action (#b= deep links)"
```

---

## Task 4: `#b=` fragment consumption on document load

**Files:**
- Modify: `frontend/src/pages/document.rs` (readiness signal `content_loaded` set at ~612; comment-nav effect precedent ~690-747; `block_id_selector` helper ~177; toast precedent `collab-liveapp-toast` ~469/866/2621)
- Modify: `frontend/style/main.css` — `.block-link-flash`
- Modify: `frontend/locales/{en-US,ar,es,it,fr,de}/main.ftl`

**Interfaces:**
- Consumes: `content_loaded: ReadSignal<bool>`; `dom_position::scroll_to_block(Some(&id), fallback_index) -> bool` and `dom_position::find_block_element(block_id)`; `leptos_router::hooks::use_location` (hash memo, per `settings.rs:84-87`); `gloo_timers::callback::Timeout`.
- Produces: on document load with a `#b=<blockId>` fragment — scroll to the block + ~2s flash highlight, or a transient "linked section no longer exists" toast when the block is gone. Plus `fn block_id_from_hash(raw: &str) -> Option<String>` with native unit tests (Plan 2's paste parser will extend the same rules).

- [ ] **Step 1: Write the failing parser tests**

In `document.rs`, add (or extend) a `#[cfg(test)] mod tests` at the bottom of the file:

```rust
#[cfg(test)]
mod block_fragment_tests {
    use super::block_id_from_hash;

    #[test]
    fn parses_well_formed_fragment() {
        assert_eq!(block_id_from_hash("#b=abc-123_XY").as_deref(), Some("abc-123_XY"));
    }

    #[test]
    fn rejects_empty_missing_and_foreign_hashes() {
        assert_eq!(block_id_from_hash(""), None);
        assert_eq!(block_id_from_hash("#"), None);
        assert_eq!(block_id_from_hash("#b="), None);
        assert_eq!(block_id_from_hash("#appearance"), None); // settings-style tab hash
        assert_eq!(block_id_from_hash("#x=abc"), None);
    }

    #[test]
    fn rejects_invalid_charset() {
        assert_eq!(block_id_from_hash("#b=abc def"), None);
        assert_eq!(block_id_from_hash("#b=abc\"onmouseover"), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd frontend && cargo test block_fragment`
Expected: FAIL — `block_id_from_hash` not found.

- [ ] **Step 3: Implement the parser**

In `document.rs` (module level, near `block_id_selector` at ~177):

```rust
/// Parse a `#b=<blockId>` fragment (mentions spec §1). Returns the block
/// id when the hash is exactly the block form with a valid id charset
/// (`[A-Za-z0-9_-]+`, matching `generate_block_id`); `None` for empty,
/// foreign (e.g. settings-tab), or malformed hashes.
fn block_id_from_hash(raw: &str) -> Option<String> {
    let id = raw.trim_start_matches('#').strip_prefix("b=")?;
    if id.is_empty()
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(id.to_string())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd frontend && cargo test block_fragment`
Expected: 3 tests PASS.

- [ ] **Step 5: Add i18n keys (all six catalogs)**

`en-US`: `doc-block-link-missing = The linked section no longer exists.`
`es`: `doc-block-link-missing = La sección enlazada ya no existe.`
`it`: `doc-block-link-missing = La sezione collegata non esiste più.`
`fr`: `doc-block-link-missing = La section liée n'existe plus.`
`de`: `doc-block-link-missing = Der verlinkte Abschnitt existiert nicht mehr.`
`ar`: `doc-block-link-missing = القسم المرتبط لم يعد موجودًا.`

- [ ] **Step 6: Add the flash CSS**

In `frontend/style/main.css` (near the comment-highlight rules ~1248):

```css
/* Transient flash for a block reached via a #b= deep link (mentions
 * spec §1). Toggled by document.rs after scroll_to_block; removed by
 * a timer. Uses the selection token at low alpha so it reads as a
 * highlight in both themes. */
@keyframes block-link-flash {
  0%   { background-color: color-mix(in srgb, var(--color-primary) 25%, transparent); }
  100% { background-color: transparent; }
}

.block-link-flash {
  animation: block-link-flash 2s ease-out 1;
}
```

(**Confirm `--color-primary` against `tokens-light.css`/`tokens-dark.css`** — it's the token verified in the width-toggle work; if a dedicated selection/highlight token exists nearby, prefer it.)

- [ ] **Step 7: Add the consume-on-load effect**

In `document.rs`, next to the comment-navigation effect (~690-747) and mirroring its structure exactly (readiness gate + handled-once cell + 80ms deferral). Add `use_location` to the `leptos_router::hooks` import at ~line 4. New toast signal near `error` (~259): `let (block_link_missing, set_block_link_missing) = signal(false);`

```rust
    // #b=<blockId> deep-link consumption (mentions spec §1). Mirrors the
    // comment-navigation effect above: gate on content_loaded, handle a
    // given fragment once, defer 80ms so the editor DOM is mounted.
    let location = leptos_router::hooks::use_location();
    let handled_fragment: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    {
        let handled_fragment = handled_fragment.clone();
        Effect::new(move |_| {
            if !content_loaded.get() {
                return;
            }
            let Some(block_id) = block_id_from_hash(&location.hash.get()) else {
                return;
            };
            if handled_fragment.borrow().as_deref() == Some(block_id.as_str()) {
                return;
            }
            *handled_fragment.borrow_mut() = Some(block_id.clone());
            gloo_timers::callback::Timeout::new(80, move || {
                let scrolled =
                    crate::components::dom_position::scroll_to_block(Some(&block_id), usize::MAX);
                if scrolled {
                    // ~2s flash on the target block; class removal via timer.
                    if let Some(el) =
                        crate::components::dom_position::find_block_element(&block_id)
                    {
                        let _ = el.class_list().add_1("block-link-flash");
                        gloo_timers::callback::Timeout::new(2100, move || {
                            let _ = el.class_list().remove_1("block-link-flash");
                        })
                        .forget();
                    }
                } else {
                    // Block gone: open stays at top; transient notice.
                    set_block_link_missing.set(true);
                    gloo_timers::callback::Timeout::new(6000, move || {
                        set_block_link_missing.set(false);
                    })
                    .forget();
                }
            })
            .forget();
        });
    }
```

Notes: `usize::MAX` as `fallback_index` disables the nth-top-level-block fallback (`nth_top_level_block` finds nothing at that index, `scroll_to_block` returns `false`) — a deleted block must NOT scroll to an arbitrary block. **Confirm `find_block_element`'s exact name/signature at `dom_position.rs:203`** and that `Timeout::forget()` matches the file's existing usage (the comment-nav effect uses the same API). If the comment-nav effect stores its `Timeout` instead of `.forget()`, match that idiom.

Render the toast in the `<main>` notice cluster (~2621, next to `collab-liveapp-toast`, same classes/pattern):

```rust
                {move || block_link_missing.get().then(|| view! {
                    <div class="collab-liveapp-toast" role="alert"
                        on:click=move |_| set_block_link_missing.set(false)>
                        {crate::t!("doc-block-link-missing")}
                    </div>
                })}
```

- [ ] **Step 8: Build (native + wasm) + full frontend tests**

Run: `cd frontend && cargo test && cargo check && cargo build --target wasm32-unknown-unknown`
Expected: all clean; parser tests green.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/pages/document.rs frontend/style/main.css frontend/locales/en-US/main.ftl frontend/locales/ar/main.ftl frontend/locales/es/main.ftl frontend/locales/it/main.ftl frontend/locales/fr/main.ftl frontend/locales/de/main.ftl
git commit -m "feat(editor): consume #b= block deep links on document load"
```

---

## Task 5: Manual verification sweep (no code)

**Files:** none (verification only).

**Interfaces:** consumes everything above.

- [ ] **Step 1: Local end-to-end check**

Via the project `verify` skill (local compose stack + trunk dist):
1. Right-click a paragraph → "Copy Link to Block" → clipboard holds `http://…/d/<id>#b=<blockId>`.
2. Open that URL in a new tab → doc loads, scrolls to the block, block flashes ~2s.
3. Delete the block, reload the URL → stays at top, "linked section no longer exists" toast appears and auto-dismisses.
4. `#appearance`-style and malformed hashes on a doc URL → no scroll, no toast, no console errors.
5. Switch locale → menu label + toast localized.
6. `curl` the resolve endpoint (with a dev token): doc-only target → title; block target → snippet; stranger's doc and garbage id → identical `notFound` results.

- [ ] **Step 2: Record outcomes**

Report any failures as findings; do not patch ad hoc without a failing understanding.

---

## Self-Review Notes

- **Spec coverage (Stages 1–2):** `#b=` convention + producer (Task 3) + consumption/scroll/highlight/notice (Task 4) = S1. Resolve endpoint + gating + snippet + full server-side matrix incl. byte-equality (Tasks 1–2) = S2. Batch cap is additive hygiene, noted in spec's endpoint section scope.
- **Conflict resolution of record:** per-target indistinguishability deliberately diverges from the document endpoints' 403/404 existence policy; scoped to `mentions.rs`, documented in its module doc and the Global Constraints.
- **Type consistency:** `block_plain_text(&Doc, &str, usize) -> Option<String>` matches between Task 1 (definition) and Task 2 (call); `block_id_from_hash` charset matches `generate_block_id`'s `[A-Za-z0-9_-]`; response field names (`status`/`title`/`blockFound`/`snippet`) match between endpoint serde (camelCase) and test assertions.
- **Flagged confirm-against-file points** (all marked inline): doc-construction idiom in `diff.rs`/api tests; `AuthUser`/`ApiError` import paths; `load_current_doc_state` visibility; view-borrow bindings in the dispatch Effect; `find_block_element` signature; `--color-primary` vs a dedicated highlight token; `Timeout` idiom.
- **Plan 2 (Stages 3–4)** — `DocMention` element + paste conversion — is written after this plan lands, against the real endpoint and helpers.
