# Quip Import — Phase 2a (Content & Blobs) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every inventoried Quip thread becomes a real OgreNotes document — faithful structure (headings, lists, checklists, tables, code, images) with inline marks preserved, images side-loaded to S3 and rendered durably, Quip's original timestamps and multi-folder membership kept — checkpointed per thread so a crash resumes without re-importing.

**Architecture:** A new Quip-specific walker in `crates/collab/src/import_quip.rs` (parse → intermediate blocks → materialize, mirroring `import_docx`) converts Quip's `/2` HTML into a `yrs::Doc`, minting a stable `blockId` per block and capturing Quip's section ids into a `SECMAP`. The worker's content pass drives it per thread: fetch HTML → stage to S3 → walk → fetch/store blobs → persist the document via the shared creation path → write `SECMAP#`/`UNRESOLVED#` rows → checkpoint `ContentDone`. Intra-import links are recorded as `UNRESOLVED#` edges with placeholder mentions; **resolving them is Phase 2b**.

**Tech Stack:** Rust (`html5ever` + `markup5ever_rcdom` + `ammonia` already in `crates/collab`; `yrs` XML fragments; aws-sdk-s3/dynamodb), Leptos 0.7 CSR frontend.

## Global Constraints

- **The Quip token never leaves the `TokenStore`.** It must never appear in a job envelope, a DynamoDB row, an S3 object, a log line, an error message, or any `Debug` output. Unchanged from Phase 1 — the content pass re-reads it from the store like the inventory pass does.
- **All writes go through the normal document creation path.** Documents are created via the shared `persist_imported_document` helper (`crates/api/src/worker_mode.rs:551`) → `DocRepo::create` + `FolderRepo::add_child`. Do not hand-roll DynamoDB document writes.
- **Per-thread transactionality + resumability.** A thread either reaches `ContentDone` (document created, `SECMAP` written) or its `THREAD#` row stays short of it and is retried. A re-run must skip threads already `ContentDone` and must never create a duplicate document for one.
- **Preserve Quip provenance.** `DocumentMeta.created_at`/`updated_at` carry Quip's original timestamps (both are plain `i64` fields the caller sets — `persist_imported_document` currently hardcodes `now_usec()`). Multi-folder threads use `folder_id` = first folder ∪ `additional_folder_ids` = the rest.
- **`blockId`s are minted by the walker.** Format: exactly 10 chars from `[A-Za-z0-9]` (matching `frontend/src/editor/model.rs::generate_block_id`). `crates/api/src/routes/comments.rs:410` validates client blockIds as alphanumeric, 4–32 chars — stay inside that.
- **Raw `String` identifiers throughout.** No newtypes.
- **Search indexing is deliberately deferred** — the prod worker is a separate process and cannot write the API's local Tantivy index. Tracked in **issue #138** for all worker-created documents. Do not attempt to index from the worker in this phase.
- **Schema containment rules are load-bearing** (`crates/collab/src/schema.rs:210-286`). Notably `ListItem`/`TaskItem` accept only `Paragraph | BulletList | OrderedList | TaskList | Blockquote | CodeBlock` — **not `Image`, `Table`, or `HorizontalRule`**; `TableCell`/`TableHeader` accept blocks but **not nested `Table` and not `Image`**. The walker must never emit an invalid tree.

---

## Design decisions taken (flag at review)

1. **The walker lives in `crates/collab/src/import_quip.rs`, not `crates/quip-import`.** The design doc contradicts itself (one bullet says each). `collab` wins: the walker needs `NodeType`/`MarkType`/containment internals and sits beside `from_docx`/`from_html`/`from_xlsx`. `crates/quip-import` stays the API-client + orchestration crate and gains no `collab` dependency.

2. **`Image.src` stores a stable blob reference, resolved to a presigned URL at render time — not a presigned URL.** Today `Image.src` holds a **4-hour** presigned GET URL baked into the CRDT (`crates/api/src/routes/documents.rs:3056`), so editor-inserted images break after 4h. A *server* indirection route can't work: auth is Bearer-header-only (`crates/api/src/middleware/auth.rs:87`) and `<img src>` cannot send that header. So the frontend resolves a stable reference at render time using the token it already holds. This fixes the pre-existing expiry bug for editor uploads too. Legacy absolute-URL `src` values keep working unchanged (backward compatible).

3. **Chat threads are skipped, and the skip reason lives in the report, not the `THREAD#` row.** Phase 1 shipped `ThreadState` as unit variants (`Pending | ContentDone | CommentsDone | Skipped`) rather than the design's `Skipped{reason}`. Rather than change a shipped enum mid-feature, the reason is recorded in the accumulating report. Revisit if Phase 5's report needs richer per-thread detail.

4. **Embedded grids follow the design's documented interim:** import the grid as its own `DocType::Spreadsheet` document and leave a `DocMention` inline where it was embedded. Issue **#133** (inline grid block) is still open; migrate when it ships.

---

## File Structure

**Created:**
- `crates/collab/src/import_quip.rs` — the walker: `parse_quip` (HTML → `Vec<QuipBlock>`) + `materialize` (blocks → `QuipDocument`), plus `blob_ref` helpers.
- `crates/collab/tests/fixtures/quip/*.html` — walker fixtures (headings, lists, checklists, tables, code+lang, images, marks, sections, links).
- `crates/api/tests/test_quip_content_worker.rs` — integration tests for the content pass.

**Modified:**
- `crates/collab/src/lib.rs` — `pub mod import_quip;`
- `crates/quip-import/src/client.rs` — add `thread_html`, `blob`.
- `crates/storage/src/models/import_inventory.rs` — add `SecMapRow`, `UnresolvedRow`, `PendingLink`.
- `crates/storage/src/repo/import_repo.rs` — add `put_secmap`, `get_secmap`, `put_unresolved`, `list_unresolved`, `set_thread_content_done`, `set_thread_skipped`.
- `crates/api/src/worker_mode.rs` — extend `persist_imported_document` (timestamps, doc_type, multi-folder), add `run_content_pass` + per-thread `import_one_thread`, call it after inventory.
- `frontend/src/editor/view.rs` + `frontend/src/components/editor_component.rs` — resolve blob-reference `src` at render; write references on upload.
- `frontend/src/components/quip_import/mod.rs` + `frontend/locales/*/main.ftl` — content-phase progress copy.

---

## Task 1: Walker — block structure, containment, and blockId minting

**Files:**
- Create: `crates/collab/src/import_quip.rs`
- Modify: `crates/collab/src/lib.rs` (add `pub mod import_quip;`)
- Test: unit tests inside `import_quip.rs`

**Interfaces:**
- Consumes: `crate::schema::{NodeType, MarkType}`, `ammonia`, `html5ever`, `markup5ever_rcdom::RcDom`, `yrs::{Doc, XmlElementPrelim, XmlTextPrelim}`. Mirror the 3-stage pipeline in `crates/collab/src/import.rs:326-356` (sanitize → parse → walk) and the split-parse/materialize rationale in `crates/collab/src/import_docx.rs:32-35`.
- Produces (Task 2 extends these; Task 6 consumes them):
  ```rust
  pub struct QuipDocument {
      pub doc: yrs::Doc,
      /// Quip section id -> minted blockId. Ordered by document order.
      pub sections: Vec<(String, String)>,
      /// Images referenced by the source, to be side-loaded by the caller.
      pub images: Vec<QuipImageRef>,
      /// Intra-Quip links needing Phase-2b back-patch.
      pub pending_links: Vec<QuipPendingLink>,
  }
  pub struct QuipImageRef { pub block_id: String, pub src: String, pub alt: String }
  pub struct QuipPendingLink {
      pub source_block_id: String,
      pub target_quip_thread_id: String,
      pub target_quip_section_id: Option<String>,
  }
  pub fn from_quip_html(html: &str) -> QuipDocument;
  pub fn new_block_id() -> String;   // 10 chars [A-Za-z0-9]
  ```
  Task 1 populates `doc` only; `sections`/`images`/`pending_links` are added in Task 2 (return them empty here).

- [ ] **Step 1: Write the failing blockId test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_ids_are_ten_alphanumeric_chars_and_unique() {
        let a = new_block_id();
        let b = new_block_id();
        assert_eq!(a.len(), 10, "blockId must be 10 chars: {a}");
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()), "alphanumeric only: {a}");
        assert_ne!(a, b, "ids must differ");
    }
}
```

- [ ] **Step 2: Run it — expect FAIL** (`new_block_id` undefined)

Run: `cargo test -p ogrenotes-collab import_quip::tests::block_ids`
Expected: FAIL to compile — "cannot find function `new_block_id`".

- [ ] **Step 3: Implement `new_block_id` + the sanitize/parse skeleton**

```rust
//! Quip `/2` HTML -> OgreNotes document. A Quip-specific walker, deliberately
//! NOT `import::from_html` (which drops inline marks and has no tables,
//! images, task lists, or blockIds — see `import.rs:14-19,31`).
//!
//! Split like `import_docx`: `parse_quip` produces a `Vec<QuipBlock>` so the
//! HTML state machine never holds a live yrs transaction, and the two halves
//! test independently.

use crate::schema::NodeType;

const BLOCK_ID_LEN: usize = 10;
const BLOCK_ID_ALPHABET: [char; 62] = [
    'A','B','C','D','E','F','G','H','I','J','K','L','M','N','O','P','Q','R','S','T','U','V','W','X','Y','Z',
    'a','b','c','d','e','f','g','h','i','j','k','l','m','n','o','p','q','r','s','t','u','v','w','x','y','z',
    '0','1','2','3','4','5','6','7','8','9',
];

/// Mint a stable block id. Matches the frontend's `generate_block_id`
/// (10 chars, `[A-Za-z0-9]`) and stays inside the 4-32 alphanumeric range
/// `routes::comments` validates for client-supplied ids.
pub fn new_block_id() -> String {
    nanoid::nanoid!(BLOCK_ID_LEN, &BLOCK_ID_ALPHABET)
}
```

Add `nanoid` to `crates/collab/Cargo.toml` if absent (it is already a workspace dependency used by `crates/api`).

- [ ] **Step 4: Run it — expect PASS**

Run: `cargo test -p ogrenotes-collab import_quip::tests::block_ids`
Expected: PASS.

- [ ] **Step 5: Write failing block-structure tests**

```rust
    fn blocks(html: &str) -> Vec<QuipBlock> { parse_quip(html) }

    #[test]
    fn headings_lists_code_and_hr_parse() {
        let b = blocks("<h2>Title</h2><ul><li>one</li><li>two</li></ul>\
                        <pre data-language=\"rust\">fn x(){}</pre><hr>");
        assert!(matches!(b[0], QuipBlock::Heading { level: 2, .. }));
        assert!(matches!(b[1], QuipBlock::List { ordered: false, .. }));
        assert!(matches!(b[2], QuipBlock::Code { ref language, .. } if language == "rust"));
        assert!(matches!(b[3], QuipBlock::Rule));
    }

    #[test]
    fn checklist_items_carry_checked_state() {
        // Quip renders checklists as list items with a checkbox input.
        let b = blocks("<ul><li><input type=\"checkbox\" checked>done</li>\
                        <li><input type=\"checkbox\">todo</li></ul>");
        let QuipBlock::List { ordered: _, task, items } = &b[0] else { panic!("expected list") };
        assert!(*task, "a list whose items carry checkboxes is a task list");
        assert_eq!(items[0].checked, Some(true));
        assert_eq!(items[1].checked, Some(false));
    }

    #[test]
    fn table_rows_and_header_cells_parse() {
        let b = blocks("<table><tr><th>H</th></tr><tr><td>C</td></tr></table>");
        let QuipBlock::Table { rows } = &b[0] else { panic!("expected table") };
        assert_eq!(rows.len(), 2);
        assert!(rows[0].cells[0].header, "th -> header cell");
        assert!(!rows[1].cells[0].header);
    }

    #[test]
    fn materialized_tree_obeys_schema_containment() {
        // An image inside a list item is illegal per schema::valid_children;
        // it must be hoisted to the top level rather than emitted invalidly.
        let out = from_quip_html("<ul><li>text<img src=\"http://x/i.png\"></li></ul>");
        let xml = crate::export::to_html(&out.doc);
        assert!(xml.contains("<ul") || xml.contains("bullet_list"), "list survives: {xml}");
        assert!(!xml.contains("<li><img"), "image must not be a direct list-item child: {xml}");
    }

    #[test]
    fn every_block_gets_a_block_id() {
        let out = from_quip_html("<p>a</p><h1>b</h1>");
        let xml = crate::export::to_html(&out.doc);
        assert_eq!(xml.matches("blockId").count(), 0, "export strips ids; check the doc instead");
        // Walk the fragment directly:
        let txn = out.doc.transact();
        let frag = crate::document::OgreDoc::get_content_fragment(&txn).expect("content fragment");
        assert!(frag.len(&txn) >= 2, "two top-level blocks");
    }
```

> Adapt the last two assertions to whatever `export`/fragment API reads cleanest — the requirement is: (a) no invalid parent/child pair is ever emitted, (b) every element except the root carries a non-empty `blockId` attribute. If `export::to_html` does not surface `blockId`, walk the `XmlFragmentRef` and assert `get_attribute(&txn, "blockId").is_some()` on each child.

- [ ] **Step 6: Run — expect FAIL** (`QuipBlock`, `parse_quip`, `from_quip_html` undefined)

Run: `cargo test -p ogrenotes-collab import_quip::`
Expected: FAIL to compile.

- [ ] **Step 7: Implement `QuipBlock`, `parse_quip`, and `materialize`**

```rust
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuipCell { pub header: bool, pub blocks: Vec<QuipBlock> }
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuipRow { pub cells: Vec<QuipCell> }
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuipItem { pub checked: Option<bool>, pub blocks: Vec<QuipBlock> }

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum QuipBlock {
    Para { section_id: Option<String>, spans: Vec<Span> },
    Heading { level: u8, section_id: Option<String>, spans: Vec<Span> },
    List { ordered: bool, task: bool, items: Vec<QuipItem> },
    Quote { blocks: Vec<QuipBlock> },
    Code { language: String, text: String },
    Rule,
    Table { rows: Vec<QuipRow> },
    Image { src: String, alt: String },
}
```

`Span` is defined in Task 2 (marks); for Task 1 a `Span` may carry text only — Task 2 adds the mark set. Keep the field name stable so Task 2 is additive.

`parse_quip(html) -> Vec<QuipBlock>`:
1. Sanitize with an **extended** ammonia allowlist — the base list in `import.rs:363-377` omits exactly what Quip needs. Allow additionally: `table, thead, tbody, tr, th, td, img, input, br, code, s, u, sub, sup, span, div`. Keep `script`/`iframe`/`style`/`form` out. Preserve the attributes the walker reads: `id`, `href`, `src`, `alt`, `title`, `type`, `checked`, `colspan`, `rowspan`, `data-language`, `class` (use `ammonia::Builder::generic_attributes` / `tag_attributes`).
2. Parse with `html5ever::parse_document(RcDom::default(), ...)` exactly as `import.rs:334-343`.
3. Walk the RcDom recursively into `Vec<QuipBlock>`, following `import.rs:379-462`'s conventions: descend `html`/`head`/`body` transparently; unknown tags are transparent passthrough; drop pure-whitespace text nodes; wrap bare fragment-level text in a `Para`.
   - `<li>` containing an `<input type=checkbox>` sets `checked`; a list with any checked-bearing item becomes `task: true`.
   - `<pre>`: language from `data-language`, else a `language-x`/`lang-x` class on the `<pre>` or its inner `<code>`, else `""`.
   - Capture the element's `id` attribute into `section_id` for block-level nodes (used by Task 2's SECMAP).

`materialize(&[QuipBlock]) -> yrs::Doc`: mirror `import_docx.rs:283-342`. Use the `XmlOpenable` insertion helper pattern from `import.rs:229-261`. For every element inserted, set `blockId`:

```rust
fn insert_block(
    txn: &mut yrs::TransactionMut<'_>,
    parent: &XmlOpenable<'_>,
    node: NodeType,
) -> XmlElementRef {
    let el = insert_at_end(txn, parent, node);          // same helper shape as import.rs:246
    el.insert_attribute(txn, "blockId", new_block_id());
    el
}
```

**Containment enforcement:** before inserting a child, consult `NodeType::valid_children(parent)`. If the pair is invalid (an `Image`, `Table`, or `Rule` inside a `ListItem`/`TaskItem`, or an `Image`/nested `Table` inside a cell), **hoist** the offending block: close the list/cell context and emit the block at the nearest ancestor that accepts it, preserving document order. Record nothing — hoisting is lossless for content, only nesting changes.

- [ ] **Step 8: Run — expect PASS**

Run: `cargo test -p ogrenotes-collab import_quip::`
Expected: PASS (structure, checklist, table, containment, blockId).

- [ ] **Step 9: Commit**

```bash
git add crates/collab/src/import_quip.rs crates/collab/src/lib.rs crates/collab/Cargo.toml
git commit -m "feat(collab): Quip HTML walker — block structure, containment, blockIds"
```

---

## Task 2: Walker — inline marks, section map, images, and intra-Quip links

**Files:**
- Modify: `crates/collab/src/import_quip.rs`
- Test: unit tests in the same file

**Interfaces:**
- Consumes: Task 1's `QuipBlock`/`parse_quip`/`materialize`; `crate::schema::MarkType`.
- Produces: fully-populated `QuipDocument { doc, sections, images, pending_links }` (types declared in Task 1).

**Mark storage format (load-bearing — verified in `crates/collab/src/export.rs:752-776`):** boolean marks are `Any::Bool(true)` under the mark's attr name; the link mark is a **JSON string** payload:
```rust
attrs.insert(Arc::from("link"), Any::String(Arc::from(r#"{"href":"https://x/y"}"#)));
```
Attr names come from `MarkType::attr_name()` (`schema.rs:311`). The decoder mirror is `crates/collab/src/diff.rs::attrs_to_marks`.

- [ ] **Step 1: Write the failing marks test**

```rust
    #[test]
    fn inline_marks_survive_the_round_trip() {
        let out = from_quip_html("<p><b>bold</b> <i>it</i> <code>c</code> \
                                  <a href=\"https://ok.example/x\">link</a></p>");
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("<strong>bold</strong>") || html.contains("<b>bold</b>"), "{html}");
        assert!(html.contains("<em>it</em>") || html.contains("<i>it</i>"), "{html}");
        assert!(html.contains("<code>c</code>"), "{html}");
        assert!(html.contains("https://ok.example/x"), "link href preserved: {html}");
    }

    #[test]
    fn section_ids_map_to_minted_block_ids() {
        let out = from_quip_html("<p id=\"sec-abc\">one</p><h1 id=\"sec-def\">two</h1>");
        let ids: Vec<&str> = out.sections.iter().map(|(q, _)| q.as_str()).collect();
        assert_eq!(ids, vec!["sec-abc", "sec-def"]);
        for (_, block_id) in &out.sections {
            assert_eq!(block_id.len(), 10, "maps to a minted blockId");
        }
    }

    #[test]
    fn images_are_collected_with_their_block_ids() {
        let out = from_quip_html("<p>x</p><img src=\"/blob/t1/b9\" alt=\"pic\">");
        assert_eq!(out.images.len(), 1);
        assert_eq!(out.images[0].src, "/blob/t1/b9");
        assert_eq!(out.images[0].alt, "pic");
        assert_eq!(out.images[0].block_id.len(), 10);
    }

    #[test]
    fn intra_quip_links_become_pending_not_plain_links() {
        let out = from_quip_html(
            "<p><a href=\"https://example.quip.com/AbCd1234/Some-Doc\">doc</a> \
             <a href=\"https://elsewhere.example/page\">ext</a></p>");
        assert_eq!(out.pending_links.len(), 1, "only the quip link is pending");
        assert_eq!(out.pending_links[0].target_quip_thread_id, "AbCd1234");
        assert!(out.pending_links[0].target_quip_section_id.is_none());
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("elsewhere.example/page"), "external link passes through: {html}");
    }

    #[test]
    fn intra_quip_link_with_fragment_records_the_section() {
        let out = from_quip_html(
            "<p><a href=\"https://example.quip.com/AbCd1234/Doc#sec-77\">x</a></p>");
        assert_eq!(out.pending_links[0].target_quip_section_id.as_deref(), Some("sec-77"));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p ogrenotes-collab import_quip::`
Expected: FAIL — marks dropped / `sections` empty / `images` empty / `pending_links` empty.

- [ ] **Step 3: Implement spans + marks**

Extend `Span` to carry a mark set and apply it when materializing text:

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Marks {
    pub bold: bool, pub italic: bool, pub underline: bool,
    pub strike: bool, pub code: bool, pub link: Option<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Span { pub text: String, pub marks: Marks }
```

Inline walk: descend inline elements accumulating marks — `b|strong` → bold, `i|em` → italic, `u` → underline, `s|del|strike` → strike, `code` → code, `a[href]` → `link`. Emit one `Span` per contiguous run with the same mark set.

Materializing a run into a yrs XmlText: insert the text, then `format` the range with the mark attrs:

```rust
fn apply_marks(txn: &mut yrs::TransactionMut<'_>, text: &yrs::XmlTextRef, start: u32, len: u32, m: &Marks) {
    if len == 0 { return; }
    let mut attrs = yrs::types::Attrs::new();
    if m.bold      { attrs.insert(std::sync::Arc::from(MarkType::Bold.attr_name()),      yrs::Any::Bool(true)); }
    if m.italic    { attrs.insert(std::sync::Arc::from(MarkType::Italic.attr_name()),    yrs::Any::Bool(true)); }
    if m.underline { attrs.insert(std::sync::Arc::from(MarkType::Underline.attr_name()), yrs::Any::Bool(true)); }
    if m.strike    { attrs.insert(std::sync::Arc::from(MarkType::Strike.attr_name()),    yrs::Any::Bool(true)); }
    if m.code      { attrs.insert(std::sync::Arc::from(MarkType::Code.attr_name()),      yrs::Any::Bool(true)); }
    if let Some(href) = &m.link {
        let payload = serde_json::json!({ "href": href }).to_string();
        attrs.insert(std::sync::Arc::from(MarkType::Link.attr_name()), yrs::Any::String(payload.into()));
    }
    if !attrs.is_empty() { text.format(txn, start, len, attrs); }
}
```

> Match the exact `format`/insert API `export.rs:2796-2800` exercises. If `XmlTextRef` needs `insert_with_attributes`, use that instead — the requirement is that `export::to_html` round-trips the marks.

- [ ] **Step 4: Implement the section map**

While materializing, when a block carries `section_id`, push `(section_id, minted_block_id)` onto `sections` in document order.

- [ ] **Step 5: Implement image collection and link classification**

- **Images:** materialize an `Image` node with `alt` set and `src` left as the **raw Quip src** for now (Task 6 rewrites it to a blob reference after side-loading). Push `QuipImageRef { block_id, src, alt }`.
- **Links:** classify each `a[href]`:
  - Host contains `quip.com` (any subdomain) → an intra-Quip link. Extract the thread id as the first non-empty path segment (`https://<sub>.quip.com/<THREAD_ID>/<slug>`), and the fragment (if any) as `target_quip_section_id`. Emit a **placeholder `DocMention`** per the design (`doc_id` empty, plus a `pending_quip_thread` attr carrying the target thread id) and record a `QuipPendingLink { source_block_id, target_quip_thread_id, target_quip_section_id }`. The placeholder's own `blockId` is the `source_block_id`.
  - Anything else → a normal `Link` mark, passed through untouched.

```rust
fn quip_thread_from_url(href: &str) -> Option<(String, Option<String>)> {
    let url = url::Url::parse(href).ok()?;
    if !url.host_str()?.ends_with("quip.com") { return None; }
    let seg = url.path_segments()?.find(|s| !s.is_empty())?.to_string();
    let frag = url.fragment().map(|f| f.to_string());
    Some((seg, frag))
}
```

Add the `url` crate if `crates/collab` lacks it (it is already a workspace dependency).

- [ ] **Step 6: Run — expect PASS**

Run: `cargo test -p ogrenotes-collab import_quip::`
Expected: PASS (marks, sections, images, both link cases).

- [ ] **Step 7: Add the fixture matrix**

Create `crates/collab/tests/fixtures/quip/` with one file per feature (`headings.html`, `lists.html`, `checklists.html`, `tables.html`, `code.html`, `images.html`, `marks.html`, `sections.html`, `links.html`) plus a `kitchen_sink.html`. Add a test that walks each fixture and asserts it converts without panicking and produces a non-empty doc — the per-feature assertions already live in the unit tests above. This is the design's required "converter fixtures" matrix.

- [ ] **Step 8: Run the full collab suite — expect PASS**

Run: `cargo test -p ogrenotes-collab`
Expected: PASS, output pristine. Existing `import.rs`/`import_docx.rs` tests must be untouched and still green.

- [ ] **Step 9: Commit**

```bash
git add crates/collab/src/import_quip.rs crates/collab/tests/fixtures/quip
git commit -m "feat(collab): Quip walker — inline marks, section map, images, pending links"
```

---

## Task 3: Durable image references (fixes the 4h presigned-URL expiry)

**Files:**
- Modify: `crates/collab/src/import_quip.rs` (reference format helper)
- Modify: `frontend/src/editor/view.rs` (resolve at render)
- Modify: `frontend/src/components/editor_component.rs` (write a reference on upload)
- Test: unit test for the reference parser; wasm32 build for the frontend

**Interfaces:**
- Produces:
  ```rust
  /// `Image.src` form for a blob owned by this workspace:
  ///   ogre-blob:<blob_id>/<url-encoded key>
  /// Anything not matching this prefix is a legacy/external absolute URL and
  /// must be used verbatim (backward compatible).
  pub fn blob_ref(blob_id: &str, key: &str) -> String;
  pub fn parse_blob_ref(src: &str) -> Option<(String /*blob_id*/, String /*key*/)>;
  ```

**Why:** `Image.src` currently stores a 14400-second presigned GET URL (`crates/api/src/routes/documents.rs:3056`), so images break 4 hours after insertion. A server-side indirection route cannot serve `<img>` because auth is Bearer-header-only (`crates/api/src/middleware/auth.rs:87`). Resolving client-side keeps a stable reference in the CRDT and uses the token the frontend already holds.

- [ ] **Step 1: Write the failing reference round-trip test** (in `import_quip.rs`)

```rust
    #[test]
    fn blob_ref_round_trips_and_ignores_absolute_urls() {
        let r = blob_ref("b1", "blobs/d1/b1/pic name.png");
        assert!(r.starts_with("ogre-blob:"), "{r}");
        let (id, key) = parse_blob_ref(&r).expect("parses");
        assert_eq!(id, "b1");
        assert_eq!(key, "blobs/d1/b1/pic name.png", "key survives encoding");
        assert!(parse_blob_ref("https://example.com/x.png").is_none(), "legacy URLs pass through");
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p ogrenotes-collab blob_ref`
Expected: FAIL — functions undefined.

- [ ] **Step 3: Implement `blob_ref` / `parse_blob_ref`**

Percent-encode the key so the reference is a single token with no ambiguity; decode on parse. Keep the prefix constant (`const BLOB_REF_PREFIX: &str = "ogre-blob:";`) exported for the frontend mirror.

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p ogrenotes-collab blob_ref`
Expected: PASS.

- [ ] **Step 5: Resolve references at render time (frontend)**

In `frontend/src/editor/view.rs` where an `Image` node's `src` is applied to the `<img>` element (`view.rs:1391-1393`): if `src` starts with `ogre-blob:`, parse out `(blob_id, key)`, call the existing `blobs::request_download_url(&doc_id, &blob_id, &key)` to get a fresh presigned URL, and set that on the element when it resolves (leave the element without a `src`, or with a placeholder, until then). Otherwise set `src` verbatim — legacy absolute URLs are unchanged.

Cache resolutions per `(blob_id, key)` for the page's lifetime so a doc with N images does not issue N requests per re-render.

- [ ] **Step 6: Write a reference on upload (frontend)**

In `frontend/src/components/editor_component.rs:1752-1755`, replace the presigned `download_url` with the stable reference:

```rust
let mut attrs = HashMap::new();
attrs.insert("src".to_string(), blob_ref(&upload.blob_id, &upload.key));
attrs.insert("alt".to_string(), filename);
```

Mirror `blob_ref`/`parse_blob_ref` on the frontend side (the frontend does not depend on `crates/collab` for this path — duplicate the two small functions and pin the shared prefix with a test asserting both produce the same string for the same input, in the spirit of the existing schema-duality CI test).

- [ ] **Step 7: Verify**

Run: `cargo test -p ogrenotes-collab` and `cd frontend && cargo build --target wasm32-unknown-unknown`
Expected: PASS / Finished clean. **This is a behavior change to a shipped path** — call it out in the task report so the reviewer weighs it deliberately.

- [ ] **Step 8: Commit**

```bash
git add crates/collab/src/import_quip.rs frontend/src/editor/view.rs frontend/src/components/editor_component.rs
git commit -m "fix(images): store durable blob references in Image.src, resolve at render"
```

---

## Task 4: Quip client — thread HTML and blob download

**Files:**
- Modify: `crates/quip-import/src/client.rs`
- Test: wiremock tests in the same file

**Interfaces:**
- Consumes: the existing `QuipClient` throttle → `bearer_auth` → `observe_and_check` pipeline (mirror `folders`/`threads` exactly).
- Produces:
  ```rust
  /// `GET /2/threads/{id}/html` — the section-id-bearing HTML.
  pub async fn thread_html(&self, t: &QuipToken, thread_id: &str) -> Result<String, QuipError>;
  /// `GET /1/blob/{thread_id}/{blob_id}` — raw bytes.
  pub async fn blob(&self, t: &QuipToken, thread_id: &str, blob_id: &str) -> Result<Vec<u8>, QuipError>;
  ```

- [ ] **Step 1: Write failing wiremock tests**

```rust
    #[tokio::test]
    async fn thread_html_fetches_the_v2_html_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/2/threads/t1/html"))
            .and(header("authorization", "Bearer tok-h"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<p id=\"s1\">hi</p>"))
            .mount(&server).await;
        let c = QuipClient::new(Some(server.uri()));
        let html = c.thread_html(&QuipToken::new("tok-h".into()), "t1").await.unwrap();
        assert!(html.contains("id=\"s1\""));
    }

    #[tokio::test]
    async fn blob_fetches_raw_bytes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/1/blob/t1/b9"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8, 2, 3]))
            .mount(&server).await;
        let c = QuipClient::new(Some(server.uri()));
        let bytes = c.blob(&QuipToken::new("tok-b".into()), "t1", "b9").await.unwrap();
        assert_eq!(bytes, vec![1u8, 2, 3]);
    }

    #[tokio::test]
    async fn thread_html_401_maps_to_unauthorized_without_leaking_the_token() {
        let server = MockServer::start().await;
        Mock::given(path("/2/threads/t1/html"))
            .respond_with(ResponseTemplate::new(401)).mount(&server).await;
        let c = QuipClient::new(Some(server.uri()));
        let e = c.thread_html(&QuipToken::new("SEEKRET".into()), "t1").await.unwrap_err();
        assert!(matches!(e, QuipError::Unauthorized));
        assert!(!format!("{e}").contains("SEEKRET"));
    }
```

- [ ] **Step 2: Run — expect FAIL** (methods undefined)

Run: `cargo test -p ogrenotes-quip-import thread_html`
Expected: FAIL to compile.

- [ ] **Step 3: Implement both methods**

Mirror `folders`/`threads`: `self.throttle.acquire().await` → `self.http.get(format!("{}/2/threads/{thread_id}/html", self.base))` → `.bearer_auth(t.expose())` → `self.observe_and_check(resp).await?`. `thread_html` reads `.text()`; `blob` reads `.bytes()` into a `Vec<u8>`. Add a `Checked::text_body`/`bytes_body` alongside the existing `json_body` so the header-before-body ordering contract is preserved.

Guard blob size: refuse bodies over a constant cap (e.g. `MAX_BLOB_BYTES: usize = 32 * 1024 * 1024`) so one pathological attachment cannot exhaust worker memory, mirroring `import_pdf.rs:36`'s posture.

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p ogrenotes-quip-import`
Expected: PASS (all three new + the existing 22).

- [ ] **Step 5: Commit**

```bash
git add crates/quip-import/src/client.rs
git commit -m "feat(quip-import): /2 thread HTML + blob download endpoints"
```

---

## Task 5: Storage — SECMAP, UNRESOLVED, and thread content checkpoints

**Files:**
- Modify: `crates/storage/src/models/import_inventory.rs`
- Modify: `crates/storage/src/repo/import_repo.rs`
- Test: unit tests for item mapping + a live-Dynamo test in `crates/storage/tests/test_import_repo.rs`

**Interfaces:**
- Consumes: the Phase-1 `ImportRepo` conventions (hand-built `AttributeValue` items, `get_s`/`get_n`, `DynamoClient::{put_item, query, update_item, update_item_conditional}`).
- Produces:
  ```rust
  pub struct SecMapRow { pub quip_thread_id: String, pub chunk: u32, pub owner_id: String,
                         pub entries: Vec<(String /*quip section id*/, String /*block id*/)> }
  pub struct UnresolvedRow { pub source_quip_thread_id: String, pub owner_id: String,
                             pub links: Vec<PendingLinkItem> }
  pub struct PendingLinkItem { pub source_block_id: String,
                               pub target_quip_thread_id: String,
                               pub target_quip_section_id: Option<String> }

  impl ImportRepo {
      pub async fn put_secmap(&self, import_id: &str, row: &SecMapRow) -> Result<(), RepoError>;
      pub async fn get_secmap(&self, import_id: &str, quip_thread_id: &str)
          -> Result<Vec<(String, String)>, RepoError>;           // concatenates chunks in order
      pub async fn put_unresolved(&self, import_id: &str, row: &UnresolvedRow) -> Result<(), RepoError>;
      pub async fn list_unresolved(&self, import_id: &str) -> Result<Vec<UnresolvedRow>, RepoError>;
      pub async fn set_thread_content_done(&self, import_id: &str, quip_thread_id: &str,
          ogre_doc_id: &str, content_s3_key: &str) -> Result<(), RepoError>;
      pub async fn set_thread_skipped(&self, import_id: &str, quip_thread_id: &str) -> Result<(), RepoError>;
  }
  ```
  SKs: `SECMAP#<thread>#<chunk>`, `UNRESOLVED#<thread>`.

- [ ] **Step 1: Write failing item round-trip tests** (in `import_repo.rs` tests)

```rust
    #[test]
    fn secmap_row_round_trips_and_has_no_token() {
        let r = SecMapRow { quip_thread_id: "t1".into(), chunk: 0, owner_id: "u1".into(),
            entries: vec![("s1".into(), "b1".into()), ("s2".into(), "b2".into())] };
        let item = secmap_to_item(&r);
        assert!(!item.contains_key("token") && !item.contains_key("secret"));
        assert_eq!(secmap_from_item(&item).expect("from_item"), r);
    }

    #[test]
    fn unresolved_row_round_trips_with_optional_section() {
        let r = UnresolvedRow { source_quip_thread_id: "t1".into(), owner_id: "u1".into(),
            links: vec![
                PendingLinkItem { source_block_id: "b1".into(),
                    target_quip_thread_id: "t2".into(), target_quip_section_id: Some("s9".into()) },
                PendingLinkItem { source_block_id: "b2".into(),
                    target_quip_thread_id: "t3".into(), target_quip_section_id: None },
            ] };
        assert_eq!(unresolved_from_item(&unresolved_to_item(&r)).expect("from_item"), r);
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p ogrenotes-storage import_repo::tests::secmap`
Expected: FAIL — types/mappers undefined.

- [ ] **Step 3: Implement models, mappers, and repo methods**

Follow the Phase-1 style exactly (`folder_to_item`/`thread_to_item` are the template). Store `entries`/`links` as `AttributeValue::L` of `M` (maps). Sparse-omit `target_quip_section_id` when `None`.

**Chunking:** DynamoDB items cap at 400 KB. `put_secmap` is called per chunk by the caller; add a `pub const SECMAP_CHUNK_ENTRIES: usize = 2_000;` constant here and have Task 6 split on it. `get_secmap` queries SK prefix `SECMAP#<thread>#`, sorts by chunk, and concatenates.

`set_thread_content_done` updates the `THREAD#` row: `SET #state = :state, ogre_doc_id = :doc, content_s3_key = :key` with `:state = "contentdone"`. `set_thread_skipped` sets `:state = "skipped"`. Both are plain `update_item` (the row already exists from Phase 1's inventory).

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p ogrenotes-storage import_repo::`
Expected: PASS.

- [ ] **Step 5: Add a live-Dynamo checkpoint test** (`crates/storage/tests/test_import_repo.rs`, same `require_infra!` harness as Phase 1)

```rust
#[tokio::test]
async fn content_checkpoint_advances_thread_and_secmap_chunks_concatenate() {
    // seed META + a Pending THREAD# row (reuse the Phase-1 helpers)
    // put_secmap chunk 0 and chunk 1, then get_secmap returns them concatenated in order
    // set_thread_content_done -> list_threads shows ContentDone + ogre_doc_id + content_s3_key
    // set_thread_content_done is idempotent: calling twice leaves one ContentDone row
}
```

- [ ] **Step 6: Run — expect PASS**

Run: `cargo test -p ogrenotes-storage --test test_import_repo`
Expected: PASS (infra up: `docker compose up -d`).

- [ ] **Step 7: Commit**

```bash
git add crates/storage/src/models/import_inventory.rs crates/storage/src/repo/import_repo.rs \
        crates/storage/tests/test_import_repo.rs
git commit -m "feat(storage): SECMAP/UNRESOLVED rows + per-thread content checkpoints"
```

---

## Task 6: Worker — the per-thread content pass

**Files:**
- Modify: `crates/api/src/worker_mode.rs`
- Test: `crates/api/tests/test_quip_content_worker.rs`

**Interfaces:**
- Consumes: Tasks 1–5 (`from_quip_html`, `blob_ref`, `thread_html`, `blob`, the new `ImportRepo` methods), plus Phase 1's `run_inventory` and lease helpers.
- Produces:
  ```rust
  /// Phase 2 content pass: convert every Pending thread into a document.
  /// Runs after inventory inside the same job; resumable — threads already
  /// ContentDone are skipped without refetching.
  async fn run_content_pass(ctx: &WorkerCtx, import_id: &str, owner_id: &str,
                            client: &QuipClient, token: &QuipToken) -> Result<(), String>;
  pub async fn import_one_thread(ctx: &WorkerCtx, import_id: &str, owner_id: &str,
                                 client: &QuipClient, token: &QuipToken,
                                 thread: &ThreadRow) -> Result<(), String>;  // pub for the test seam
  ```

**`persist_imported_document` must be extended** (`worker_mode.rs:551`) to accept what Quip needs — it currently hardcodes `DocType::Document`, `now_usec()` for both timestamps, and a single folder:
```rust
async fn persist_imported_document(
    doc_repo: &DocRepo, folder_repo: &FolderRepo,
    snapshot: &[u8], title: &str, owner_id: &str,
    folder_id: &str,
    doc_type: DocType,
    additional_folder_ids: &[String],
    created_at: i64, updated_at: i64,
) -> Result<String, String>;
```
Update the two existing callers (`execute_import_docx`, `execute_import_pdf`) to pass `DocType::Document`, `&[]`, `now_usec(), now_usec()` — a behavior-preserving change. Link the doc into every folder in `additional_folder_ids` too (`folder_repo.add_child` per folder), since `additional_folder_ids` alone does not create the child rows.

- [ ] **Step 1: Write failing integration tests**

```rust
mod common;

#[tokio::test]
async fn content_pass_creates_documents_with_quip_timestamps_and_folders() {
    common::require_infra!();
    // wiremock: /1/folders/, /1/threads/ (inventory) + /2/threads/{id}/html (content)
    // run the whole job (inventory then content)
    // assert: a document exists per non-chat thread; title matches; created_at/updated_at
    //         equal the Quip updated_usec (not "now"); folder_id = first_folder and
    //         additional_folder_ids covers the rest; THREAD# rows are ContentDone with ogre_doc_id.
}

#[tokio::test]
async fn content_pass_is_resumable_and_never_duplicates() {
    common::require_infra!();
    // run the content pass twice; assert exactly one document per thread and that
    // the second run performs no /2/html fetch for already-ContentDone threads
    // (assert via wiremock received_requests count).
}

#[tokio::test]
async fn images_are_sideloaded_to_s3_and_src_becomes_a_blob_reference() {
    common::require_infra!();
    // fixture HTML with <img src="/blob/t1/b9">; wiremock serves /1/blob/t1/b9 bytes.
    // assert: an object exists under blobs/{doc_id}/... in S3 and the persisted
    // snapshot's Image src starts with "ogre-blob:".
}

#[tokio::test]
async fn chat_threads_are_skipped_and_spreadsheets_become_spreadsheet_docs() {
    common::require_infra!();
    // inventory yields one chat + one spreadsheet thread; assert the chat THREAD#
    // is Skipped with no document, and the spreadsheet doc has DocType::Spreadsheet.
}

#[tokio::test]
async fn intra_quip_links_are_recorded_unresolved_for_phase_2b() {
    common::require_infra!();
    // HTML with a quip.com link to another in-scope thread; assert an UNRESOLVED#
    // row exists naming the source block and target thread.
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p ogrenotes-api --test test_quip_content_worker`
Expected: FAIL to compile / no content pass.

- [ ] **Step 3: Implement `import_one_thread`**

Order matters — each step's failure must leave the thread retryable:

1. **Skip if already done.** `thread.state != ThreadState::Pending` → return `Ok(())`.
2. **Disposition by type.** Quip `thread_type` of `"chat"` → `set_thread_skipped` + report note → `Ok(())`. `"spreadsheet"` → `DocType::Spreadsheet`; otherwise `DocType::Document`.
3. **Fetch** `client.thread_html(token, &thread.quip_thread_id)`.
4. **Stage** the raw HTML to S3 at `imports/{import_id}/threads/{quip_thread_id}.html` via `ctx.s3.put_object`; remember the key.
5. **Walk** → `from_quip_html(&html)`.
6. **Side-load blobs.** For each `QuipImageRef`: parse the Quip blob id out of `src` (Quip image srcs look like `/blob/<thread_id>/<blob_id>`); fetch via `client.blob(...)`; `put_object` to `blobs/{doc_id}/{blob_id}/{filename}` — **note `doc_id` is not known until step 7**, so mint the `doc_id` up front (`new_id()`) and pass it into `persist_imported_document` rather than letting it mint one. Rewrite the image node's `src` to `blob_ref(blob_id, key)`. A blob that fails to fetch: leave the node with its `alt`, drop the `src`, and record a report note — one bad image must not fail the thread.
7. **Persist** via `persist_imported_document` with the minted `doc_id`, `thread.title`, `owner_id`, `folder_id = thread.first_folder`, `additional_folder_ids = thread.member_folders minus first_folder`, `doc_type`, and `created_at = updated_at = thread.updated_usec`.
   - The folder ids in `THREAD#` rows are **Quip** folder ids; map them to OgreNotes folder ids via the `FOLDER#` rows' `ogre_folder_id`. **If `ogre_folder_id` is not yet populated** (Phase 1 wrote `None`), fall back to the import's `target_folder_id` from the `META` row for every thread, and record that in the report. Creating the mirrored OgreNotes folder tree is out of scope here — flag it as a Phase-2a follow-up if the demo shows a flat structure is unacceptable.
8. **Write `SECMAP#` chunks** from `QuipDocument.sections`, chunked at `SECMAP_CHUNK_ENTRIES`.
9. **Write `UNRESOLVED#`** if `pending_links` is non-empty.
10. **Checkpoint** `set_thread_content_done(import_id, thread_id, &doc_id, &staged_key)`.

`run_content_pass` lists threads via `list_threads`, iterates in a stable order, heartbeats the runner lease every N threads (the lease is best-effort per Phase 1's design), and maps errors the same way inventory does: `QuipError::Unauthorized` → `TokenRejected` + `Ok(())`; transient → `Err` so the queue retries.

Wire `run_content_pass` into `run_inventory`'s tail (or the caller) so a single `StartQuipImport` job does inventory → content, and set `phase = 2` when the content pass completes.

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p ogrenotes-api --test test_quip_content_worker`
Expected: PASS (all five).

- [ ] **Step 5: Confirm no regressions**

Run: `cargo test -p ogrenotes-api --test test_quip_inventory_worker --test test_worker_mode`
Expected: PASS — the `persist_imported_document` signature change must not disturb DOCX/PDF.

- [ ] **Step 6: Commit**

```bash
git add crates/api/src/worker_mode.rs crates/api/tests/test_quip_content_worker.rs
git commit -m "feat(worker): Quip content pass — convert threads to documents with blobs"
```

---

## Task 7: Wizard — content-phase progress

**Files:**
- Modify: `frontend/src/components/quip_import/mod.rs`
- Modify: `frontend/locales/*/main.ftl` (6 locales)
- Test: wasm32 build; optional doctor probe extension

**Interfaces:**
- Consumes: the existing `GET /imports/quip/{id}` → `{status, phase, progress{done,total,stage}}`. Phase 1 already returns `done` = threads past `Pending` and `total` = all threads, so the content pass makes `done` climb for free; `stage` becomes `"content"` when `phase >= 2`.

- [ ] **Step 1: Extend `get_status`'s stage** (`crates/api/src/routes/imports.rs`)

`stage = match record.phase { 0 => "scoping", 1 => "inventory", _ => "content" }`. Keep the existing `done`/`total` computation untouched.

- [ ] **Step 2: Render the content phase in the wizard**

The poll loop currently stops at `phase >= 1`. Change the terminal condition to `phase >= 2` (content complete) so the wizard keeps polling through the content pass, and render:
- `phase == 0` → "Scanning Quip…"
- `phase == 1` → "Found {total} items to import" + "Importing… {done} of {total}"
- `phase >= 2` → "Imported {total} items" + a link to the target folder

Keep the existing per-session `generation` guard, terminal-status handling (`failed`/`tokenrejected`/`cancelled`), and `a11y::defer_close` discipline exactly as-is.

- [ ] **Step 3: Add i18n keys to all 6 locales**

```
quip-import-importing = Importing… { $done } of { $total }
quip-import-content-done = Imported { $total } items
quip-import-open-folder = Open folder
```

- [ ] **Step 4: Verify**

Run: `cd frontend && cargo build --target wasm32-unknown-unknown`
Expected: Finished clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/quip_import/mod.rs frontend/locales crates/api/src/routes/imports.rs
git commit -m "feat(frontend): Quip import content-phase progress"
```

---

## Self-Review

**Spec coverage (design §Pipeline Phase 2, first half):**
- "fetch `/2` HTML → stage S3" → Task 4 + Task 6 step 3-4. ✓
- "walk → assign `blockId` per block + persist `SECMAP`" → Tasks 1, 2, 5, 6 step 8. ✓
- "fetch blobs → `put_object` under `blobs/{doc_id}/…` + set `Image.src`" → Task 6 step 6 (with Task 3 making the src durable). ✓
- "emit placeholders + record `UNRESOLVED#` for intra-import links" → Task 2 + Task 6 step 9. ✓ (resolution is Phase 2b)
- "create doc via the `persist_imported_document` path preserving Quip timestamps" → Task 6 step 7. ✓
- "(+ explicit search index)" → **deliberately deferred, issue #138** (structurally impossible from a separate-process worker). ✗ by design.
- "checkpoint `ContentDone`" → Task 5 + Task 6 step 10. ✓
- "Spreadsheet threads → native Spreadsheet doc" → Task 6 step 2. ✓
- "Chat threads → skip, note" → Task 6 step 2. ✓
- "Embedded grids → Spreadsheet doc + `DocMention` (interim, #133)" → **not covered by a dedicated task.** Handle inside Task 6 step 2 if Quip's HTML exposes embedded grids distinguishably; if the demo shows they need real work, split them into their own task rather than bloating Task 6.
- Converter fixtures matrix (design §Testing) → Task 2 step 7. ✓

**Placeholder scan:** No "TBD"/"handle errors appropriately" steps. Three places defer deliberately *with named reasons*: search indexing (#138), the mirrored folder tree (Task 6 step 7 fallback), and embedded grids (above). Each says what happens instead.

**Type consistency:** `QuipDocument`/`QuipImageRef`/`QuipPendingLink` declared in Task 1, populated in Task 2, consumed in Task 6. `SecMapRow`/`UnresolvedRow`/`PendingLinkItem` declared in Task 5, written in Task 6. `blob_ref`/`parse_blob_ref` declared in Task 3, used in Task 6 and the frontend. `persist_imported_document`'s new signature is stated once in Task 6 and its existing callers are explicitly updated there.

**Known risk — unverified against real Quip HTML.** No sample of Quip's `/2/threads/{id}/html` was available while writing this. The walker's assumptions (section ids on block-level `id` attributes; checklists as `<input type=checkbox>` inside `<li>`; code language via `data-language`/`class`; image srcs shaped `/blob/<thread>/<blob>`) are drawn from the design doc and Quip's public API shape. **The first real-token demo must dump one real thread's HTML and reconcile** — expect Task 1/2 adjustments. Budget for it; do not treat the fixtures as authoritative until then.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-29-quip-import-phase2a.md`.
