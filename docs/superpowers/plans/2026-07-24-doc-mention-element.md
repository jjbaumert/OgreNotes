# DocMention Element + Paste Conversion (Mentions Plan 2 of 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Stages 3–4 of the mentions spec (`docs/superpowers/specs/2026-07-23-document-mentions-design.md`): the `DocMention` inline leaf atom (both schemas), its render/degradation states, clipboard/export round-trips, paste-time URL→mention conversion with a single-undo guarantee, convert-back, and refresh-on-open.

**Architecture:** `DocMention` mirrors the user `Mention` atom across both schemas (`crates/collab/src/schema.rs` canonical + `frontend/src/editor/schema.rs`, CI-enforced). Paste conversion follows the embed precedent: `on_paste` inserts the plain URL and flags a pending conversion; an Effect resolves via `POST /api/v1/mentions/resolve` and replaces the URL through a guarded transaction that force-groups with the paste in `HistoryPlugin` (new merge-meta — the spec's single-undo hard requirement). Per-viewer render state (live/dangling/missing + fresh titles) is a DOM overlay driven from `document.rs` (the `comment_highlights` pattern) — never persisted to the CRDT. Cached `title`/`snippet` attrs refresh via history-skipped transactions in editable sessions only.

**Tech Stack:** Rust — `crates/collab` (schema/export), frontend Leptos 0.7 CSR/WASM (`frontend/`, outside the workspace), Fluent i18n. Backend endpoint already exists (Plan 1).

## Global Constraints

- **Schema duality:** every node-shape change lands in BOTH `crates/collab/src/schema.rs` and `frontend/src/editor/schema.rs`, and the cross-schema CI tests (`ALL_NODE_TYPES`, count 25→26, tag tables, leaf/inline lists) update in the same commit. Wire tag: `doc_mention`.
- **Attr names (the node's own identity attr is `blockId` — auto-added; do NOT collide):** `url`, `doc_id`, `target_block_id` (empty string ⇒ document mention), `title`, `snippet`. All `String` (attrs are `HashMap<String,String>`).
- **Ephemeral state is never persisted:** live/dangling/missing and per-viewer fresh titles live in Leptos signals + DOM classes only. CRDT attr writes happen only in editable sessions, only on successful resolve, always with the history-skip meta.
- **Single-undo contract (spec §5):** the URL→mention replacement must coalesce with the paste for ONE undo. Implemented via an explicit `history: "merge"` transaction meta honored by `HistoryPlugin::record` (time-independent). If review finds this unreliable, the spec's fallback is offer-conversion, not auto-convert — escalate, don't ship broken undo.
- **`blockFound` is the anchor liveness signal** (spec §4 contract note): empty-string snippet with `blockFound: true` renders as a live anchor.
- **Frontend is outside the workspace** — `cd frontend/`; editor code is wasm-gated: verify with `cargo check` AND `cargo build --target wasm32-unknown-unknown`.
- **i18n:** new user-facing strings in ALL SIX catalogs with real translations.
- **Existing tests are immutable** — cross-schema count/list tests are explicitly designed to be extended (adding entries + bumping the count is the designed extension point, not a behavior change).
- **Do not `git add -A`.** Line numbers are from exploration on 2026-07-24 — anchor by content.

---

## File Structure

**Task 1 (element exists):** `crates/collab/src/schema.rs`, `crates/collab/src/export.rs`, `frontend/src/editor/model.rs`, `frontend/src/editor/schema.rs`, `frontend/src/editor/yrs_bridge.rs`
**Task 2 (render + clipboard + click):** `frontend/src/editor/view.rs`, `frontend/src/editor/clipboard.rs`, `frontend/style/main.css`
**Task 3 (paste + undo):** `frontend/src/editor/plugins.rs`, `frontend/src/editor/mention_url.rs` (new), `frontend/src/editor/view.rs`, `frontend/src/editor/commands.rs`, `frontend/src/components/editor_component.rs`, `frontend/src/api/documents.rs`
**Task 4 (context menu + convert):** `frontend/src/components/editor_context_menu.rs`, `frontend/src/components/editor_component.rs`, `frontend/src/editor/commands.rs`, 6× `frontend/locales/*/main.ftl`
**Task 5 (refresh + overlay):** `frontend/src/components/mention_overlay.rs` (new), `frontend/src/components/mod.rs`, `frontend/src/pages/document.rs`, `frontend/style/main.css`, 6× `frontend/locales/*/main.ftl`
**Task 6:** manual verification (no code)

---

## Task 1: The `DocMention` node in both schemas + model + export

**Files:**
- Modify: `crates/collab/src/schema.rs` (enum ~55; `tag_name` ~84; `from_tag` ~115; `is_inline` ~176; `is_leaf` ~181; `valid_children` leaf arm ~273; `NodeSpec` insert ~662; tests: `node_type_tag_roundtrip` types array ~343, `ALL_NODE_TYPES` ~468, count assert ~527, tag table ~561, `expected_leaves` ~626, `expected_inline` ~646)
- Modify: `crates/collab/src/export.rs` (markdown Mention arm ~1180; HTML arms ~785/~1468; tag maps ~1310)
- Modify: `frontend/src/editor/model.rs` (enum ~180; `is_leaf` ~204; `is_inline` ~212; `is_atom` ~237; `is_commentable` exhaustive ~278; `needs_block_id` exhaustive ~326; `default_attrs` ~393; `text_content` ~642; tests ~1676)
- Modify: `frontend/src/editor/schema.rs` (`NodeSpec` insert ~662)
- Modify: `frontend/src/editor/yrs_bridge.rs` (tag maps ~857-888+; do NOT touch the leaf-fallback `is_leaf` list — `needs_block_id()==true` means Strategy 1 always applies, per the comment there)

**Interfaces:**
- Produces: `NodeType::DocMention` — inline, leaf, atom, `needs_block_id`, NOT commentable; wire tag `doc_mention`; `default_attrs()` = `{url, doc_id, target_block_id, title, snippet}` all `""`; `text_content()` returns the `title` attr. Export: markdown `[title](url)`, HTML `<a class="doc-mention" data-doc-id data-block-id href>title</a>`. Tasks 2–5 depend on all of these.

- [ ] **Step 1: Write the failing model tests (frontend)**

In `frontend/src/editor/model.rs` tests, next to the Mention test (~1676), add:

```rust
    #[test]
    fn doc_mention_is_inline_leaf_atom_with_expected_attrs() {
        assert!(NodeType::DocMention.is_leaf());
        assert!(NodeType::DocMention.is_atom());
        assert!(NodeType::DocMention.is_inline());
        assert!(!NodeType::DocMention.is_block());
        assert!(!NodeType::DocMention.is_commentable());
        assert!(NodeType::DocMention.needs_block_id());
        let attrs = NodeType::DocMention.default_attrs();
        assert_eq!(attrs.len(), 5);
        for key in ["url", "doc_id", "target_block_id", "title", "snippet"] {
            assert!(attrs.contains_key(key), "missing default attr {key}");
        }
    }

    #[test]
    fn doc_mention_text_content_is_title() {
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("title".to_string(), "Target Doc".to_string());
        attrs.insert("url".to_string(), "/d/abc".to_string());
        let node = Node::element_with_attrs(NodeType::DocMention, attrs, vec![]);
        assert_eq!(node.text_content(), "Target Doc");
        assert_eq!(node.node_size(), 1); // leaf atom = one position
    }
```

(Adapt `element_with_attrs`'s exact call shape to the file's existing test usage.)

- [ ] **Step 2: Write the failing cross-schema/backed tests additions**

In `crates/collab/src/schema.rs` tests: add `NodeType::DocMention` to `ALL_NODE_TYPES` and the `node_type_tag_roundtrip` types array; bump the count assert 25→26; add `("doc_mention", NodeType::DocMention)` to the tag table; add DocMention to `expected_leaves` and `expected_inline`.

- [ ] **Step 3: Run to verify failures**

Run: `cargo test -p ogrenotes-collab schema` and `cd frontend && cargo test doc_mention`
Expected: compile errors (`DocMention` not found) — the RED state.

- [ ] **Step 4: Implement backend schema**

In `crates/collab/src/schema.rs`, following Mention at every site:

```rust
    /// Inline document/anchor mention as a leaf atom (mentions spec §2).
    /// Carries the pasted `url`, target `doc_id`, optional
    /// `target_block_id` ("" ⇒ document mention), and cached
    /// `title`/`snippet`. Live/degraded render state is per-viewer and
    /// never stored. Single-keystroke delete like Mention.
    DocMention,
```

- `tag_name`: `NodeType::DocMention => "doc_mention"`
- `from_tag`: `"doc_mention" => Some(NodeType::DocMention)`
- `is_inline`: extend to `matches!(self, NodeType::HardBreak | NodeType::Mention | NodeType::DocMention)`
- `is_leaf`: add `NodeType::DocMention`
- `valid_children` leaf arm: add `NodeType::DocMention`
- `NodeSpec` insert (copy the Mention block ~662-676 verbatim, keyed `NodeType::DocMention`; same flags: `leaf: true, atom: true, block: false, allowed_marks: Some(vec![])`)

- [ ] **Step 5: Implement export.rs arms**

Markdown renderer (next to the Mention arm ~1180): DocMention emits `[title](url)` using the existing link shape (`format!("[{}]({})", label, escape_md_url(&href))` — reuse the same escaping helpers; empty title falls back to the url as the label). HTML renderer (~1468 area) + its tag map: emit `<a class="doc-mention" data-doc-id="…" data-block-id="…" href="…">title</a>` (escaped; omit `data-block-id` when `target_block_id` is empty). Add the `NodeType::DocMention => "a"` arm to export's tag maps.

- [ ] **Step 6: Implement frontend model + schema + yrs_bridge**

`model.rs`: enum variant (same doc comment), `is_leaf`/`is_inline`/`is_atom` additions, `is_commentable => false` arm, `needs_block_id => true` arm, `default_attrs` arm inserting the five keys as empty `String`s, `text_content` arm returning the `title` attr (fall back to `url` when title empty). `schema.rs`: `NodeSpec` insert mirroring the backend. `yrs_bridge.rs`: add `NodeType::DocMention => "doc_mention"` (and reverse) to BOTH exhaustive tag maps; leave the leaf-fallback list alone.

- [ ] **Step 7: Run to verify green (both sides)**

Run: `cargo test -p ogrenotes-collab` (full crate — cross-schema suite + export + everything) and `cd frontend && cargo test && cargo check`
Expected: all green. Compile errors from remaining exhaustive matches in `view.rs`/`clipboard.rs` tag maps may surface — if so, add the minimal `NodeType::DocMention => "span"` arms there now (Task 2 replaces them with the real render) and note it in the report.

- [ ] **Step 8: Commit**

```bash
git add crates/collab/src/schema.rs crates/collab/src/export.rs frontend/src/editor/model.rs frontend/src/editor/schema.rs frontend/src/editor/yrs_bridge.rs
git commit -m "feat(editor): DocMention node type in both schemas + export"
```

(If Step 7 forced arms in `view.rs`/`clipboard.rs`, stage those too and say so.)

---

## Task 2: Render, CSS, clipboard round-trip, click-to-navigate

**Files:**
- Modify: `frontend/src/editor/view.rs` (Mention render ~1291-1326 as the model; `node_type_to_tag` ~1601; delegated `on_click` ~1028-1053)
- Modify: `frontend/src/editor/clipboard.rs` (Mention parse ~145-161; serialize ~1055-1064)
- Modify: `frontend/style/main.css` (`.mention` block ~2752 as the model)

**Interfaces:**
- Consumes: Task 1's node type + attrs; `is_safe_url` (`view.rs:1543`); the delegated-listener pattern (no per-node listeners).
- Produces: in-editor chip `<span class="doc-mention" contenteditable="false" data-atom-size data-doc-id data-block-id data-url data-node-block-id>` with icon glyph + text; copy-out HTML `<a class="doc-mention" …>` (matches export); paste-parse for BOTH `a.doc-mention[data-doc-id]` and `span.doc-mention[data-doc-id]`; plain click navigates (Ctrl not required — atoms aren't editable text).

- [ ] **Step 1: Write the failing clipboard round-trip test**

In `clipboard.rs` tests (pattern-match the existing Mention round-trip test if one exists; otherwise the serialize/parse pair):

```rust
    #[test]
    fn doc_mention_round_trips_through_html() {
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("url".into(), "https://notes.example/d/abc#b=blk1".into());
        attrs.insert("doc_id".into(), "abc".into());
        attrs.insert("target_block_id".into(), "blk1".into());
        attrs.insert("title".into(), "Roadmap".into());
        attrs.insert("snippet".into(), "Q3 goals".into());
        let node = Node::element_with_attrs(NodeType::DocMention, attrs, vec![]);
        let html = serialize_nodes_for_test(&[node]); // use the file's real serialize entry point
        assert!(html.contains(r#"class="doc-mention""#));
        assert!(html.contains(r#"data-doc-id="abc""#));
        let parsed = parse_from_html(&html);
        // exactly one DocMention with the same doc_id/target/url/title
        // (assert via the file's existing slice-inspection idiom)
    }
```

**Confirm the real serialize/parse test entry points and assertion idiom against the file's existing tests** — note `parse_from_html` needs a DOM, so if existing clipboard tests are wasm-gated or use a helper, follow that (a native-only serialize assertion + a separate wasm-covered parse is acceptable if that's the file's pattern; report what you found).

- [ ] **Step 2: RED**

Run: `cd frontend && cargo test doc_mention_round_trips` (or `cargo check` if the test is wasm-gated)
Expected: fails/doesn't compile.

- [ ] **Step 3: Implement render**

In `view.rs` `render_node`, next to Mention (~1291), add the DocMention arm:

```rust
            NodeType::DocMention => {
                let el = doc.create_element("span")?;
                el.set_attribute("class", "doc-mention")?;
                el.set_attribute("contenteditable", "false")?;
                el.set_attribute("data-atom-size", &node.node_size().to_string())?;
                let get = |k: &str| attrs.get(k).map(String::as_str).unwrap_or("");
                el.set_attribute("data-doc-id", get("doc_id"))?;
                if !get("target_block_id").is_empty() {
                    el.set_attribute("data-block-id-target", get("target_block_id"))?;
                }
                el.set_attribute("data-url", get("url"))?;
                // Node's own identity id — the overlay (Task 5) keys on it.
                if let Some(bid) = node.block_id() {
                    el.set_attribute("data-node-block-id", bid)?;
                }
                // Glyph: ⚓ for anchor mentions, 📄 for document mentions.
                let is_anchor = !get("target_block_id").is_empty();
                let icon = if is_anchor { "\u{2693} " } else { "\u{1F4C4} " };
                let label = if is_anchor && !get("snippet").is_empty() {
                    get("snippet").to_string()
                } else if !get("title").is_empty() {
                    get("title").to_string()
                } else {
                    get("url").to_string()
                };
                el.set_text_content(Some(&format!("{icon}{label}")));
                el
            }
```

(Adapt to the arm's real surrounding shape — how `attrs` is in scope, error handling, how the element is returned/appended; the Mention arm is the source of truth. Do NOT use `data-block-id` for the TARGET id — that attribute means "this node's own block" everywhere else in the DOM; use `data-block-id-target`. NOTE this deliberately refines the spec's copy-out sketch for the IN-EDITOR DOM only; the clipboard/export `<a>` format keeps spec §7's `data-block-id` name since it leaves the app.) Update `node_type_to_tag` (~1601): `NodeType::DocMention => "span"`.

- [ ] **Step 4: Implement clipboard parse + serialize**

Parse (next to the Mention branch ~145): recognize `a`/`span` with class `doc-mention` and non-empty `data-doc-id`; build the node with `url` (from `href` or `data-url`), `doc_id`, `target_block_id` (from `data-block-id`, per the copy-out format), `title` (text content, icon glyph stripped: trim a leading `⚓ `/`📄 `), empty `snippet`. Serialize (~1055): emit the spec §7 `<a class="doc-mention" data-doc-id="…" data-block-id="…" href="…">title</a>` (escape everything; omit `data-block-id` when target empty) — matching export.rs so re-paste round-trips.

- [ ] **Step 5: Click-to-navigate**

In the delegated `on_click` (~1028): before/alongside the `<a>` ancestor walk, walk ancestors for `span.doc-mention`; if found and its class list does NOT contain `doc-mention-missing`, read `data-url`, check `is_safe_url`, and navigate via `window.location().set_href(url)` (the codebase's full-reload nav precedent). Plain click (no Ctrl gate — the chip is `contenteditable=false`, clicks can't be text edits).

- [ ] **Step 6: CSS**

Next to `.mention` (~2752):

```css
/* Document/anchor mention chip (mentions spec §3). State classes are
 * applied per-viewer by the mention overlay — never persisted. */
.doc-mention {
  display: inline;
  background: var(--color-mention-bg, rgba(59, 130, 246, 0.12));
  color: var(--color-mention-fg, #1d4ed8);
  padding: 0 4px;
  border-radius: 4px;
  font-weight: 500;
  cursor: pointer;
}

.doc-mention.doc-mention-dangling {
  border-bottom: 1px dashed currentColor;
}

.doc-mention.doc-mention-missing {
  background: var(--color-bg-hover);
  color: var(--color-text-secondary);
  cursor: default;
}
```

- [ ] **Step 7: GREEN + builds**

Run: `cd frontend && cargo test && cargo check && cargo build --target wasm32-unknown-unknown`
Expected: round-trip test green (or its wasm-gated equivalent compiles), everything clean.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/editor/view.rs frontend/src/editor/clipboard.rs frontend/style/main.css
git commit -m "feat(editor): DocMention chip render, clipboard round-trip, click-to-navigate"
```

---

## Task 3: URL parsing, paste conversion, single-undo merge

**Files:**
- Create: `frontend/src/editor/mention_url.rs` (pure URL parser + conversion ladder — natively testable)
- Modify: `frontend/src/editor/mod.rs` (or wherever editor submodules are declared — register `pub mod mention_url;`)
- Modify: `frontend/src/editor/plugins.rs` (`HistoryPlugin::record` ~95-160)
- Modify: `frontend/src/editor/view.rs` (`on_paste` ~801-1024)
- Modify: `frontend/src/editor/commands.rs` (next to `insert_user_mention` ~822)
- Modify: `frontend/src/components/editor_component.rs` (embed-Effect pattern ~2634-2739; pending-signal pattern like `pending_ctx_cmd` ~2749)
- Modify: `frontend/src/api/documents.rs` (next to `resolve_embed` ~403)

**Interfaces:**
- Consumes: `POST /api/v1/mentions/resolve` (Plan 1): request `{targets:[{docId, blockId?}]}` → `{results:[{status, title?, blockFound?, snippet?}]}`; `api_post` (`client.rs:342`); `apply_and_notify` (editor_component.rs ~64); Task 1's node.
- Produces:
  - `mention_url::ParsedDocUrl { doc_id: String, block_id: Option<String> }` and `pub fn parse_ogre_doc_url(text: &str, origin: &str) -> Option<ParsedDocUrl>` — Some only for a LONE same-origin doc URL (`{origin}/d/<id>`, optional `/slug`, optional `#b=<id>` with charset `[A-Za-z0-9_-]+`; trailing/leading whitespace tolerated; anything else None).
  - `HistoryPlugin` honors `meta["history"] == "merge"` (group with previous entry regardless of time) and `"skip"` (don't record — Task 5 uses it).
  - `commands::replace_text_with_doc_mention(state, from, to, expected_text, attrs) -> Option<Transaction>` — returns None (abort) unless `state.doc` text at `[from,to)` still equals `expected_text`; transaction carries `meta("history","merge")`.
  - `api::documents::resolve_mentions(targets) -> Result<Vec<MentionResolveResult>, ApiClientError>`.

- [ ] **Step 1: Failing native tests — URL parser (the paste-side resolution matrix rows)**

In `mention_url.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    const ORIGIN: &str = "https://notes.example";

    #[test]
    fn parses_doc_slug_and_fragment_variants() {
        assert_eq!(parse_ogre_doc_url("https://notes.example/d/abc123", ORIGIN),
            Some(ParsedDocUrl { doc_id: "abc123".into(), block_id: None }));
        assert_eq!(parse_ogre_doc_url("https://notes.example/d/abc123/some-slug", ORIGIN),
            Some(ParsedDocUrl { doc_id: "abc123".into(), block_id: None }));
        assert_eq!(parse_ogre_doc_url("  https://notes.example/d/abc123#b=blk_1  ", ORIGIN),
            Some(ParsedDocUrl { doc_id: "abc123".into(), block_id: Some("blk_1".into()) }));
        assert_eq!(parse_ogre_doc_url("https://notes.example/d/abc/slug#b=blk-2", ORIGIN),
            Some(ParsedDocUrl { doc_id: "abc".into(), block_id: Some("blk-2".into()) }));
    }

    #[test]
    fn rejects_foreign_multi_and_malformed() {
        for bad in [
            "https://other.example/d/abc123",            // foreign origin
            "https://notes.example/settings",            // not a doc path
            "https://notes.example/d/",                  // empty id
            "see https://notes.example/d/abc123 please", // not a lone URL
            "https://notes.example/d/abc#b=bad id",      // invalid fragment charset
            "https://notes.example/d/abc#appearance",    // foreign hash
            "not a url",
        ] {
            assert_eq!(parse_ogre_doc_url(bad, ORIGIN), None, "should reject: {bad}");
        }
    }

    #[test]
    fn foreign_hash_still_resolves_doc_without_block() {
        // Spec ladder: fragment that isn't the block form ⇒ treat as doc-only?
        // NO — a foreign hash means the URL addresses something we don't
        // understand; conservative choice (recorded): reject entirely, leave
        // the plain URL. This test pins that decision.
        assert_eq!(parse_ogre_doc_url("https://notes.example/d/abc#appearance", ORIGIN), None);
    }
}
```

- [ ] **Step 2: Failing native tests — history merge/skip**

In `plugins.rs` tests (native `current_time_ms()==0` means time-grouping never fires — perfect isolation):

```rust
    #[test]
    fn merge_meta_groups_with_previous_entry_regardless_of_time() {
        // build a state, record txn A (normal), then txn B with
        // meta("history","merge") — undo_stack length must stay 1 and a
        // single undo must revert BOTH. Pattern-match the file's existing
        // record/undo test idiom for constructing states and transactions.
    }

    #[test]
    fn skip_meta_records_nothing() {
        // txn with meta("history","skip") leaves undo_stack unchanged and
        // does not clear the redo stack.
    }
```

(**The bodies must follow the file's existing HistoryPlugin test idiom** — there are `record`/`undo` tests in `plugins.rs`; copy their state-construction verbatim. The assertions above are the contract.)

- [ ] **Step 3: RED**

Run: `cd frontend && cargo test mention_url && cargo test plugins` (adjust filter to the real test-module names)
Expected: compile failures.

- [ ] **Step 4: Implement `mention_url.rs`**

```rust
//! Lone-URL paste detection for document/anchor mentions (spec §5).
//! Pure string logic — natively tested; the wasm paste path calls it
//! with `window.location.origin()`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDocUrl {
    pub doc_id: String,
    pub block_id: Option<String>,
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parse a pasted string that is exactly one same-origin document URL.
/// `Some` for `{origin}/d/<id>[/slug][#b=<blockId>]`; `None` otherwise
/// (foreign origin, embedded-in-text, malformed fragment — the paste
/// then stays a plain link, spec case c).
pub fn parse_ogre_doc_url(text: &str, origin: &str) -> Option<ParsedDocUrl> {
    let t = text.trim();
    if t.contains(char::is_whitespace) {
        return None; // lone URL only
    }
    let rest = t.strip_prefix(origin)?.strip_prefix("/d/")?;
    let (path, frag) = match rest.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (rest, None),
    };
    let doc_id = path.split('/').next().unwrap_or("");
    if !valid_id(doc_id) {
        return None;
    }
    let block_id = match frag {
        None => None,
        Some(f) => {
            let id = f.strip_prefix("b=")?; // foreign hash forms ⇒ reject whole URL
            if !valid_id(id) {
                return None;
            }
            Some(id.to_string())
        }
    };
    Some(ParsedDocUrl { doc_id: doc_id.to_string(), block_id })
}
```

Register the module beside the other editor modules (match how `markdown`/`clipboard` are declared).

- [ ] **Step 5: Implement history merge/skip**

In `HistoryPlugin::record` (~95): at the top, if `transaction.meta("history") == Some("skip")` → return (alongside the existing undo/redo skip). Where `should_group` is computed (~131): `let force_merge = transaction.meta("history") == Some("merge");` and `let should_group = force_merge || (existing time-window logic)` — but `force_merge` must ALSO group when `last_change_time` is None (as long as `undo_stack` is non-empty). Match the file's real `meta` accessor signature (it's used for `"history" == "undo"/"redo"` already — reuse that exact accessor).

- [ ] **Step 6: Implement the pipeline**

1. **`api/documents.rs`** (mirror `resolve_embed` ~403):

```rust
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionResolveTarget<'a> {
    doc_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<&'a str>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionResolveRequest<'a> { targets: Vec<MentionResolveTarget<'a>> }

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionResolveResult {
    pub status: String,
    #[serde(default)] pub title: Option<String>,
    #[serde(default)] pub block_found: Option<bool>,
    #[serde(default)] pub snippet: Option<String>,
}

#[derive(serde::Deserialize)]
struct MentionResolveResponse { results: Vec<MentionResolveResult> }

/// Batch-resolve mention targets. Order matches the input.
pub async fn resolve_mentions(
    targets: &[(String, Option<String>)],
) -> Result<Vec<MentionResolveResult>, ApiClientError> {
    let body = MentionResolveRequest {
        targets: targets.iter()
            .map(|(d, b)| MentionResolveTarget { doc_id: d, block_id: b.as_deref() })
            .collect(),
    };
    let resp: MentionResolveResponse =
        client::api_post("/mentions/resolve", &body).await?;
    Ok(resp.results)
}
```

(**Confirm** the `api_post` path convention — whether the `/api/v1` prefix is added by the client — against `resolve_embed`'s path usage.)

2. **`commands.rs`** (next to `insert_user_mention` ~822):

```rust
/// Replace `[from,to)` — which must still contain exactly `expected_text`
/// (the pasted URL) — with a DocMention atom. Returns None when the
/// document changed underneath (concurrent edit): the paste then stays a
/// plain URL, per spec §5 case c. The transaction carries the
/// `history: merge` meta so it coalesces with the paste into ONE undo.
pub fn replace_text_with_doc_mention(
    state: &EditorState,
    from: usize,
    to: usize,
    expected_text: &str,
    attrs: std::collections::HashMap<String, String>,
) -> Option<Transaction> {
    if state.doc.text_between(from, to)? != expected_text {
        return None;
    }
    let node = Node::element_with_attrs(NodeType::DocMention, attrs, vec![]);
    let slice = Slice::from_nodes(vec![node]);
    let mut txn = state.transaction().replace(from, to, slice);
    txn = txn.set_meta("history", "merge");
    Some(txn.set_selection(Selection::cursor(from + 1)))
}
```

(**Adapt every API call** — `text_between`, `Slice::from_nodes`, `set_meta`, `Selection::cursor`, builder-vs-mut style — to the real signatures visible in `insert_user_mention` and the Transaction impl. The guard, the meta, and the `from+1` cursor are the contract.)

3. **`view.rs` `on_paste`**: right after the html/text extraction (~820), add the lone-URL branch BEFORE the generic paste handling:

```rust
            // Mentions spec §5: a lone same-origin doc URL pastes as plain
            // text immediately (no latency), then async-converts. Only when
            // the paste is EXACTLY one URL — multi-URL/rich pastes fall
            // through to the normal paths untouched.
            if html.is_empty() || is_trivial_html {
                if let Ok(origin) = web_sys::window().unwrap().location().origin() {
                    if let Some(parsed) =
                        crate::editor::mention_url::parse_ogre_doc_url(&text, &origin)
                    {
                        let url = text.trim().to_string();
                        let from = /* selection start per this closure's existing vars */;
                        let txn = state_with_sel.transaction().insert_text(&url);
                        dispatch_paste(txn);
                        pending_mention_paste.set(Some(PendingMentionPaste {
                            from,
                            to: from + url.chars().count(),
                            url,
                            parsed,
                        }));
                        return;
                    }
                }
            }
```

Where `pending_mention_paste` is a Leptos `WriteSignal<Option<PendingMentionPaste>>` passed into the view at construction the same way other hooks are (**find how the view receives its existing callbacks/dispatch from `editor_component.rs` and mirror it**; if the view takes no Leptos signals today, use an `Rc<RefCell<Option<...>>>` + a nudge via the existing `on_change` flow — the `pending_ctx_cmd` signal in editor_component ~2749 is the drain-pattern precedent). `PendingMentionPaste { from: usize, to: usize, url: String, parsed: ParsedDocUrl }` lives in `mention_url.rs`. **The `from`/`to` must be model positions of the inserted text** — derive them exactly as the closure's other branches compute replace ranges; `to` uses the position delta of the insert, not char count, if those differ (**verify against how insert_text positions work**).

4. **`editor_component.rs`**: an Effect (mirror the embed Effect ~2634) drains `pending_mention_paste`, then:

```rust
            leptos::task::spawn_local(async move {
                let targets = vec![(p.parsed.doc_id.clone(), p.parsed.block_id.clone())];
                let results = crate::api::documents::resolve_mentions(&targets).await;
                let Ok(results) = results else { return }; // network error: URL stays
                let Some(r) = results.first() else { return };
                if r.status != "ok" { return; } // notFound: URL stays (case c)
                // a/b ladder: anchor when the block resolved, else document
                let block_found = r.block_found.unwrap_or(false);
                let mut attrs = std::collections::HashMap::new();
                attrs.insert("url".into(), p.url.clone());
                attrs.insert("doc_id".into(), p.parsed.doc_id.clone());
                attrs.insert(
                    "target_block_id".into(),
                    // dangling fragment keeps the id (spec case b: indicator + notice)
                    p.parsed.block_id.clone().unwrap_or_default(),
                );
                attrs.insert("title".into(), r.title.clone().unwrap_or_default());
                attrs.insert(
                    "snippet".into(),
                    if block_found { r.snippet.clone().unwrap_or_default() } else { String::new() },
                );
                // back on the reactive side: guarded replace + merge-undo
                // (route through the same snapshot/dispatch machinery the
                // embed Effect uses — apply_and_notify with history recording)
                ...
            });
```

The post-await dispatch must re-read the CURRENT state (not the pre-await snapshot) and call `commands::replace_text_with_doc_mention(&current, p.from, p.to, &p.url, attrs)`; on `Some(txn)` route it through `apply_and_notify` exactly like other command dispatches; on `None`, do nothing. (**The await-then-dispatch shape must match how toolbar.rs's embed flow re-enters the reactive world** — it goes `spawn_local → on_command.run(...) → command_signal → Effect`; if dispatching directly from the spawn_local is not the codebase way, add a `ToolbarCommand`-style pending command instead and handle it in the existing command Effect. Follow the precedent, not this sketch.)

- [ ] **Step 7: GREEN + builds**

Run: `cd frontend && cargo test && cargo check && cargo build --target wasm32-unknown-unknown`
Expected: parser + history tests green; builds clean.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/editor/mention_url.rs frontend/src/editor/plugins.rs frontend/src/editor/view.rs frontend/src/editor/commands.rs frontend/src/components/editor_component.rs frontend/src/api/documents.rs
git commit -m "feat(editor): paste a doc URL to create a DocMention (single-undo)"
```

(Plus the module-registration file if separate.)

---

## Task 4: Element context menu — Copy Original URL / Convert to Plain Link

**Files:**
- Modify: `frontend/src/components/editor_context_menu.rs` (enum ~23-54; entries builder ~76-125)
- Modify: `frontend/src/components/editor_component.rs` (`on:contextmenu` ~3078-3094; command match ~2837; `copy_block_link` ~609 as the clipboard precedent)
- Modify: `frontend/src/editor/commands.rs`
- Modify: 6× `frontend/locales/*/main.ftl`

**Interfaces:**
- Consumes: Task 2's chip DOM (`span.doc-mention[data-url][data-node-block-id]`); `insert_doc_link` (commands.rs ~745) as the linked-text insert precedent; `find_block_content_start`-style lookups if needed.
- Produces: right-clicking a DocMention adds two entries — `menu-copy-original-url` ("Copy Original URL", works in every state incl. missing) and `menu-convert-to-plain-link` ("Convert to Plain Link"); new `EditorContextCommand::{CopyOriginalUrl, ConvertMentionToLink}` variants; `commands::convert_doc_mention_to_link(state, node_block_id) -> Option<Transaction>` replacing the atom with `title` text carrying a Link mark to `url`.

- [ ] **Step 1: i18n keys (all six catalogs)**

`en-US`:
```
menu-copy-original-url = Copy Original URL
menu-convert-to-plain-link = Convert to Plain Link
```
`es`: `Copiar URL original` / `Convertir en enlace normal`
`it`: `Copia URL originale` / `Converti in link normale`
`fr`: `Copier l'URL d'origine` / `Convertir en lien simple`
`de`: `Original-URL kopieren` / `In normalen Link umwandeln`
`ar`: `نسخ الرابط الأصلي` / `تحويل إلى رابط عادي`

- [ ] **Step 2: Context detection**

In the `on:contextmenu` handler (~3078): walk up from `e.target()` (the same ancestry-walk idiom as view.rs's `on_click`) looking for `span.doc-mention`; capture `Option<DocMentionCtx { url: String, node_block_id: String }>` into a new signal `ctx_doc_mention`; pass it to `EditorContextMenu` as a new `#[prop(into)] doc_mention: Signal<Option<DocMentionCtx>>` prop, and add the two entries to the builder when it's `Some` (grouped with a separator, after the CopyBlockLink entry). New enum variants carry no payload — the handler reads `ctx_doc_mention` when dispatching (same pattern as other stateful commands).

- [ ] **Step 3: Commands**

`CopyOriginalUrl`: clipboard-write `ctx.url` via the same reflection helper pattern as `copy_block_link` (~609) — factor or duplicate per the file's convention. `ConvertMentionToLink`: add

```rust
/// Replace a DocMention atom (found by its own blockId) with plain text
/// `title` (falling back to the url) carrying a Link mark to `url` —
/// the permanent opt-out (spec §5). Normal undo entry (no merge meta).
pub fn convert_doc_mention_to_link(state: &EditorState, node_block_id: &str) -> Option<Transaction> { … }
```

Locate the node position by walking `state.doc` for the DocMention whose `blockId` attr equals `node_block_id` (reuse/extend the doc-walk idiom in `find_block_content_start` ~563); build the replacement text node with the Link mark exactly as `insert_doc_link` (~745) does; replace the 1-position atom.

- [ ] **Step 4: Native test for the conversion**

In `commands.rs` tests (pattern-match `insert_user_mention`'s test if present, else the module's test idiom): build a state containing a DocMention with known attrs, run `convert_doc_mention_to_link`, assert the atom is gone, the text equals the title, and the Link mark's href equals the url. RED→GREEN.

- [ ] **Step 5: Builds + commit**

Run: `cd frontend && cargo test && cargo check && cargo build --target wasm32-unknown-unknown`

```bash
git add frontend/src/components/editor_context_menu.rs frontend/src/components/editor_component.rs frontend/src/editor/commands.rs frontend/locales/en-US/main.ftl frontend/locales/ar/main.ftl frontend/locales/es/main.ftl frontend/locales/it/main.ftl frontend/locales/fr/main.ftl frontend/locales/de/main.ftl
git commit -m "feat(editor): DocMention context actions — copy original URL, convert to link"
```

---

## Task 5: Refresh-on-open + per-viewer degradation overlay

**Files:**
- Create: `frontend/src/components/mention_overlay.rs`
- Modify: `frontend/src/components/mod.rs` (register)
- Modify: `frontend/src/pages/document.rs` (editor_state signal ~337; `extract_outline` walk precedent — `document_outline.rs:21`; comment_highlights wiring ~15/892 as the overlay precedent)
- Modify: `frontend/style/main.css` (state classes shipped in Task 2 — extend only if needed)
- Modify: 6× `frontend/locales/*/main.ftl`

**Interfaces:**
- Consumes: `editor_state: ReadSignal<Option<EditorState>>`; `resolve_mentions` (Task 3); the chip DOM (`data-node-block-id`, `data-doc-id`, `data-block-id-target`); history `skip` meta (Task 3); `blockFound` liveness contract.
- Produces:
  - `mention_overlay::scan_doc_mentions(doc: &Node) -> Vec<MentionRef { node_block_id, doc_id, target_block_id: Option<String> }>` (pure walk, natively tested).
  - On document open: one batch resolve for all mentions; a `mention_states: RwSignal<HashMap<String, MentionState>>` (`MentionState::{Live{title, snippet}, Dangling{title}, Missing}`) keyed by `node_block_id`.
  - A DOM-decoration Effect (comment_highlights pattern): applies `doc-mention-missing`/`doc-mention-dangling` classes and swaps in fresh title/snippet text; re-applies when `editor_state` changes (editor re-renders rebuild the spans). Missing state also sets `title` tooltip via the i18n key.
  - Editable sessions only: after a successful resolve whose title/snippet differ from the cached attrs, dispatch attr-refresh transactions tagged `history: skip`.

- [ ] **Step 1: Failing native test — the scan**

In `mention_overlay.rs`:

```rust
    #[test]
    fn scan_finds_mentions_at_any_depth_with_targets() {
        // Build a doc: paragraph(text) + paragraph(DocMention doc_id=a),
        // list > item > paragraph(DocMention doc_id=b, target_block_id=x).
        // Assert two MentionRefs, correct doc_ids, target Some("x")/None,
        // node_block_id == each node's blockId attr.
    }
```

(Construct nodes via `Node::element_with_attrs` exactly as model.rs tests do.) RED first.

- [ ] **Step 2: Implement scan + state types + overlay component**

`scan_doc_mentions`: recursive walk matching `Node::Element { node_type: NodeType::DocMention, attrs, .. }` (mirror `extract_outline`'s destructure). The component/logic:

```rust
/// Per-viewer mention state — NEVER persisted (spec §2): access differs
/// per viewer, so live/dangling/missing exists only in this session.
```

- An Effect gated on the first `Some` of `editor_state` (and re-run on a `mentions_refresh` tick if one is ever added): scan → dedupe targets → `spawn_local(resolve_mentions(...))` → build the state map: `status != "ok"` ⇒ `Missing`; `ok` + target present + `block_found == false` ⇒ `Dangling{title}`; else `Live{title, snippet}` (empty snippet stays Live — `blockFound` is the signal).
- A second Effect on `(editor_state, mention_states)`: query `document` for `span.doc-mention[data-node-block-id]`, and per element: Missing → add class `doc-mention-missing`, set text to icon + `t!("doc-mention-missing")`, set `title` attr tooltip; Dangling → add `doc-mention-dangling`, tooltip `t!("doc-block-link-missing")` (reuse Plan 1's key), text stays title; Live → ensure classes absent and, when the fresh title/snippet differ from the rendered text, update the text content. (DOM-only mutation — the next editor re-render rebuilds from attrs and this Effect re-applies; that flicker-tolerance matches comment_highlights.)
- Editable-session attr refresh: when the doc is editable (**find the signal document.rs uses for read-only/lock state — the trash/lock banners around ~2632 read it**) and a Live resolve differs from cached attrs, build attr-update transactions (a `commands::update_doc_mention_attrs(state, node_block_id, title, snippet)` helper — same walk as Task 4's convert, `set_meta("history","skip")`) and dispatch through the standard path.

- [ ] **Step 3: i18n (all six catalogs)**

`en-US`: `doc-mention-missing = Missing document`
`es`: `Documento no disponible` — `it`: `Documento mancante` — `fr`: `Document introuvable` — `de`: `Fehlendes Dokument` — `ar`: `مستند مفقود`

- [ ] **Step 4: Wire into `document.rs`**

Mount/invoke the overlay beside the comment-highlights wiring, feeding it `editor_state` and the dispatch/apply path used by other outside-the-editor mutations (**mirror how comment_highlights receives its inputs**).

- [ ] **Step 5: GREEN + builds + commit**

Run: `cd frontend && cargo test && cargo check && cargo build --target wasm32-unknown-unknown`

```bash
git add frontend/src/components/mention_overlay.rs frontend/src/components/mod.rs frontend/src/pages/document.rs frontend/style/main.css frontend/locales/en-US/main.ftl frontend/locales/ar/main.ftl frontend/locales/es/main.ftl frontend/locales/it/main.ftl frontend/locales/fr/main.ftl frontend/locales/de/main.ftl
git commit -m "feat(editor): mention refresh-on-open + per-viewer degradation overlay"
```

---

## Task 6: Manual verification sweep (no code)

Via the `verify` skill (local stack), extending `scripts/frontend-doctor/probe-block-links.mjs` or a new probe where scriptable:

- [ ] **Paste matrix (the spec §9 S4 matrix, UI-observable cells):**
  1. Paste own-doc URL (no fragment) → 📄 document mention with live title; ONE undo restores the raw URL text.
  2. Paste own-doc URL with valid `#b=` → ⚓ anchor mention with snippet.
  3. Paste URL with dangling `#b=` → document mention + dashed indicator + tooltip.
  4. Paste another (inaccessible) user's doc URL → stays a plain URL, no error shown.
  5. Paste garbage/foreign URLs and URL-in-sentence → untouched by conversion.
- [ ] **Element behavior:** click navigates (anchor → scrolls to block via Plan 1's `#b=` path); Backspace/Delete removes the whole chip; caret steps over it as one position; copy→paste the chip round-trips; copy→paste into a plain-text editor yields the `<a>`/plain link.
- [ ] **Context menu:** Copy Original URL yields the pasted URL; Convert to Plain Link leaves linked title text; both entries absent when right-clicking normal text.
- [ ] **Degradation:** rename the target doc in another tab → reopen → title refreshes; trash the target doc → reopen → grayed "Missing document", click inert, Copy Original URL still works.
- [ ] **Cross-locale:** menu entries + missing label localized.
- [ ] Record outcomes; failures become findings, not ad-hoc patches.

---

## Self-Review Notes

- **Spec coverage (S3–S4):** element in both schemas + states + atom sweep (T1–T2), paste ladder a/b/c + single-undo + convert-back (T3–T4), refresh-on-open + per-viewer ephemeral degradation (T5), full matrix via native parser/ladder tests + backend matrix (Plan 1) + manual UI matrix (T6). Serialization layering (§7): clipboard `<a>` (T2) + export markdown/HTML (T1).
- **Decisions recorded in-plan:** foreign hash on a doc URL rejects the whole conversion (conservative, pinned by test); in-editor DOM uses `data-block-id-target` (avoids colliding with the pervasive `data-block-id` = own-block convention) while clipboard/export keep spec §7's `data-block-id`; plain click (not Ctrl) activates the chip.
- **Known risks flagged to implementers:** exact Transaction/Slice/meta APIs (anchor to `insert_user_mention`), view-hook plumbing for the pending-paste signal (anchor to `pending_ctx_cmd`/embed flow), position semantics of `insert_text`, wasm-gated clipboard tests. Each task says "follow the precedent, not the sketch" at those points.
- **Type consistency:** attr names (`url`/`doc_id`/`target_block_id`/`title`/`snippet`), `MentionResolveResult` field shapes (camelCase wire), `history` meta values (`"merge"`/`"skip"`), and `MentionState` variants are used identically across T1–T5.
- **Carried from Plan 1:** empty-snippet⇒Live contract (T5 ladder), no per-batch dedup concern (T5 dedupes targets client-side before resolving), charset constant duplication (mention_url reuses the `[A-Za-z0-9_-]` rule — a shared-constant cleanup remains future work).
