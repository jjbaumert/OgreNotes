// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Quip `/2` HTML → OgreNotes document (Quip import, Phase 2a).
//!
//! A Quip-specific walker, deliberately **not** `import::from_html`:
//! that one drops inline marks and knows nothing about tables, images,
//! task lists, or blockIds (see `import.rs:14-19,31`). Quip needs all
//! four, and it needs a stable `blockId` on every element so a Quip
//! section anchor can be mapped onto an OgreNotes block later.
//!
//! Split like `import_docx` (`import_docx.rs:32-35`): `parse_quip`
//! produces a `Vec<QuipBlock>` so the HTML state machine never holds a
//! live yrs transaction, and the two halves test independently.
//!
//! Pipeline (mirrors `import::from_html`):
//!
//!   1. **ammonia.clean** with an *extended* allowlist — the base list
//!      in `import.rs:363-377` omits exactly what Quip needs (tables,
//!      images, checkbox inputs). `script` / `iframe` / `style` /
//!      `form` stay out.
//!   2. **html5ever** parses the cleaned string into an `RcDom`.
//!   3. A recursive walker maps the DOM to `Vec<QuipBlock>`;
//!      `enforce_containment` then hoists anything the schema forbids;
//!      `materialize` builds the yrs `Doc`.
//!
//! ## The markup shape is an educated guess
//!
//! No sample of a real `/2/threads/{id}/html` response was available
//! when this was written. Every assumption about how Quip spells a
//! feature is therefore isolated behind a small named helper carrying
//! an `UNVERIFIED MARKUP` note — those helpers are the reconciliation
//! checklist when real markup arrives:
//!
//!   - [`checked_state`]  — checklist item checked/unchecked
//!   - [`list_is_task`]   — a list being a checklist at all
//!   - [`code_language`]  — code-block language tag
//!   - [`section_id`]     — Quip section anchor id
//!
//! Everything else degrades gracefully by construction: unknown tags
//! are transparent passthrough (their children are walked in the same
//! context) rather than dropped, exactly as `import.rs:417-423` does.

use std::collections::{HashMap, HashSet};

use yrs::{
    Doc, Transact, WriteTxn, Xml, XmlElementRef,
    types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim},
};

use crate::schema::NodeType;

// ─── blockId minting ─────────────────────────────────────────────

const BLOCK_ID_LEN: usize = 10;
#[rustfmt::skip]
const BLOCK_ID_ALPHABET: [char; 62] = [
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M',
    'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm',
    'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
];

/// Mint a block id. Matches the frontend's `generate_block_id`
/// (10 chars, `[A-Za-z0-9]`) and stays inside the 4–32 alphanumeric
/// range `routes::comments` validates for client-supplied ids.
pub fn new_block_id() -> String {
    nanoid::nanoid!(BLOCK_ID_LEN, &BLOCK_ID_ALPHABET)
}

// ─── public result types ─────────────────────────────────────────

/// The product of a Quip HTML import: the document itself plus the
/// side-tables the caller needs to finish the job (side-load blobs,
/// back-patch cross-document links, resolve section anchors).
pub struct QuipDocument {
    pub doc: Doc,
    /// Quip section id → minted blockId, in document order.
    pub sections: Vec<(String, String)>,
    /// Images referenced by the source, to be side-loaded by the caller.
    pub images: Vec<QuipImageRef>,
    /// Intra-Quip links needing Phase-2b back-patch.
    pub pending_links: Vec<QuipPendingLink>,
}

/// An image the source referenced, keyed by the blockId of the
/// `image` element that carries it.
pub struct QuipImageRef {
    pub block_id: String,
    pub src: String,
    pub alt: String,
}

/// A link to another Quip thread, recorded so Phase 2b can rewrite it
/// to the imported OgreNotes document once that document exists.
pub struct QuipPendingLink {
    pub source_block_id: String,
    pub target_quip_thread_id: String,
    pub target_quip_section_id: Option<String>,
}

/// Import a Quip HTML body into a fresh `Doc`.
///
/// Phase 2a Task 1 populates `doc` only; `sections` / `images` /
/// `pending_links` are filled in by the marks-and-anchors pass.
pub fn from_quip_html(html: &str) -> QuipDocument {
    let blocks = parse_quip(html);
    QuipDocument {
        doc: materialize(&blocks),
        sections: Vec::new(),
        images: Vec::new(),
        pending_links: Vec::new(),
    }
}

// ─── intermediate block model ────────────────────────────────────

/// A run of inline content. Task 1 carries text plus hard-break
/// position only; the mark set is added by the marks pass, so the
/// field name is stable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Span {
    pub text: String,
    /// Set when a `<br>` immediately precedes this run. `HardBreak` is
    /// an inline leaf (`NodeType::is_inline`, `schema.rs:184-186`), a
    /// category `NodeType::valid_children` never enumerates — it only
    /// governs *block*-level containment (see the doc on
    /// `insert_block`'s containment check below). Materializing this
    /// as a real `HardBreak` element rather than folding it into the
    /// text as `\n` keeps the invariant documented at
    /// `frontend/style/main.css:906-907`: a text node never carries a
    /// literal newline.
    pub hard_break_before: bool,
}

impl Span {
    fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), hard_break_before: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuipCell {
    pub header: bool,
    pub blocks: Vec<QuipBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuipRow {
    pub cells: Vec<QuipCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuipItem {
    pub checked: Option<bool>,
    pub blocks: Vec<QuipBlock>,
}

/// Block-level intermediate, decoupled from yrs so the walker is a
/// pure `&str -> Vec<QuipBlock>` function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuipBlock {
    Para {
        // Read by the section-anchor (SECMAP) pass, which lands in the
        // next task; captured here so that pass is purely additive.
        #[allow(dead_code)]
        section_id: Option<String>,
        spans: Vec<Span>,
    },
    Heading {
        level: u8,
        #[allow(dead_code)]
        section_id: Option<String>,
        spans: Vec<Span>,
    },
    List {
        ordered: bool,
        task: bool,
        items: Vec<QuipItem>,
    },
    Quote {
        blocks: Vec<QuipBlock>,
    },
    Code {
        language: String,
        text: String,
    },
    Rule,
    Table {
        rows: Vec<QuipRow>,
    },
    Image {
        src: String,
        alt: String,
    },
}

impl QuipBlock {
    /// The schema node this block materializes as — the key the
    /// containment pass checks against `NodeType::valid_children`.
    fn node_type(&self) -> NodeType {
        match self {
            QuipBlock::Para { .. } => NodeType::Paragraph,
            QuipBlock::Heading { .. } => NodeType::Heading,
            QuipBlock::List { ordered, task, .. } => match (task, ordered) {
                (true, _) => NodeType::TaskList,
                (false, true) => NodeType::OrderedList,
                (false, false) => NodeType::BulletList,
            },
            QuipBlock::Quote { .. } => NodeType::Blockquote,
            QuipBlock::Code { .. } => NodeType::CodeBlock,
            QuipBlock::Rule => NodeType::HorizontalRule,
            QuipBlock::Table { .. } => NodeType::Table,
            QuipBlock::Image { .. } => NodeType::Image,
        }
    }
}

fn empty_para() -> QuipBlock {
    QuipBlock::Para { section_id: None, spans: Vec::new() }
}

// ─── stage 1: sanitize ───────────────────────────────────────────

/// Tags ammonia lets through. The union of what the walker
/// understands plus the inline tags whose *text* must survive even
/// though Task 1 drops their marks. Deliberately wider than
/// `import::allowed_html_tags` (tables / images / checkbox inputs);
/// deliberately still without `script`, `iframe`, `style`, `form`,
/// `object`, `embed`.
fn allowed_tags() -> HashSet<&'static str> {
    [
        "html", "head", "body", //
        "p", "div", "span", "section", "article", //
        "h1", "h2", "h3", "h4", "h5", "h6", //
        "ul", "ol", "li", //
        "blockquote", //
        "pre", "code", //
        "hr", "br", //
        "a", "img", "input", //
        "table", "thead", "tbody", "tfoot", "tr", "th", "td", //
        "strong", "em", "b", "i", "u", "s", "del", "sub", "sup", "mark",
    ]
    .into_iter()
    .collect()
}

/// Attributes the walker reads. Anything not listed here (and not
/// `data-*`) is stripped before the DOM is built, so the walker never
/// sees an event handler or a style payload.
fn allowed_attributes() -> HashSet<&'static str> {
    [
        "id", "href", "src", "alt", "title", "type", "checked", "value",
        "colspan", "rowspan", "class", "start", "align",
    ]
    .into_iter()
    .collect()
}

fn sanitize(html: &str) -> String {
    ammonia::Builder::default()
        .tags(allowed_tags())
        .generic_attributes(allowed_attributes())
        // `data-language`, `data-checked`, `data-type`, `data-section-id`
        // — every spelling of a Quip hint we might have to read. The
        // prefix form means a reconciliation only touches the reader
        // helper, never this allowlist.
        .generic_attribute_prefixes(["data-"].into_iter().collect())
        // ammonia would otherwise rewrite `id` with a prefix; Quip
        // section ids must survive byte-for-byte to be matchable.
        .id_prefix(None)
        .tag_attributes(HashMap::new())
        .clean(html)
        .to_string()
}

// ─── stage 2+3: parse to the intermediate block model ────────────

/// Parse Quip HTML into the intermediate block model. Pure: no yrs
/// transaction is live while the DOM is walked.
pub(crate) fn parse_quip(html: &str) -> Vec<QuipBlock> {
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::RcDom;

    let safe = sanitize(html);
    let dom: RcDom = html5ever::parse_document(RcDom::default(), html5ever::driver::ParseOpts::default())
        .from_utf8()
        .read_from(&mut safe.as_bytes())
        .expect("html5ever parse is infallible on bytes");

    let mut out = Vec::new();
    let mut pending = InlineBuf::default();
    walk_children(&dom.document, &mut out, &mut pending);
    pending.flush(&mut out, None);

    enforce_containment(out, NodeType::Doc)
}

/// Inline text accumulated between block boundaries. Flushed as a
/// paragraph when a block element interrupts it or the parent closes.
#[derive(Default)]
struct InlineBuf {
    spans: Vec<Span>,
}

impl InlineBuf {
    fn push_text(&mut self, s: &str) {
        match self.spans.last_mut() {
            Some(last) => last.text.push_str(s),
            None => self.spans.push(Span::new(s)),
        }
    }

    /// Record a `<br>`. Pushes a fresh, empty span flagged
    /// `hard_break_before` so the next `push_text` call appends *after*
    /// the break rather than into the run that preceded it; two
    /// consecutive breaks therefore produce two distinct empty spans,
    /// which `materialize_block` turns into two distinct `HardBreak`
    /// elements — matching the real DOM's `<br><br>`.
    fn push_break(&mut self) {
        self.spans.push(Span { text: String::new(), hard_break_before: true });
    }

    /// Emit the buffer as a paragraph (dropping it when it holds only
    /// whitespace) and reset.
    fn flush(&mut self, out: &mut Vec<QuipBlock>, section_id: Option<String>) {
        let spans = std::mem::take(&mut self.spans);
        if spans.iter().all(|s| s.text.trim().is_empty()) {
            return;
        }
        out.push(QuipBlock::Para { section_id, spans: trim_spans(spans) });
    }
}

/// Trim leading/trailing whitespace across the span run without
/// disturbing the interior (which may carry meaningful single spaces
/// between formatted runs).
fn trim_spans(mut spans: Vec<Span>) -> Vec<Span> {
    if let Some(first) = spans.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = spans.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    // An empty span still marks a `<br>` — dropping it here would
    // silently eat a leading/trailing hard break.
    spans.retain(|s| !s.text.is_empty() || s.hard_break_before);
    spans
}

/// Inline-level tags: their text joins the surrounding paragraph
/// rather than starting a new block. Anything *not* here and not a
/// known block tag is treated as a transparent block-level wrapper
/// (`div`, `section`, and whatever Quip invents), matching
/// `import.rs:417-423`.
fn is_inline_tag(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "span"
            | "strong"
            | "em"
            | "b"
            | "i"
            | "u"
            | "s"
            | "del"
            | "code"
            | "sub"
            | "sup"
            | "mark"
            | "input"
            | "br"
    )
}

fn walk_children(handle: &markup5ever_rcdom::Handle, out: &mut Vec<QuipBlock>, pending: &mut InlineBuf) {
    for child in handle.children.borrow().iter() {
        walk_node(child, out, pending);
    }
}

fn walk_node(handle: &markup5ever_rcdom::Handle, out: &mut Vec<QuipBlock>, pending: &mut InlineBuf) {
    use markup5ever_rcdom::NodeData;

    match &handle.data {
        NodeData::Document => walk_children(handle, out, pending),
        NodeData::Text { contents } => {
            let s = contents.borrow();
            let raw = s.as_ref();
            if raw.trim().is_empty() {
                // Whitespace between block tags is layout, not content
                // — but whitespace *inside* a run separates words.
                if !pending.spans.is_empty() && !raw.is_empty() {
                    pending.push_text(" ");
                }
                return;
            }
            pending.push_text(raw);
        }
        NodeData::Element { name, .. } => {
            let tag = name.local.as_ref().to_ascii_lowercase();
            walk_element(handle, &tag, out, pending);
        }
        _ => {
            // Comments, doctype, processing instructions: dropped.
        }
    }
}

fn walk_element(
    handle: &markup5ever_rcdom::Handle,
    tag: &str,
    out: &mut Vec<QuipBlock>,
    pending: &mut InlineBuf,
) {
    match tag {
        // Scaffolding html5ever inserts — descend transparently.
        "html" | "body" => walk_children(handle, out, pending),
        // `<head>` holds no document content.
        "head" => {}
        "p" => {
            pending.flush(out, None);
            let (spans, rest) = walk_text_container(handle);
            // An empty `<p>` is a deliberate spacer and is kept; an
            // empty one that only wrapped a block (`<p><img></p>`) is
            // not, or every image would trail a ghost paragraph.
            if !spans.is_empty() || rest.is_empty() {
                out.push(QuipBlock::Para { section_id: section_id(handle), spans });
            }
            out.extend(rest);
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            pending.flush(out, None);
            let level = tag.as_bytes()[1] - b'0';
            let (spans, rest) = walk_text_container(handle);
            out.push(QuipBlock::Heading { level, section_id: section_id(handle), spans });
            out.extend(rest);
        }
        "ul" | "ol" => {
            pending.flush(out, None);
            out.push(parse_list(handle, tag == "ol"));
        }
        "li" => {
            // A stray `<li>` outside any list — wrap it so the content
            // survives instead of being dropped.
            pending.flush(out, None);
            out.push(QuipBlock::List {
                ordered: false,
                task: false,
                items: vec![parse_item(handle)],
            });
        }
        "blockquote" => {
            pending.flush(out, None);
            let mut inner = Vec::new();
            let mut buf = InlineBuf::default();
            walk_children(handle, &mut inner, &mut buf);
            buf.flush(&mut inner, None);
            out.push(QuipBlock::Quote { blocks: inner });
        }
        "pre" => {
            pending.flush(out, None);
            out.push(QuipBlock::Code {
                language: code_language(handle),
                text: raw_text(handle),
            });
        }
        "hr" => {
            pending.flush(out, None);
            out.push(QuipBlock::Rule);
        }
        "table" => {
            pending.flush(out, None);
            out.push(parse_table(handle));
        }
        "img" => {
            // An image is a block node in this schema, so it closes any
            // paragraph in progress. Marks/links around it are the next
            // task's problem; the `src` stays the raw Quip value until
            // the blob side-load pass rewrites it.
            pending.flush(out, None);
            out.push(QuipBlock::Image {
                src: attr(handle, "src").unwrap_or_default(),
                alt: attr(handle, "alt").unwrap_or_default(),
            });
        }
        // A hard line break inside a run of text. This is *not* folded
        // into the text as `\n` — see the doc on `Span::hard_break_before`.
        // `Paragraph::valid_children()` being empty says nothing about
        // this: it's a block-containment predicate, and `HardBreak` is
        // an inline leaf, a category that predicate doesn't cover.
        // `import.rs:198-208` inserts a real `HardBreak` *element* into
        // the open paragraph for exactly this reason; `push_break` is
        // the equivalent move for this walker's two-stage pipeline
        // (`materialize_block` turns the flag into that element once a
        // yrs transaction actually exists).
        "br" => pending.push_break(),
        // A checkbox is consumed by `checked_state` on its `<li>`; it
        // contributes no text of its own.
        "input" => {}
        _ if is_inline_tag(tag) => walk_children(handle, out, pending),
        // Unknown / structural tag: transparent passthrough. The
        // children are walked in the *same* context, so a `<div>`
        // wrapper neither creates a block nor breaks a paragraph.
        _ => walk_children(handle, out, pending),
    }
}

/// Walk a text container (`<p>`, `<h1>`…`<h6>`): all of its text folds
/// into one block's spans, and any *block* it wrapped is returned to be
/// emitted after it. A paragraph can't hold an image or a table in this
/// schema, so hoisting is the only lossless option.
fn walk_text_container(handle: &markup5ever_rcdom::Handle) -> (Vec<Span>, Vec<QuipBlock>) {
    let mut nested = Vec::new();
    let mut buf = InlineBuf::default();
    walk_children(handle, &mut nested, &mut buf);
    buf.flush(&mut nested, None);

    let mut spans: Vec<Span> = Vec::new();
    let mut rest = Vec::new();
    for block in nested {
        match block {
            QuipBlock::Para { spans: s, .. } => {
                if !spans.is_empty() && !s.is_empty() {
                    spans.push(Span::new(" "));
                }
                spans.extend(s);
            }
            other => rest.push(other),
        }
    }
    (trim_spans(spans), rest)
}

fn parse_list(handle: &markup5ever_rcdom::Handle, ordered: bool) -> QuipBlock {
    let mut items = Vec::new();
    collect_items(handle, &mut items);

    // A list is a checklist if any item carries real checked state, or
    // if the list itself is marked as one — except for `<ol>`, where a
    // checklist-ish class alone is more likely styling on a numbered
    // list, so per-item state is required.
    let has_item_state = items.iter().any(|i| i.checked.is_some());
    let task = has_item_state || (!ordered && list_is_task(handle));
    if task {
        // A list marked as a checklist whose items say nothing: the
        // items default to unchecked.
        for item in &mut items {
            item.checked = Some(item.checked.unwrap_or(false));
        }
    }

    QuipBlock::List { ordered, task, items }
}

/// Gather the `<li>` children of a list, descending through wrapper
/// elements Quip might interpose but *not* into nested lists (those
/// are parsed as blocks inside their own item).
fn collect_items(handle: &markup5ever_rcdom::Handle, items: &mut Vec<QuipItem>) {
    use markup5ever_rcdom::NodeData;
    for child in handle.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else { continue };
        let tag = name.local.as_ref().to_ascii_lowercase();
        match tag.as_str() {
            "li" => items.push(parse_item(child)),
            "ul" | "ol" => {
                // A list directly inside a list (no `<li>` between) is
                // malformed; keep its items at this level.
                collect_items(child, items);
            }
            _ => collect_items(child, items),
        }
    }
}

fn parse_item(handle: &markup5ever_rcdom::Handle) -> QuipItem {
    let mut blocks = Vec::new();
    let mut buf = InlineBuf::default();
    walk_children(handle, &mut blocks, &mut buf);
    // A block child flushes the buffer as it goes, so the trailing
    // text is appended last and document order is preserved: an item
    // reading `a <ul>…</ul> b` keeps `b` after the nested list.
    buf.flush(&mut blocks, None);
    QuipItem { checked: checked_state(handle), blocks }
}

fn parse_table(handle: &markup5ever_rcdom::Handle) -> QuipBlock {
    let mut rows = Vec::new();
    collect_rows(handle, &mut rows);
    QuipBlock::Table { rows }
}

fn collect_rows(handle: &markup5ever_rcdom::Handle, rows: &mut Vec<QuipRow>) {
    use markup5ever_rcdom::NodeData;
    for child in handle.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else { continue };
        let tag = name.local.as_ref().to_ascii_lowercase();
        match tag.as_str() {
            "tr" => rows.push(parse_row(child)),
            // thead / tbody / tfoot (and any wrapper) are transparent.
            "table" => {} // nested table: handled inside its cell
            _ => collect_rows(child, rows),
        }
    }
}

fn parse_row(handle: &markup5ever_rcdom::Handle) -> QuipRow {
    use markup5ever_rcdom::NodeData;
    let mut cells = Vec::new();
    for child in handle.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else { continue };
        let tag = name.local.as_ref().to_ascii_lowercase();
        if tag != "td" && tag != "th" {
            continue;
        }
        let mut blocks = Vec::new();
        let mut buf = InlineBuf::default();
        walk_children(child, &mut blocks, &mut buf);
        buf.flush(&mut blocks, None);
        if blocks.is_empty() {
            // A cell must have a body — an empty one renders as a
            // collapsed grid slot in the editor.
            blocks.push(empty_para());
        }
        cells.push(QuipCell { header: tag == "th", blocks });
    }
    QuipRow { cells }
}

// ─── UNVERIFIED-MARKUP readers ───────────────────────────────────
//
// Each helper below encodes a guess about how Quip spells a feature.
// They accept every plausible spelling rather than betting on one, and
// they are the only places that need to change when a real `/2` HTML
// sample lands.

/// **UNVERIFIED MARKUP.** Checked state of a checklist item. Accepts:
///
///   - `<li><input type="checkbox" checked>` (presence of the
///     attribute means checked, per the HTML spec)
///   - `<li class="checked">` / `<li class="unchecked">`, and the
///     `-item` / `_item` suffixed variants
///   - `<li data-checked="true|false">`
///
/// Returns `None` when the item carries no checklist signal at all —
/// that's what makes an ordinary bullet stay an ordinary bullet.
fn checked_state(li: &markup5ever_rcdom::Handle) -> Option<bool> {
    if let Some(v) = attr(li, "data-checked") {
        return Some(!matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0" | ""));
    }
    for class in classes(li) {
        match class.as_str() {
            "checked" | "checked-item" | "checked_item" | "is-checked" => return Some(true),
            "unchecked" | "unchecked-item" | "unchecked_item" | "is-unchecked" => {
                return Some(false);
            }
            _ => {}
        }
    }
    find_checkbox(li).map(|cb| match attr(&cb, "checked") {
        // A bare boolean attribute serializes as `checked=""`; only an
        // explicit "false" counts as unchecked.
        Some(v) => !v.trim().eq_ignore_ascii_case("false"),
        None => false,
    })
}

/// Find this item's own checkbox, not one belonging to a nested list.
fn find_checkbox(handle: &markup5ever_rcdom::Handle) -> Option<markup5ever_rcdom::Handle> {
    use markup5ever_rcdom::NodeData;
    for child in handle.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else { continue };
        let tag = name.local.as_ref().to_ascii_lowercase();
        match tag.as_str() {
            "input" => {
                let ty = attr(child, "type").unwrap_or_default();
                if ty.eq_ignore_ascii_case("checkbox") || ty.is_empty() {
                    return Some(child.clone());
                }
            }
            // Don't cross into a nested list — its checkboxes belong
            // to its own items.
            "ul" | "ol" | "li" => {}
            _ => {
                if let Some(found) = find_checkbox(child) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// **UNVERIFIED MARKUP.** Whether a `<ul>` / `<ol>` is a checklist
/// independent of its items. Accepts a `checklist` / `task-list` /
/// `tasklist` / `todo` class token, or `data-type="taskList"` (the
/// spelling our own HTML export uses).
fn list_is_task(list: &markup5ever_rcdom::Handle) -> bool {
    if let Some(t) = attr(list, "data-type")
        && t.eq_ignore_ascii_case("tasklist")
    {
        return true;
    }
    classes(list).iter().any(|c| {
        matches!(c.as_str(), "checklist" | "task-list" | "tasklist" | "task_list" | "todo" | "todo-list")
    })
}

/// **UNVERIFIED MARKUP.** Language tag of a `<pre>` code block.
/// Accepts, in order: `data-language` on the `<pre>`, a
/// `language-x` / `lang-x` class on the `<pre>`, then the same on a
/// descendant `<code>`. Empty string when nothing says.
fn code_language(pre: &markup5ever_rcdom::Handle) -> String {
    if let Some(l) = attr(pre, "data-language")
        && !l.trim().is_empty()
    {
        return l.trim().to_string();
    }
    if let Some(l) = language_from_classes(pre) {
        return l;
    }
    if let Some(code) = find_descendant(pre, "code") {
        if let Some(l) = attr(&code, "data-language")
            && !l.trim().is_empty()
        {
            return l.trim().to_string();
        }
        if let Some(l) = language_from_classes(&code) {
            return l;
        }
    }
    String::new()
}

fn language_from_classes(handle: &markup5ever_rcdom::Handle) -> Option<String> {
    classes(handle).into_iter().find_map(|c| {
        c.strip_prefix("language-")
            .or_else(|| c.strip_prefix("lang-"))
            .filter(|rest| !rest.is_empty())
            .map(str::to_string)
    })
}

/// **UNVERIFIED MARKUP.** The Quip section anchor of a block element.
/// Assumed to be the plain `id` attribute; `data-section-id` is
/// accepted as an alternative spelling. Quip section ids are opaque —
/// no charset validation here on purpose.
fn section_id(handle: &markup5ever_rcdom::Handle) -> Option<String> {
    attr(handle, "id")
        .or_else(|| attr(handle, "data-section-id"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ─── DOM helpers ─────────────────────────────────────────────────

fn attr(handle: &markup5ever_rcdom::Handle, name: &str) -> Option<String> {
    use markup5ever_rcdom::NodeData;
    let NodeData::Element { attrs, .. } = &handle.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|a| a.name.local.as_ref().eq_ignore_ascii_case(name))
        .map(|a| a.value.to_string())
}

fn classes(handle: &markup5ever_rcdom::Handle) -> Vec<String> {
    attr(handle, "class")
        .map(|c| c.split_whitespace().map(|t| t.to_ascii_lowercase()).collect())
        .unwrap_or_default()
}

fn find_descendant(
    handle: &markup5ever_rcdom::Handle,
    tag: &str,
) -> Option<markup5ever_rcdom::Handle> {
    use markup5ever_rcdom::NodeData;
    for child in handle.children.borrow().iter() {
        if let NodeData::Element { name, .. } = &child.data
            && name.local.as_ref().eq_ignore_ascii_case(tag)
        {
            return Some(child.clone());
        }
        if let Some(found) = find_descendant(child, tag) {
            return Some(found);
        }
    }
    None
}

/// Verbatim text of a subtree — used for `<pre>`, where interior
/// whitespace is content.
fn raw_text(handle: &markup5ever_rcdom::Handle) -> String {
    use markup5ever_rcdom::NodeData;
    let mut out = String::new();
    fn go(handle: &markup5ever_rcdom::Handle, out: &mut String) {
        use markup5ever_rcdom::NodeData;
        match &handle.data {
            NodeData::Text { contents } => out.push_str(contents.borrow().as_ref()),
            _ => {
                for child in handle.children.borrow().iter() {
                    go(child, out);
                }
            }
        }
    }
    if let NodeData::Element { .. } | NodeData::Document = &handle.data {
        for child in handle.children.borrow().iter() {
            go(child, &mut out);
        }
    }
    // html5ever keeps the newline right after `<pre>`; the browser
    // swallows it, so we do too.
    out.strip_prefix('\n').map(str::to_string).unwrap_or(out)
}

// ─── containment: hoist what the schema forbids ──────────────────

/// Normalize a block sequence so every parent/child pair the
/// materializer will emit is legal per `NodeType::valid_children`.
///
/// Quip's HTML happily nests an image (or a table, or a rule) inside a
/// list item or a table cell; our schema does not. Rather than drop
/// that content or emit an invalid tree, we **hoist**: the offending
/// block is lifted to the nearest ancestor that accepts it. Content is
/// preserved; only nesting changes.
fn enforce_containment(blocks: Vec<QuipBlock>, parent: NodeType) -> Vec<QuipBlock> {
    let (kept, escaped) = split_for(blocks, parent);
    // At the top level (`Doc`) nothing can escape further — `Doc`
    // accepts every block kind the walker produces. The `escaped`
    // tail is therefore empty in practice; appending it is the
    // defensive branch that guarantees no content is ever dropped.
    let mut out = kept;
    out.extend(escaped);
    out
}

/// Route each block into `kept` (legal under `parent`) or `escaped`
/// (must be emitted by an ancestor), recursing into containers first.
fn split_for(blocks: Vec<QuipBlock>, parent: NodeType) -> (Vec<QuipBlock>, Vec<QuipBlock>) {
    let allowed = parent.valid_children();
    let mut kept = Vec::new();
    let mut escaped = Vec::new();
    for block in blocks {
        for flat in flatten(block) {
            if allowed.contains(&flat.node_type()) {
                kept.push(flat);
            } else {
                escaped.push(flat);
            }
        }
    }
    (kept, escaped)
}

/// Expand one block into an ordered sequence of containment-clean
/// blocks. A container that held an illegal child yields the container
/// followed (or interleaved, for lists) by the hoisted blocks.
fn flatten(block: QuipBlock) -> Vec<QuipBlock> {
    match block {
        QuipBlock::Quote { blocks } => {
            let (kept, escaped) = split_for(blocks, NodeType::Blockquote);
            let mut out = vec![QuipBlock::Quote { blocks: kept }];
            // A blockquote can't be split without changing what reads
            // as quoted, so escapees follow the quote.
            out.extend(escaped);
            out
        }
        QuipBlock::List { ordered, task, items } => flatten_list(ordered, task, items),
        QuipBlock::Table { rows } => flatten_table(rows),
        other => vec![other],
    }
}

/// Lists split at item boundaries: when an item holds something a list
/// item can't contain, the list closes, the hoisted blocks are emitted,
/// and a fresh list of the same kind resumes. That keeps document order
/// exact at item granularity.
fn flatten_list(ordered: bool, task: bool, items: Vec<QuipItem>) -> Vec<QuipBlock> {
    let item_ctx = if task { NodeType::TaskItem } else { NodeType::ListItem };
    let mut out = Vec::new();
    let mut run: Vec<QuipItem> = Vec::new();

    for item in items {
        let QuipItem { checked, blocks } = item;
        let (mut kept, escaped) = split_for(blocks, item_ctx);
        if kept.is_empty() {
            // An item whose entire content was hoisted still needs a
            // body — an empty item is legal but renders as a ghost.
            kept.push(empty_para());
        }
        run.push(QuipItem { checked, blocks: kept });
        if !escaped.is_empty() {
            out.push(QuipBlock::List { ordered, task, items: std::mem::take(&mut run) });
            out.extend(escaped);
        }
    }

    if !run.is_empty() {
        out.push(QuipBlock::List { ordered, task, items: run });
    }
    out
}

/// Tables can't be split the way lists can — closing a row mid-table
/// would mangle the grid — so a cell's escapees are emitted *after*
/// the whole table, in row-major order.
fn flatten_table(rows: Vec<QuipRow>) -> Vec<QuipBlock> {
    let mut after = Vec::new();
    let mut clean_rows = Vec::new();
    for row in rows {
        let mut cells = Vec::new();
        for cell in row.cells {
            let ctx = if cell.header { NodeType::TableHeader } else { NodeType::TableCell };
            let (mut kept, escaped) = split_for(cell.blocks, ctx);
            if kept.is_empty() {
                kept.push(empty_para());
            }
            after.extend(escaped);
            cells.push(QuipCell { header: cell.header, blocks: kept });
        }
        clean_rows.push(QuipRow { cells });
    }
    let mut out = vec![QuipBlock::Table { rows: clean_rows }];
    out.extend(after);
    out
}

// ─── materialize into yrs ────────────────────────────────────────

/// Either the root XmlFragment or a nested XmlElement — the two
/// "where to insert a child" targets (same shape as `import.rs:229`).
enum XmlOpenable<'a> {
    Fragment(&'a yrs::XmlFragmentRef),
    Element(XmlElementRef),
}

/// Insert `node` at the end of `parent` and stamp it with a fresh
/// blockId. Every element except the root fragment carries one.
///
/// `parent_type` is `None` for the root fragment (which materializes
/// the schema's `Doc`); the debug assertion is the safety net behind
/// `enforce_containment` — if a walker change ever produced an invalid
/// pair, tests fail loudly rather than writing a corrupt document.
fn insert_block(
    txn: &mut yrs::TransactionMut<'_>,
    parent: &XmlOpenable<'_>,
    parent_type: NodeType,
    node: NodeType,
) -> XmlElementRef {
    // `valid_children()` is a block-containment predicate; it's
    // legitimately empty for text containers like `Paragraph` and
    // `Heading`, which have no *block* children. Inline leaves
    // (`NodeType::is_inline` — `HardBreak`, `Mention`, `DocMention`)
    // are a separate, orthogonal category it never enumerates, so they
    // must be exempted here rather than added to `valid_children`
    // itself (that would incorrectly claim a `Paragraph` can contain a
    // whole `HardBreak` *subtree*, which it can't — `HardBreak` is a
    // leaf). See `frontend/src/editor/schema.rs:60-78`
    // `content_matches` for the same special-case on the render side.
    debug_assert!(
        node.is_inline() || parent_type.valid_children().contains(&node),
        "schema containment violated: {:?} inside {:?}",
        node,
        parent_type
    );
    let prelim = XmlElementPrelim::empty(node.tag_name());
    let el = match parent {
        XmlOpenable::Fragment(f) => {
            let pos = f.len(txn);
            f.insert(txn, pos, prelim)
        }
        XmlOpenable::Element(e) => {
            let pos = e.len(txn);
            e.insert(txn, pos, prelim)
        }
    };
    el.insert_attribute(txn, "blockId", new_block_id());
    el
}

fn insert_text(txn: &mut yrs::TransactionMut<'_>, el: &XmlElementRef, text: &str) {
    if text.is_empty() {
        return;
    }
    let pos = el.len(txn);
    el.insert(txn, pos, XmlTextPrelim::new(text));
}

// Only test-side callers remain now that `materialize_block` splices
// `HardBreak` elements via `insert_spans` instead of concatenating
// `spans` into one flat string.
#[cfg(test)]
fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// Insert `spans` as `el`'s content, splicing in a real `HardBreak`
/// element wherever a span carries `hard_break_before` — mirrors
/// `import.rs:198-208` inserting a `HardBreak` into the open paragraph,
/// adapted for this walker's two-stage (parse-then-materialize)
/// pipeline. `container` is `el`'s own `NodeType`, passed through only
/// for `insert_block`'s containment check.
fn insert_spans(txn: &mut yrs::TransactionMut<'_>, el: &XmlElementRef, container: NodeType, spans: &[Span]) {
    let scope = XmlOpenable::Element(el.clone());
    for span in spans {
        if span.hard_break_before {
            insert_block(txn, &scope, container, NodeType::HardBreak);
        }
        insert_text(txn, el, &span.text);
    }
}

/// Build the yrs `Doc` from containment-clean blocks.
fn materialize(blocks: &[QuipBlock]) -> Doc {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");
        let root = XmlOpenable::Fragment(&fragment);
        for block in blocks {
            materialize_block(&mut txn, &root, NodeType::Doc, block);
        }
    }
    doc
}

fn materialize_block(
    txn: &mut yrs::TransactionMut<'_>,
    parent: &XmlOpenable<'_>,
    parent_type: NodeType,
    block: &QuipBlock,
) {
    match block {
        QuipBlock::Para { spans, .. } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Paragraph);
            insert_spans(txn, &el, NodeType::Paragraph, spans);
        }
        QuipBlock::Heading { level, spans, .. } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Heading);
            el.insert_attribute(txn, "level", (*level).clamp(1, 6).to_string());
            insert_spans(txn, &el, NodeType::Heading, spans);
        }
        QuipBlock::List { task, items, .. } => {
            let list_type = block.node_type();
            let item_type = if *task { NodeType::TaskItem } else { NodeType::ListItem };
            let list = insert_block(txn, parent, parent_type, list_type);
            for item in items {
                let li = insert_block(txn, &XmlOpenable::Element(list.clone()), list_type, item_type);
                if item_type == NodeType::TaskItem {
                    li.insert_attribute(txn, "checked", item.checked.unwrap_or(false).to_string());
                }
                let scope = XmlOpenable::Element(li);
                for child in &item.blocks {
                    materialize_block(txn, &scope, item_type, child);
                }
            }
        }
        QuipBlock::Quote { blocks } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Blockquote);
            let scope = XmlOpenable::Element(el);
            for child in blocks {
                materialize_block(txn, &scope, NodeType::Blockquote, child);
            }
        }
        QuipBlock::Code { language, text } => {
            let el = insert_block(txn, parent, parent_type, NodeType::CodeBlock);
            if !language.is_empty() {
                el.insert_attribute(txn, "language", language.clone());
            }
            insert_text(txn, &el, text);
        }
        QuipBlock::Rule => {
            insert_block(txn, parent, parent_type, NodeType::HorizontalRule);
        }
        QuipBlock::Table { rows } => {
            let table = insert_block(txn, parent, parent_type, NodeType::Table);
            for row in rows {
                let row_el = insert_block(
                    txn,
                    &XmlOpenable::Element(table.clone()),
                    NodeType::Table,
                    NodeType::TableRow,
                );
                for cell in &row.cells {
                    let cell_type =
                        if cell.header { NodeType::TableHeader } else { NodeType::TableCell };
                    let cell_el = insert_block(
                        txn,
                        &XmlOpenable::Element(row_el.clone()),
                        NodeType::TableRow,
                        cell_type,
                    );
                    let scope = XmlOpenable::Element(cell_el);
                    for child in &cell.blocks {
                        materialize_block(txn, &scope, cell_type, child);
                    }
                }
            }
        }
        QuipBlock::Image { src, alt } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Image);
            // Left as the raw Quip value on purpose — the blob
            // side-load pass rewrites it to a durable blob reference.
            el.insert_attribute(txn, "src", src.clone());
            if !alt.is_empty() {
                el.insert_attribute(txn, "alt", alt.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::ReadTxn;
    use yrs::types::GetString;
    use yrs::types::xml::XmlOut;

    fn blocks(html: &str) -> Vec<QuipBlock> {
        parse_quip(html)
    }

    // ─── blockId ─────────────────────────────────────────────

    #[test]
    fn block_ids_are_ten_alphanumeric_chars_and_unique() {
        let a = new_block_id();
        let b = new_block_id();
        assert_eq!(a.len(), 10, "blockId must be 10 chars: {a}");
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()), "alphanumeric only: {a}");
        assert_ne!(a, b, "ids must differ");
    }

    // ─── block structure ─────────────────────────────────────

    #[test]
    fn headings_lists_code_and_hr_parse() {
        let b = blocks(
            "<h2>Title</h2><ul><li>one</li><li>two</li></ul>\
             <pre data-language=\"rust\">fn x(){}</pre><hr>",
        );
        assert!(matches!(b[0], QuipBlock::Heading { level: 2, .. }), "{b:?}");
        assert!(matches!(b[1], QuipBlock::List { ordered: false, .. }), "{b:?}");
        assert!(matches!(b[2], QuipBlock::Code { ref language, .. } if language == "rust"), "{b:?}");
        assert!(matches!(b[3], QuipBlock::Rule), "{b:?}");
    }

    #[test]
    fn paragraph_and_ordered_list_parse() {
        let b = blocks("<p>hello</p><ol><li>first</li></ol>");
        let QuipBlock::Para { spans, .. } = &b[0] else { panic!("expected para: {b:?}") };
        assert_eq!(spans_text(spans), "hello");
        assert!(matches!(b[1], QuipBlock::List { ordered: true, task: false, .. }), "{b:?}");
    }

    #[test]
    fn unknown_tags_are_transparent_passthrough() {
        // The whole point of the tolerance: markup we didn't anticipate
        // keeps its content instead of vanishing.
        let b = blocks("<div class=\"quip-section\"><section><p>inside</p></section></div>");
        assert_eq!(b.len(), 1, "{b:?}");
        let QuipBlock::Para { spans, .. } = &b[0] else { panic!("expected para: {b:?}") };
        assert_eq!(spans_text(spans), "inside");
    }

    #[test]
    fn bare_text_becomes_a_paragraph() {
        let b = blocks("loose text");
        let QuipBlock::Para { spans, .. } = &b[0] else { panic!("expected para: {b:?}") };
        assert_eq!(spans_text(spans), "loose text");
    }

    #[test]
    fn script_and_style_never_reach_the_walker() {
        let b = blocks("<p>ok</p><script>alert(1)</script><style>p{}</style><iframe src=\"x\"></iframe>");
        assert_eq!(b.len(), 1, "only the paragraph survives: {b:?}");
        let QuipBlock::Para { spans, .. } = &b[0] else { panic!("expected para: {b:?}") };
        assert_eq!(spans_text(spans), "ok");
    }

    #[test]
    fn blockquote_wraps_its_children() {
        let b = blocks("<blockquote><p>quoted</p></blockquote>");
        let QuipBlock::Quote { blocks: inner } = &b[0] else { panic!("expected quote: {b:?}") };
        assert!(matches!(inner[0], QuipBlock::Para { .. }), "{inner:?}");
    }

    #[test]
    fn nested_lists_stay_inside_their_item() {
        let b = blocks("<ul><li>outer<ul><li>inner</li></ul></li></ul>");
        let QuipBlock::List { items, .. } = &b[0] else { panic!("expected list: {b:?}") };
        assert_eq!(items.len(), 1, "the inner list is not a sibling item: {items:?}");
        assert!(matches!(items[0].blocks[0], QuipBlock::Para { .. }));
        assert!(matches!(items[0].blocks[1], QuipBlock::List { .. }));
    }

    #[test]
    fn mixed_inline_and_block_content_keeps_document_order() {
        // Regression: the trailing text of an item used to land in
        // front of the block it followed.
        let b = blocks("<ul><li>a<ul><li>x</li></ul>b</li></ul>");
        let QuipBlock::List { items, .. } = &b[0] else { panic!("expected list: {b:?}") };
        let kinds: Vec<_> = items[0].blocks.iter().map(|x| x.node_type()).collect();
        assert_eq!(
            kinds,
            vec![NodeType::Paragraph, NodeType::BulletList, NodeType::Paragraph],
            "{:?}",
            items[0].blocks
        );
        let QuipBlock::Para { spans, .. } = &items[0].blocks[0] else { panic!() };
        assert_eq!(spans_text(spans), "a");
        let QuipBlock::Para { spans, .. } = &items[0].blocks[2] else { panic!() };
        assert_eq!(spans_text(spans), "b");
    }

    #[test]
    fn a_table_cell_keeps_its_blocks_in_order() {
        let b = blocks("<table><tr><td>a<pre>code</pre>b</td></tr></table>");
        let QuipBlock::Table { rows } = &b[0] else { panic!("expected table: {b:?}") };
        let kinds: Vec<_> = rows[0].cells[0].blocks.iter().map(|x| x.node_type()).collect();
        assert_eq!(
            kinds,
            vec![NodeType::Paragraph, NodeType::CodeBlock, NodeType::Paragraph],
            "{:?}",
            rows[0].cells[0].blocks
        );
    }

    #[test]
    fn a_paragraph_wrapping_an_image_hoists_it_after_the_text() {
        // A paragraph can't hold an image in this schema; the text
        // stays one paragraph and the image follows it.
        let b = blocks("<p>before<img src=\"http://x/i.png\">after</p>");
        let kinds: Vec<_> = b.iter().map(|x| x.node_type()).collect();
        assert_eq!(kinds, vec![NodeType::Paragraph, NodeType::Image], "{b:?}");
        let QuipBlock::Para { spans, .. } = &b[0] else { panic!() };
        assert_eq!(spans_text(spans), "before after");
    }

    #[test]
    fn an_image_alone_in_a_paragraph_leaves_no_ghost_paragraph() {
        let b = blocks("<p><img src=\"http://x/i.png\"></p>");
        assert_eq!(b.len(), 1, "{b:?}");
        assert!(matches!(b[0], QuipBlock::Image { .. }), "{b:?}");
    }

    #[test]
    fn an_empty_paragraph_is_kept_as_a_spacer() {
        let b = blocks("<p>a</p><p></p><p>b</p>");
        assert_eq!(b.len(), 3, "{b:?}");
        let QuipBlock::Para { spans, .. } = &b[1] else { panic!("expected para: {b:?}") };
        assert!(spans.is_empty());
    }

    #[test]
    fn inline_marks_keep_their_text_and_spacing() {
        let b = blocks("<p>a <b>bold</b> and <i>italic</i></p>");
        let QuipBlock::Para { spans, .. } = &b[0] else { panic!("expected para: {b:?}") };
        assert_eq!(spans_text(spans), "a bold and italic");
    }

    // ─── UNVERIFIED-MARKUP tolerances ────────────────────────

    #[test]
    fn checklist_items_carry_checked_state() {
        // Spelling 1: a checkbox input inside the item.
        let b = blocks(
            "<ul><li><input type=\"checkbox\" checked>done</li>\
             <li><input type=\"checkbox\">todo</li></ul>",
        );
        let QuipBlock::List { ordered: _, task, items } = &b[0] else { panic!("expected list") };
        assert!(*task, "a list whose items carry checkboxes is a task list");
        assert_eq!(items[0].checked, Some(true));
        assert_eq!(items[1].checked, Some(false));
    }

    #[test]
    fn checklist_state_from_item_classes() {
        // Spelling 2: class markers on the `<li>`.
        let b = blocks("<ul><li class=\"checked\">done</li><li class=\"unchecked\">todo</li></ul>");
        let QuipBlock::List { task, items, .. } = &b[0] else { panic!("expected list") };
        assert!(*task);
        assert_eq!(items[0].checked, Some(true));
        assert_eq!(items[1].checked, Some(false));
    }

    #[test]
    fn checklist_state_from_data_checked() {
        // Spelling 3: a data attribute.
        let b = blocks("<ul><li data-checked=\"true\">a</li><li data-checked=\"false\">b</li></ul>");
        let QuipBlock::List { task, items, .. } = &b[0] else { panic!("expected list") };
        assert!(*task);
        assert_eq!(items[0].checked, Some(true));
        assert_eq!(items[1].checked, Some(false));
    }

    #[test]
    fn checklist_marked_on_the_list_defaults_items_to_unchecked() {
        // Spelling 4: only the list says "checklist"; items say nothing.
        for html in [
            "<ul class=\"checklist\"><li>a</li></ul>",
            "<ul data-type=\"taskList\"><li>a</li></ul>",
        ] {
            let b = blocks(html);
            let QuipBlock::List { task, items, .. } = &b[0] else { panic!("expected list: {html}") };
            assert!(*task, "{html}");
            assert_eq!(items[0].checked, Some(false), "{html}");
        }
    }

    #[test]
    fn a_plain_bullet_list_is_not_a_task_list() {
        let b = blocks("<ul><li>a</li></ul>");
        let QuipBlock::List { task, items, .. } = &b[0] else { panic!("expected list") };
        assert!(!*task);
        assert_eq!(items[0].checked, None);
    }

    #[test]
    fn code_language_alternate_spellings() {
        let cases = [
            ("<pre data-language=\"rust\">x</pre>", "rust"),
            ("<pre class=\"language-python\">x</pre>", "python"),
            ("<pre class=\"lang-go\">x</pre>", "go"),
            ("<pre><code class=\"language-ruby\">x</code></pre>", "ruby"),
            ("<pre>x</pre>", ""),
        ];
        for (html, want) in cases {
            let b = blocks(html);
            let QuipBlock::Code { language, .. } = &b[0] else { panic!("expected code: {html}") };
            assert_eq!(language, want, "{html}");
        }
    }

    #[test]
    fn code_block_keeps_its_text_verbatim() {
        let b = blocks("<pre><code>let a = 1;\nlet b = 2;</code></pre>");
        let QuipBlock::Code { text, .. } = &b[0] else { panic!("expected code") };
        assert_eq!(text, "let a = 1;\nlet b = 2;");
    }

    #[test]
    fn section_ids_are_captured_verbatim() {
        // Quip section ids are opaque; no charset assumptions.
        let b = blocks("<p id=\"XYZ:abc-123\">a</p><h1 data-section-id=\"S2\">b</h1>");
        let QuipBlock::Para { section_id, .. } = &b[0] else { panic!("expected para: {b:?}") };
        assert_eq!(section_id.as_deref(), Some("XYZ:abc-123"));
        let QuipBlock::Heading { section_id, .. } = &b[1] else { panic!("expected heading: {b:?}") };
        assert_eq!(section_id.as_deref(), Some("S2"));
    }

    // ─── tables ──────────────────────────────────────────────

    #[test]
    fn table_rows_and_header_cells_parse() {
        let b = blocks("<table><tr><th>H</th></tr><tr><td>C</td></tr></table>");
        let QuipBlock::Table { rows } = &b[0] else { panic!("expected table") };
        assert_eq!(rows.len(), 2);
        assert!(rows[0].cells[0].header, "th -> header cell");
        assert!(!rows[1].cells[0].header);
    }

    #[test]
    fn thead_tbody_are_transparent() {
        let b = blocks(
            "<table><thead><tr><th>H</th></tr></thead>\
             <tbody><tr><td>C</td></tr></tbody></table>",
        );
        let QuipBlock::Table { rows } = &b[0] else { panic!("expected table") };
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows[0].cells[0].header);
    }

    // ─── containment / hoisting ──────────────────────────────

    #[test]
    fn materialized_tree_obeys_schema_containment() {
        // An image inside a list item is illegal per schema::valid_children;
        // it must be hoisted rather than emitted invalidly.
        let out = from_quip_html("<ul><li>text<img src=\"http://x/i.png\"></li></ul>");
        assert_valid_tree(&out.doc);
        let xml = crate::export::to_html(&out.doc);
        assert!(xml.contains("<ul"), "list survives: {xml}");
        assert!(!xml.contains("<li><img"), "image must not be a direct list-item child: {xml}");
        assert!(xml.contains("<img"), "image content is preserved: {xml}");
    }

    #[test]
    fn hoisting_splits_the_list_and_preserves_document_order() {
        let b = blocks(
            "<ul><li>a</li><li>b<img src=\"i.png\"></li><li>c</li></ul>",
        );
        assert_eq!(b.len(), 3, "list / image / list: {b:?}");
        let QuipBlock::List { items, .. } = &b[0] else { panic!("expected list: {b:?}") };
        assert_eq!(items.len(), 2, "items up to and including the offender: {items:?}");
        assert!(matches!(b[1], QuipBlock::Image { .. }), "{b:?}");
        let QuipBlock::List { items, .. } = &b[2] else { panic!("expected list: {b:?}") };
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn table_and_rule_in_a_list_item_are_hoisted() {
        let b = blocks("<ul><li>a<table><tr><td>c</td></tr></table><hr></li></ul>");
        let kinds: Vec<_> = b.iter().map(|x| x.node_type()).collect();
        assert_eq!(
            kinds,
            vec![NodeType::BulletList, NodeType::Table, NodeType::HorizontalRule],
            "{b:?}"
        );
    }

    #[test]
    fn image_in_a_table_cell_is_hoisted_after_the_table() {
        let out = from_quip_html("<table><tr><td>c<img src=\"i.png\"></td></tr></table>");
        assert_valid_tree(&out.doc);
        let b = blocks("<table><tr><td>c<img src=\"i.png\"></td></tr></table>");
        let kinds: Vec<_> = b.iter().map(|x| x.node_type()).collect();
        assert_eq!(kinds, vec![NodeType::Table, NodeType::Image], "{b:?}");
    }

    #[test]
    fn nested_table_in_a_cell_is_hoisted() {
        let out = from_quip_html(
            "<table><tr><td>outer<table><tr><td>inner</td></tr></table></td></tr></table>",
        );
        assert_valid_tree(&out.doc);
    }

    #[test]
    fn an_item_emptied_by_hoisting_still_has_a_paragraph() {
        let b = blocks("<ul><li><img src=\"i.png\"></li></ul>");
        let QuipBlock::List { items, .. } = &b[0] else { panic!("expected list: {b:?}") };
        assert_eq!(items[0].blocks, vec![empty_para()], "{items:?}");
    }

    #[test]
    fn a_gnarly_document_materializes_to_a_valid_tree() {
        let out = from_quip_html(
            "<h1 id=\"s1\">T</h1>\
             <div><p>intro <b>bold</b></p></div>\
             <ul class=\"checklist\"><li class=\"checked\">done<img src=\"http://x/a.png\">\
             <ul><li>nested<hr></li></ul></li></ul>\
             <blockquote><p>q</p><table><tr><th>h</th></tr></table></blockquote>\
             <table><tr><td><pre class=\"lang-rs\">code</pre>\
             <img src=\"http://x/b.png\"></td></tr></table>\
             <hr>",
        );
        assert_valid_tree(&out.doc);
        // Nothing was dropped: both images survive somewhere.
        let xml = crate::export::to_html(&out.doc);
        assert!(xml.contains("a.png"), "{xml}");
        assert!(xml.contains("b.png"), "{xml}");
    }

    // ─── materialization ─────────────────────────────────────

    #[test]
    #[should_panic(expected = "schema containment violated")]
    fn the_materializer_refuses_an_invalid_pair() {
        // Proves the safety net behind `enforce_containment` is live:
        // if a future walker change ever hands `materialize` an illegal
        // nesting, debug builds (i.e. the test suite) fail loudly
        // instead of writing a corrupt document.
        materialize(&[QuipBlock::List {
            ordered: false,
            task: false,
            items: vec![QuipItem {
                checked: None,
                blocks: vec![QuipBlock::Image { src: "x".into(), alt: String::new() }],
            }],
        }]);
    }

    #[test]
    fn br_produces_a_hard_break_element_not_a_literal_newline() {
        // The old (wrong) behavior pushed "\n" into the paragraph's
        // text, justified by `Paragraph::valid_children()` being empty.
        // That predicate governs block containment only; `HardBreak` is
        // an inline leaf (`NodeType::is_inline`), an orthogonal
        // category it never enumerates. Assert both halves of the fix:
        // a real `HardBreak` element sits between the two text runs,
        // and no text run ever carries a raw '\n'.
        let out = from_quip_html("<p>a<br>b</p>");
        let txn = out.doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        assert_eq!(frag.len(&txn), 1, "one paragraph");
        let Some(XmlOut::Element(para)) = frag.get(&txn, 0) else { panic!("expected an element") };
        assert_eq!(NodeType::from_tag(para.tag().as_ref()), Some(NodeType::Paragraph));

        let mut seen = Vec::new();
        for i in 0..para.len(&txn) {
            match para.get(&txn, i) {
                Some(XmlOut::Text(t)) => {
                    let s = t.get_string(&txn);
                    assert!(!s.contains('\n'), "text run carries a literal newline: {s:?}");
                    seen.push(format!("text:{s}"));
                }
                Some(XmlOut::Element(el)) => {
                    assert_eq!(NodeType::from_tag(el.tag().as_ref()), Some(NodeType::HardBreak));
                    assert!(
                        el.get_attribute(&txn, "blockId").is_some_and(|id| id.len() == 10),
                        "HardBreak gets a blockId like any other element"
                    );
                    seen.push("hard_break".to_string());
                }
                _ => {}
            }
        }
        assert_eq!(seen, vec!["text:a", "hard_break", "text:b"], "{seen:?}");
    }

    #[test]
    fn br_exports_as_a_real_br_tag_not_a_newline() {
        let out = from_quip_html("<p>a<br>b</p>");
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("<br"), "HardBreak must export as <br>: {html}");
        assert!(!html.contains("a\nb"), "no literal newline leaked into the exported text: {html}");
    }

    #[test]
    fn every_element_gets_a_block_id() {
        let out = from_quip_html("<p>a</p><h1>b</h1><ul><li>c</li></ul>");
        let txn = out.doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        assert_eq!(frag.len(&txn), 3, "three top-level blocks");
        let mut seen = std::collections::HashSet::new();
        for_each_element(&txn, &frag, &mut |txn, el| {
            let id = el.get_attribute(txn, "blockId").unwrap_or_default();
            assert_eq!(id.len(), 10, "blockId on <{}>: {id:?}", el.tag());
            assert!(id.chars().all(|c| c.is_ascii_alphanumeric()), "blockId charset: {id}");
            assert!(seen.insert(id.clone()), "blockId {id} reused");
        });
        assert!(seen.len() >= 5, "paragraph + heading + list + item + item paragraph");
    }

    #[test]
    fn materialized_attributes_match_the_editor_schema() {
        let out = from_quip_html(
            "<h3>H</h3><pre data-language=\"rust\">x</pre>\
             <ul><li><input type=\"checkbox\" checked>t</li></ul>\
             <img src=\"http://x/i.png\" alt=\"pic\">",
        );
        let xml = crate::export::to_html(&out.doc);
        assert!(xml.contains("<h3"), "{xml}");
        assert!(xml.contains("language-rust"), "{xml}");
        assert!(xml.contains("data-checked=\"true\""), "{xml}");
        assert!(xml.contains("alt=\"pic\""), "{xml}");
        assert!(xml.contains("src=\"http://x/i.png\""), "raw src is preserved: {xml}");
    }

    #[test]
    fn empty_input_yields_an_empty_document() {
        let out = from_quip_html("");
        let txn = out.doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        assert_eq!(frag.len(&txn), 0);
    }

    #[test]
    fn side_tables_are_empty_in_this_slice() {
        let out = from_quip_html("<p id=\"s1\">a</p><img src=\"i.png\">");
        assert!(out.sections.is_empty());
        assert!(out.images.is_empty());
        assert!(out.pending_links.is_empty());
    }

    // ─── helpers ─────────────────────────────────────────────

    fn for_each_element<T: ReadTxn>(
        txn: &T,
        frag: &yrs::XmlFragmentRef,
        f: &mut impl FnMut(&T, &XmlElementRef),
    ) {
        for i in 0..frag.len(txn) {
            if let Some(XmlOut::Element(el)) = frag.get(txn, i) {
                visit_element(txn, &el, f);
            }
        }
    }

    fn visit_element<T: ReadTxn>(
        txn: &T,
        el: &XmlElementRef,
        f: &mut impl FnMut(&T, &XmlElementRef),
    ) {
        f(txn, el);
        for i in 0..el.len(txn) {
            if let Some(XmlOut::Element(child)) = el.get(txn, i) {
                visit_element(txn, &child, f);
            }
        }
    }

    /// Assert that every parent/child pair in the document is legal per
    /// `NodeType::valid_children`, and that every element carries a
    /// blockId. This is the invariant an invalid tree would break —
    /// downstream, an invalid tree is document corruption.
    fn assert_valid_tree(doc: &Doc) {
        let txn = doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        for i in 0..frag.len(&txn) {
            let Some(XmlOut::Element(el)) = frag.get(&txn, i) else { continue };
            check_subtree(&txn, &el, NodeType::Doc);
        }
    }

    fn check_subtree<T: ReadTxn>(txn: &T, el: &XmlElementRef, parent: NodeType) {
        let tag = el.tag().to_string();
        let nt = NodeType::from_tag(&tag).unwrap_or_else(|| panic!("unknown tag {tag}"));
        assert!(
            parent.valid_children().contains(&nt),
            "{nt:?} is not a legal child of {parent:?}"
        );
        assert!(
            el.get_attribute(txn, "blockId").is_some_and(|id| id.len() == 10),
            "missing blockId on <{tag}>"
        );
        for i in 0..el.len(txn) {
            match el.get(txn, i) {
                Some(XmlOut::Element(child)) => check_subtree(txn, &child, nt),
                Some(XmlOut::Text(t)) => {
                    // Text is only legal where the schema expects a text
                    // container; a list/table container must not hold it.
                    let _ = t.get_string(txn);
                    assert!(
                        !matches!(
                            nt,
                            NodeType::BulletList
                                | NodeType::OrderedList
                                | NodeType::TaskList
                                | NodeType::Table
                                | NodeType::TableRow
                        ),
                        "raw text inside {nt:?}"
                    );
                }
                _ => {}
            }
        }
    }
}
