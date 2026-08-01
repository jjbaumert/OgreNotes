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
//!   - [`quip_thread_from_url`] — an `<a href>` being an intra-Quip
//!     document link, and where the thread id / section anchor sit
//!     inside it
//!
//! Inline *marks*, by contrast, are ordinary HTML (`<b>`, `<em>`,
//! `<a href>`, …) and are read directly — with every spelling variant
//! accepted (`b`/`strong`, `i`/`em`, `s`/`del`/`strike`).
//!
//! Everything else degrades gracefully by construction: unknown tags
//! are transparent passthrough (their children are walked in the same
//! context) rather than dropped, exactly as `import.rs:417-423` does.

use std::collections::{HashMap, HashSet};

use yrs::{
    Any, Doc, ReadTxn, Text, Transact, WriteTxn, Xml, XmlElementRef,
    types::Attrs,
    types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim, XmlTextRef},
};

use crate::schema::{MarkType, NodeType};

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
    /// How many subtrees were flattened for exceeding
    /// [`MAX_NESTING_DEPTH`](crate::import_quip::MAX_NESTING_DEPTH).
    ///
    /// Non-zero means the document nested deeper than the walker will
    /// descend: the text survived, the structure below that point did not.
    /// A named loss, so the caller reports it the way it reports a dropped
    /// image rather than letting it pass silently.
    pub deep_nesting_truncated: usize,
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

/// Import a Quip HTML body into a fresh `Doc`, together with the
/// side-tables the caller needs to finish the job.
pub fn from_quip_html(html: &str) -> QuipDocument {
    let (blocks, truncated) = parse_quip_counting_truncations(html);
    QuipDocument { deep_nesting_truncated: truncated, ..materialize(&blocks) }
}

// ─── intermediate block model ────────────────────────────────────

/// The inline formatting active over a run of text.
///
/// Deliberately a subset of `MarkType` (`schema.rs:311`): `subscript`,
/// `superscript` and `mention` are absent because no Quip spelling for
/// them has been observed. `<sub>` / `<sup>` / `<mark>` therefore stay
/// transparent passthrough — their *text* survives, their formatting
/// does not. That is a known, recorded loss, not an oversight.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    /// `href` of an enclosing `<a>` that is *not* an intra-Quip link —
    /// those become a `DocMention` placeholder instead (see
    /// [`quip_thread_from_url`]).
    pub link: Option<String>,
}

/// An intra-Quip document link. Materialized as a placeholder
/// `DocMention` inline leaf whose `doc_id` is empty until Phase 2b
/// learns the id of the OgreNotes document the target thread imported
/// into, and recorded in `QuipDocument::pending_links` so that
/// back-patch can find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuipMention {
    pub thread_id: String,
    pub section_id: Option<String>,
    /// The original href. Kept on the placeholder so an un-back-patched
    /// chip still links *somewhere* (back to Quip) rather than nowhere.
    pub url: String,
}

/// A run of inline content: text plus the marks covering it, or — when
/// `mention` is set — an intra-Quip link placeholder whose `text` is
/// the anchor's label.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Span {
    pub text: String,
    /// Inline formatting covering the whole run. A run is split
    /// wherever the mark set changes, so this is uniform by
    /// construction and materializes as a single yrs `format` call.
    pub marks: Marks,
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
    /// Set when this run *is* an intra-Quip link: it materializes as a
    /// `DocMention` element, not as a text run.
    pub mention: Option<QuipMention>,
}

impl Span {
    fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Self::default() }
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
        /// Quip section anchor, if the source carried one. Read by the
        /// SECMAP pass in `materialize`, which pairs it with the
        /// blockId minted for this block.
        section_id: Option<String>,
        spans: Vec<Span>,
    },
    Heading {
        level: u8,
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
/// when their formatting has no mark to map onto (`sub`, `sup`,
/// `mark`). Deliberately wider than
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
        "strong", "em", "b", "i", "u", "s", "del", "strike", "sub", "sup", "mark",
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

/// Deepest element nesting the walker is ever allowed to descend.
///
/// **This is a process-liveness bound, not a formatting preference.** The
/// block walker (`walk_node` -> `walk_children` -> `walk_element`) recurses
/// once per DOM level, and the HTML it consumes is authored entirely by a
/// third party: Quip returns whatever `/2/threads/{id}/html` holds. Exhaust
/// the thread stack and Rust **aborts the process** — stack overflow is not a
/// panic, so no `catch_unwind` anywhere can contain it. On the shared import
/// worker that kills every concurrent job, not just the offending import, and
/// the re-run reaches the same document and dies again.
///
/// Measured abort threshold on a 2 MiB stack — what tokio worker threads and
/// Rust test threads both get — is ~1 050 levels of nested
/// `<div>`/`<span>`/`<blockquote>`. (`ammonia::clean` and html5ever survive
/// 8 000+; the recursion that fails is ours.) 128 leaves roughly 8x headroom
/// for a smaller stack or a deeper call path above the walker, and sits far
/// above anything an authored document reaches: Quip's editor tops out around
/// a dozen levels of nested list, and even paste-artifact wrapper
/// accumulation runs to tens, not hundreds.
pub const MAX_NESTING_DEPTH: usize = 128;

/// Parse Quip HTML into the intermediate block model. Pure: no yrs
/// transaction is live while the DOM is walked.
pub(crate) fn parse_quip(html: &str) -> Vec<QuipBlock> {
    parse_quip_counting_truncations(html).0
}

/// [`parse_quip`] plus the number of subtrees that were flattened for
/// exceeding [`MAX_NESTING_DEPTH`]. Split out so `parse_quip` keeps the
/// signature its unit tests use while [`from_quip_html`] can surface the loss
/// on [`QuipDocument`] and the importer can report it.
pub(crate) fn parse_quip_counting_truncations(html: &str) -> (Vec<QuipBlock>, usize) {
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::RcDom;

    let safe = sanitize(html);
    let dom: RcDom = html5ever::parse_document(RcDom::default(), html5ever::driver::ParseOpts::default())
        .from_utf8()
        .read_from(&mut safe.as_bytes())
        .expect("html5ever parse is infallible on bytes");

    // Bound the tree BEFORE the recursive walk rather than threading a depth
    // counter through all twelve `walk_children` call sites. The guarantee is
    // then structural: whatever the walker receives is already shallower than
    // MAX_NESTING_DEPTH, so every recursive pass over it — `walk_element`,
    // `enforce_containment`, `materialize_block` — is bounded by the same
    // constant for free, and no future recursive pass can forget to check.
    let truncated = flatten_below_depth(&dom.document);

    let mut out = Vec::new();
    let mut pending = InlineBuf::default();
    walk_children(&dom.document, &mut out, &Marks::default(), &mut pending);
    pending.flush(&mut out, None);

    (enforce_containment(out, NodeType::Doc), truncated)
}

/// Replace every subtree rooted deeper than [`MAX_NESTING_DEPTH`] with a
/// single text node holding that subtree's flattened text, returning how many
/// subtrees were flattened.
///
/// **Fails soft: text survives, only the nesting is lost.** Dropping the
/// subtree would silently delete the deepest content of a document, and
/// returning an error would wedge the thread on every retry — the importer
/// already treats "keep the content, record the loss" as the right answer for
/// an unfetchable image, and over-deep nesting is the same shape of problem.
///
/// **Iterative, with an explicit stack, in both halves.** A recursive
/// implementation of the guard against unbounded recursion would abort on
/// exactly the input it exists to survive.
fn flatten_below_depth(document: &markup5ever_rcdom::Handle) -> usize {
    use markup5ever_rcdom::{Node, NodeData};

    let mut truncated = 0usize;
    // (node, depth) pairs still to inspect.
    let mut stack: Vec<(markup5ever_rcdom::Handle, usize)> = vec![(document.clone(), 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth >= MAX_NESTING_DEPTH {
            let children = std::mem::take(&mut *node.children.borrow_mut());
            if children.is_empty() {
                continue;
            }
            truncated += 1;
            let text = collect_text(&children);
            if !text.is_empty() {
                let replacement = Node::new(NodeData::Text {
                    contents: std::cell::RefCell::new(text.into()),
                });
                node.children.borrow_mut().push(replacement);
            }
            continue;
        }
        for child in node.children.borrow().iter() {
            stack.push((child.clone(), depth + 1));
        }
    }
    truncated
}

/// Concatenate every text node in `roots` and their descendants, separated by
/// single spaces. Iterative for the same reason [`flatten_below_depth`] is.
fn collect_text(roots: &[markup5ever_rcdom::Handle]) -> String {
    use markup5ever_rcdom::NodeData;

    let mut text = String::new();
    let mut stack: Vec<markup5ever_rcdom::Handle> = roots.iter().rev().cloned().collect();
    while let Some(node) = stack.pop() {
        if let NodeData::Text { contents } = &node.data {
            let borrowed = contents.borrow();
            let chunk = borrowed.as_ref().trim();
            if !chunk.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(chunk);
            }
        }
        for child in node.children.borrow().iter().rev() {
            stack.push(child.clone());
        }
    }
    text
}

/// Inline text accumulated between block boundaries. Flushed as a
/// paragraph when a block element interrupts it or the parent closes.
#[derive(Default)]
struct InlineBuf {
    spans: Vec<Span>,
}

impl InlineBuf {
    /// Append text carrying `marks`. Merges into the run in progress
    /// only when the mark set is identical (and that run is real text,
    /// not a mention placeholder) — that split-on-change is what makes
    /// every `Span` uniformly formatted, so materializing is one yrs
    /// `format` call per span.
    fn push_text(&mut self, s: &str, marks: &Marks) {
        match self.spans.last_mut() {
            Some(last) if last.mention.is_none() && last.marks == *marks => last.text.push_str(s),
            _ => self.spans.push(Span { text: s.to_string(), marks: marks.clone(), ..Span::default() }),
        }
    }

    /// Record a `<br>`. Pushes a fresh, empty span flagged
    /// `hard_break_before` so the next `push_text` call appends *after*
    /// the break rather than into the run that preceded it; two
    /// consecutive breaks therefore produce two distinct empty spans,
    /// which `materialize_block` turns into two distinct `HardBreak`
    /// elements — matching the real DOM's `<br><br>`.
    fn push_break(&mut self) {
        self.spans.push(Span { hard_break_before: true, ..Span::default() });
    }

    /// Record an intra-Quip link as its own run. `label` is the
    /// anchor's text, which becomes the placeholder's `title`.
    fn push_mention(&mut self, mention: QuipMention, label: String) {
        self.spans.push(Span { text: label, mention: Some(mention), ..Span::default() });
    }

    /// Emit the buffer as a paragraph (dropping it when it holds only
    /// whitespace) and reset. A mention placeholder counts as content
    /// even when its label is empty — dropping the paragraph would take
    /// the pending link with it.
    fn flush(&mut self, out: &mut Vec<QuipBlock>, section_id: Option<String>) {
        let spans = std::mem::take(&mut self.spans);
        if spans.iter().all(|s| s.text.trim().is_empty() && s.mention.is_none()) {
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
    // An empty span still marks a `<br>` or an intra-Quip link —
    // dropping it here would silently eat a hard break or a pending
    // link with an empty label.
    spans.retain(|s| !s.text.is_empty() || s.hard_break_before || s.mention.is_some());
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
            | "strike"
            | "code"
            | "sub"
            | "sup"
            | "mark"
            | "input"
            | "br"
    )
}

fn walk_children(
    handle: &markup5ever_rcdom::Handle,
    out: &mut Vec<QuipBlock>,
    marks: &Marks,
    pending: &mut InlineBuf,
) {
    for child in handle.children.borrow().iter() {
        walk_node(child, out, marks, pending);
    }
}

fn walk_node(
    handle: &markup5ever_rcdom::Handle,
    out: &mut Vec<QuipBlock>,
    marks: &Marks,
    pending: &mut InlineBuf,
) {
    use markup5ever_rcdom::NodeData;

    match &handle.data {
        NodeData::Document => walk_children(handle, out, marks, pending),
        NodeData::Text { contents } => {
            let s = contents.borrow();
            let raw = s.as_ref();
            if raw.trim().is_empty() {
                // Whitespace between block tags is layout, not content
                // — but whitespace *inside* a run separates words.
                if !pending.spans.is_empty() && !raw.is_empty() {
                    pending.push_text(" ", marks);
                }
                return;
            }
            pending.push_text(raw, marks);
        }
        NodeData::Element { name, .. } => {
            let tag = name.local.as_ref().to_ascii_lowercase();
            walk_element(handle, &tag, out, marks, pending);
        }
        _ => {
            // Comments, doctype, processing instructions: dropped.
        }
    }
}

/// Descend into an inline element with one more mark switched on.
fn walk_marked(
    handle: &markup5ever_rcdom::Handle,
    out: &mut Vec<QuipBlock>,
    marks: &Marks,
    pending: &mut InlineBuf,
    set: impl FnOnce(&mut Marks),
) {
    let mut inner = marks.clone();
    set(&mut inner);
    walk_children(handle, out, &inner, pending);
}

fn walk_element(
    handle: &markup5ever_rcdom::Handle,
    tag: &str,
    out: &mut Vec<QuipBlock>,
    marks: &Marks,
    pending: &mut InlineBuf,
) {
    match tag {
        // Scaffolding html5ever inserts — descend transparently.
        "html" | "body" => walk_children(handle, out, marks, pending),
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
            walk_children(handle, &mut inner, &Marks::default(), &mut buf);
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
            // paragraph in progress — marks covering it are therefore
            // dropped, since a mark is a yrs *text* attribute and this
            // is an element. The `src` stays the raw Quip value until
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
        // ── inline marks ──
        // Note `code` here is the *inline* one: `<pre><code>` never
        // reaches this arm because the `pre` branch above consumes the
        // subtree verbatim (a code block carries no marks by schema —
        // `NodeType::is_code`).
        "b" | "strong" => walk_marked(handle, out, marks, pending, |m| m.bold = true),
        "i" | "em" => walk_marked(handle, out, marks, pending, |m| m.italic = true),
        "u" => walk_marked(handle, out, marks, pending, |m| m.underline = true),
        "s" | "del" | "strike" => walk_marked(handle, out, marks, pending, |m| m.strike = true),
        "code" => walk_marked(handle, out, marks, pending, |m| m.code = true),
        "a" => walk_anchor(handle, out, marks, pending),
        _ if is_inline_tag(tag) => walk_children(handle, out, marks, pending),
        // Unknown / structural tag: transparent passthrough. The
        // children are walked in the *same* context, so a `<div>`
        // wrapper neither creates a block nor breaks a paragraph.
        _ => walk_children(handle, out, marks, pending),
    }
}

/// An `<a href>`: either an intra-Quip document link (→ a `DocMention`
/// placeholder plus a pending-link record) or an ordinary link, which
/// passes through untouched as a `Link` mark over its text.
///
/// An intra-Quip anchor's *block* children (an image inside a link,
/// say) are hoisted out and kept exactly as `<p><img></p>` is; only its
/// text collapses into the placeholder's label.
fn walk_anchor(
    handle: &markup5ever_rcdom::Handle,
    out: &mut Vec<QuipBlock>,
    marks: &Marks,
    pending: &mut InlineBuf,
) {
    let href = attr(handle, "href").unwrap_or_default();
    if let Some(mention) = quip_thread_from_url(&href) {
        let (spans, rest) = walk_text_container(handle);
        let label: String = spans.iter().map(|s| s.text.as_str()).collect();
        pending.push_mention(mention, label.trim().to_string());
        if !rest.is_empty() {
            // Emitting a block closes the paragraph in progress first —
            // the same move every other block-emitting arm makes (see
            // the `img` and `p` arms). Without it the hoisted block
            // lands *before* the text that preceded the link.
            pending.flush(out, None);
            out.extend(rest);
        }
        return;
    }
    let mut inner = marks.clone();
    if !href.trim().is_empty() {
        inner.link = Some(href);
    }
    walk_children(handle, out, &inner, pending);
}

/// Walk a text container (`<p>`, `<h1>`…`<h6>`): all of its text folds
/// into one block's spans, and any *block* it wrapped is returned to be
/// emitted after it. A paragraph can't hold an image or a table in this
/// schema, so hoisting is the only lossless option.
fn walk_text_container(handle: &markup5ever_rcdom::Handle) -> (Vec<Span>, Vec<QuipBlock>) {
    let mut nested = Vec::new();
    let mut buf = InlineBuf::default();
    walk_children(handle, &mut nested, &Marks::default(), &mut buf);
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
    walk_children(handle, &mut blocks, &Marks::default(), &mut buf);
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
        walk_children(child, &mut blocks, &Marks::default(), &mut buf);
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

/// **UNVERIFIED MARKUP.** Recognize an intra-Quip document link and
/// split out `(thread_id, section_id)`.
///
/// Assumed shape: `https://<subdomain>.quip.com/<THREAD_ID>/<slug>#<section>`
/// — thread id is the first non-empty path segment, and the fragment
/// (when present) is the target section anchor. `https://quip.com/ID`
/// and a fragment-less URL are both accepted, as is the **relative**
/// form (`/AbCd1234/Some-Doc`): see [`resolve_href`].
///
/// The returned `QuipMention::url` is the **resolved absolute** URL,
/// not the source href — see [`resolve_href`] for why that matters.
///
/// Returns `None` for everything else, which is what makes an ordinary
/// external link stay an ordinary link. The one known imprecision:
/// a **Quip host on a non-document path** (`/blob/...`) has a first
/// path segment but is not a thread. Phase 2b's back-patch is keyed on
/// the thread id existing in the inventory, so a bogus id resolves to
/// nothing, gets *reported* as an unresolved link, and the placeholder
/// keeps its url. That failure is loud, which is the property this
/// helper optimizes for.
fn quip_thread_from_url(href: &str) -> Option<QuipMention> {
    let url = resolve_href(href)?;
    if !is_quip_host(url.host_str()?) {
        return None;
    }
    let thread_id = url.path_segments()?.find(|s| !s.is_empty())?.to_string();
    let section_id = url.fragment().filter(|f| !f.is_empty()).map(str::to_string);
    Some(QuipMention { thread_id, section_id, url: url.to_string() })
}

/// The base a *relative* href in a Quip thread body is relative to.
///
/// This is not a guess about the markup: `from_quip_html`'s input is a
/// thread body fetched from Quip's `/2` API, so a relative href in it
/// is relative to the Quip site by construction. There is no path
/// through the importer by which `/AbCd1234/Some-Doc` could mean "a
/// link into OgreNotes".
const QUIP_BASE: &str = "https://quip.com/";

/// Parse an href, falling back to resolution against [`QUIP_BASE`].
///
/// **Leaving a relative href unresolved is not the safe default.**
/// `export::is_safe_url` (`export.rs:1572-1581`) explicitly accepts a
/// leading `/`, so an unclassified relative Quip href would export as
/// `<a href="/AbCd1234/Some-Doc">` — a live link into *OgreNotes' own
/// origin* that 404s. Resolving it makes the worst case an
/// over-classified pending link, which Phase 2b reports as unresolved.
///
/// The same reasoning is already baked into this changeset elsewhere:
/// the design's image assumption is `<img src="/blob/{thread}/{blob}">`,
/// also relative. Believing Quip relativizes blobs but not documents
/// would be an unargued inconsistency inside one assumption set.
fn resolve_href(href: &str) -> Option<url::Url> {
    let href = href.trim();
    if let Ok(absolute) = url::Url::parse(href) {
        return Some(absolute);
    }
    let base = url::Url::parse(QUIP_BASE).ok()?;
    url::Url::options().base_url(Some(&base)).parse(href).ok()
}

/// `quip.com` itself or any subdomain of it. Deliberately *not*
/// `ends_with("quip.com")`, which would also accept `notquip.com` and
/// hand an attacker-controlled host a pending-link record. Reading the
/// host off a parsed `Url` (rather than string-matching the href) is
/// also what defeats `https://quip.com@evil.example/` userinfo
/// spoofing.
fn is_quip_host(host: &str) -> bool {
    // A trailing dot is the fully-qualified form of the same host.
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "quip.com" || host.ends_with(".quip.com")
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

/// Append `text` as a new text run. Returns the handle so the caller
/// can format it; `None` for empty text (yrs would create an empty,
/// unformattable node).
fn insert_text(
    txn: &mut yrs::TransactionMut<'_>,
    el: &XmlElementRef,
    text: &str,
) -> Option<XmlTextRef> {
    if text.is_empty() {
        return None;
    }
    let pos = el.len(txn);
    Some(el.insert(txn, pos, XmlTextPrelim::new(text)))
}

/// Apply `marks` across the whole of `text`.
///
/// Encoding is the one `export::to_html` decodes (`export.rs:752-776`,
/// mirrored by `diff::attrs_to_marks`): a boolean mark is
/// `Any::Bool(true)` under `MarkType::attr_name()`, and the link mark
/// is a **JSON string** payload `{"href": "…"}`. Getting the shape
/// wrong is silently lossy, so the round trip is asserted by test.
///
/// The range is always the entire node: `insert_spans` writes one text
/// node per uniformly-marked run, so `0..len(txn)` needs no offset
/// arithmetic and can't disagree with the document's offset kind.
fn apply_marks(txn: &mut yrs::TransactionMut<'_>, text: &XmlTextRef, marks: &Marks) {
    let mut attrs = Attrs::new();
    for (on, mark) in [
        (marks.bold, MarkType::Bold),
        (marks.italic, MarkType::Italic),
        (marks.underline, MarkType::Underline),
        (marks.strike, MarkType::Strike),
        (marks.code, MarkType::Code),
    ] {
        if on {
            attrs.insert(std::sync::Arc::from(mark.attr_name()), Any::Bool(true));
        }
    }
    if let Some(href) = &marks.link {
        let payload = serde_json::json!({ "href": href }).to_string();
        attrs.insert(std::sync::Arc::from(MarkType::Link.attr_name()), Any::String(payload.into()));
    }
    if attrs.is_empty() {
        return;
    }
    let len = text.len(&*txn);
    if len == 0 {
        return;
    }
    text.format(txn, 0, len, attrs);
}

/// The blockId `insert_block` just minted for `el`. Read back rather
/// than returned so the minting path itself stays untouched.
fn block_id_of<T: ReadTxn>(txn: &T, el: &XmlElementRef) -> String {
    el.get_attribute(txn, "blockId").unwrap_or_default()
}

/// The side-tables accumulated while materializing: they can only be
/// filled in once blockIds exist, which is inside the transaction.
#[derive(Default)]
struct SideTables {
    sections: Vec<(String, String)>,
    images: Vec<QuipImageRef>,
    pending_links: Vec<QuipPendingLink>,
}

impl SideTables {
    /// Pair a Quip section anchor with the blockId minted for the block
    /// that carried it. `materialize_block` runs in document order, so
    /// `sections` is in document order by construction.
    fn record_section<T: ReadTxn>(
        &mut self,
        txn: &T,
        el: &XmlElementRef,
        section_id: Option<&String>,
    ) {
        if let Some(sid) = section_id {
            self.sections.push((sid.clone(), block_id_of(txn, el)));
        }
    }
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
/// pipeline. A span carrying a `mention` becomes a `DocMention` inline
/// leaf instead of a text run. `container` is `el`'s own `NodeType`,
/// passed through only for `insert_block`'s containment check.
fn insert_spans(
    txn: &mut yrs::TransactionMut<'_>,
    el: &XmlElementRef,
    container: NodeType,
    spans: &[Span],
    side: &mut SideTables,
) {
    let scope = XmlOpenable::Element(el.clone());
    for span in spans {
        if span.hard_break_before {
            insert_block(txn, &scope, container, NodeType::HardBreak);
        }
        match &span.mention {
            Some(mention) => insert_doc_mention(txn, &scope, container, &span.text, mention, side),
            None => {
                if let Some(text) = insert_text(txn, el, &span.text) {
                    apply_marks(txn, &text, &span.marks);
                }
            }
        }
    }
}

/// Materialize an intra-Quip link as a placeholder `DocMention` and
/// record the back-patch it needs.
///
/// `doc_id` is written empty on purpose: the OgreNotes document the
/// target thread imports into may not exist yet (or at all). Phase 2b
/// finds these by `source_block_id` and fills in `doc_id` /
/// `target_block_id`. Until then the chip still carries the original
/// Quip `url`, so an un-back-patched import degrades to a link back to
/// Quip rather than to a dead chip.
fn insert_doc_mention(
    txn: &mut yrs::TransactionMut<'_>,
    scope: &XmlOpenable<'_>,
    container: NodeType,
    label: &str,
    mention: &QuipMention,
    side: &mut SideTables,
) {
    let el = insert_block(txn, scope, container, NodeType::DocMention);
    el.insert_attribute(txn, "doc_id", "");
    el.insert_attribute(txn, "url", mention.url.clone());
    if !label.is_empty() {
        el.insert_attribute(txn, "title", label.to_string());
    }
    // The unresolved target, kept on the node as well as in the side
    // table so a document that outlives this import run is still
    // self-describing (a re-run of the back-patch needs no side car).
    el.insert_attribute(txn, "pending_quip_thread", mention.thread_id.clone());
    if let Some(section) = &mention.section_id {
        el.insert_attribute(txn, "pending_quip_section", section.clone());
    }
    side.pending_links.push(QuipPendingLink {
        source_block_id: block_id_of(&*txn, &el),
        target_quip_thread_id: mention.thread_id.clone(),
        target_quip_section_id: mention.section_id.clone(),
    });
}

/// Build the yrs `Doc` from containment-clean blocks, collecting the
/// side-tables (section map, images, pending links) as it goes.
fn materialize(blocks: &[QuipBlock]) -> QuipDocument {
    let doc = Doc::new();
    let mut side = SideTables::default();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");
        let root = XmlOpenable::Fragment(&fragment);
        for block in blocks {
            materialize_block(&mut txn, &root, NodeType::Doc, block, &mut side);
        }
    }
    QuipDocument {
        doc,
        sections: side.sections,
        images: side.images,
        pending_links: side.pending_links,
        // Set by `from_quip_html`, which is the only caller that has the
        // parse-time count; `materialize` alone cannot know.
        deep_nesting_truncated: 0,
    }
}

fn materialize_block(
    txn: &mut yrs::TransactionMut<'_>,
    parent: &XmlOpenable<'_>,
    parent_type: NodeType,
    block: &QuipBlock,
    side: &mut SideTables,
) {
    match block {
        QuipBlock::Para { spans, section_id } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Paragraph);
            side.record_section(&*txn, &el, section_id.as_ref());
            insert_spans(txn, &el, NodeType::Paragraph, spans, side);
        }
        QuipBlock::Heading { level, spans, section_id } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Heading);
            el.insert_attribute(txn, "level", (*level).clamp(1, 6).to_string());
            side.record_section(&*txn, &el, section_id.as_ref());
            insert_spans(txn, &el, NodeType::Heading, spans, side);
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
                    materialize_block(txn, &scope, item_type, child, side);
                }
            }
        }
        QuipBlock::Quote { blocks } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Blockquote);
            let scope = XmlOpenable::Element(el);
            for child in blocks {
                materialize_block(txn, &scope, NodeType::Blockquote, child, side);
            }
        }
        QuipBlock::Code { language, text } => {
            let el = insert_block(txn, parent, parent_type, NodeType::CodeBlock);
            if !language.is_empty() {
                el.insert_attribute(txn, "language", language.clone());
            }
            // A code block carries no marks (`NodeType::is_code`), so
            // the returned text handle is deliberately unused.
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
                        materialize_block(txn, &scope, cell_type, child, side);
                    }
                }
            }
        }
        QuipBlock::Image { src, alt } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Image);
            // Left as the raw Quip value on purpose — the blob
            // side-load pass rewrites it to a durable blob reference,
            // keyed on the blockId recorded alongside it here.
            el.insert_attribute(txn, "src", src.clone());
            if !alt.is_empty() {
                el.insert_attribute(txn, "alt", alt.clone());
            }
            side.images.push(QuipImageRef {
                block_id: block_id_of(&*txn, &el),
                src: src.clone(),
                alt: alt.clone(),
            });
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

    // ─── inline marks ────────────────────────────────────────

    #[test]
    fn inline_marks_survive_the_round_trip() {
        let out = from_quip_html(
            "<p><b>bold</b> <i>it</i> <code>c</code> \
             <a href=\"https://ok.example/x\">link</a></p>",
        );
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("<strong>bold</strong>") || html.contains("<b>bold</b>"), "{html}");
        assert!(html.contains("<em>it</em>") || html.contains("<i>it</i>"), "{html}");
        assert!(html.contains("<code>c</code>"), "{html}");
        assert!(html.contains("https://ok.example/x"), "link href preserved: {html}");
    }

    // ─── side tables ─────────────────────────────────────────

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
             <a href=\"https://elsewhere.example/page\">ext</a></p>",
        );
        assert_eq!(out.pending_links.len(), 1, "only the quip link is pending");
        assert_eq!(out.pending_links[0].target_quip_thread_id, "AbCd1234");
        assert!(out.pending_links[0].target_quip_section_id.is_none());
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("elsewhere.example/page"), "external link passes through: {html}");
    }

    #[test]
    fn intra_quip_link_with_fragment_records_the_section() {
        let out = from_quip_html(
            "<p><a href=\"https://example.quip.com/AbCd1234/Doc#sec-77\">x</a></p>",
        );
        assert_eq!(out.pending_links[0].target_quip_section_id.as_deref(), Some("sec-77"));
    }

    #[test]
    fn underline_and_strike_accept_their_spelling_variants() {
        for (html, tag) in [
            ("<p><u>x</u></p>", "<u>x</u>"),
            ("<p><s>x</s></p>", "<s>x</s>"),
            ("<p><del>x</del></p>", "<s>x</s>"),
            ("<p><strike>x</strike></p>", "<s>x</s>"),
            ("<p><strong>x</strong></p>", "<strong>x</strong>"),
            ("<p><em>x</em></p>", "<em>x</em>"),
        ] {
            let exported = crate::export::to_html(&from_quip_html(html).doc);
            assert!(exported.contains(tag), "{html} -> {exported}");
        }
    }

    #[test]
    fn nested_marks_combine_on_a_single_run() {
        let out = from_quip_html("<p><b><i>both</i></b></p>");
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("<strong><em>both</em></strong>"), "{html}");
    }

    #[test]
    fn a_mark_does_not_leak_onto_the_text_beside_it() {
        let out = from_quip_html("<p>plain <b>bold</b> plain</p>");
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("plain <strong>bold</strong> plain"), "{html}");
    }

    #[test]
    fn marks_apply_inside_headings_and_list_items() {
        let out = from_quip_html("<h2><b>H</b></h2><ul><li><i>L</i></li></ul>");
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("<strong>H</strong>"), "{html}");
        assert!(html.contains("<em>L</em>"), "{html}");
    }

    #[test]
    fn a_link_mark_survives_a_hard_break() {
        // Regression guard for the span-splitting interaction: the
        // break must not swallow the following run's marks.
        let out = from_quip_html("<p>a<br><a href=\"https://ok.example/y\">b</a></p>");
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("<br"), "{html}");
        assert!(html.contains("<a href=\"https://ok.example/y\">b</a>"), "{html}");
    }

    #[test]
    fn quip_placeholder_is_a_valid_inline_leaf_carrying_its_target() {
        let out = from_quip_html("<p>see <a href=\"https://x.quip.com/T1/Doc#s9\">Doc</a></p>");
        // Exercises the `is_inline()` exemption in `check_subtree`:
        // `DocMention` is an inline leaf, a category
        // `valid_children()` never enumerates.
        assert_valid_tree(&out.doc);
        let txn = out.doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        let Some(XmlOut::Element(para)) = frag.get(&txn, 0) else { panic!("expected an element") };
        let mut found = false;
        for i in 0..para.len(&txn) {
            let Some(XmlOut::Element(el)) = para.get(&txn, i) else { continue };
            assert_eq!(NodeType::from_tag(el.tag().as_ref()), Some(NodeType::DocMention));
            found = true;
            assert_eq!(el.get_attribute(&txn, "doc_id").as_deref(), Some(""), "unresolved");
            assert_eq!(el.get_attribute(&txn, "title").as_deref(), Some("Doc"));
            assert_eq!(el.get_attribute(&txn, "pending_quip_thread").as_deref(), Some("T1"));
            assert_eq!(el.get_attribute(&txn, "pending_quip_section").as_deref(), Some("s9"));
            assert_eq!(
                el.get_attribute(&txn, "blockId").as_deref(),
                Some(out.pending_links[0].source_block_id.as_str()),
                "the placeholder's own blockId is the pending link's source"
            );
        }
        assert!(found, "a DocMention placeholder was emitted");
    }

    #[test]
    fn quip_thread_url_shapes() {
        // (href, expected thread id — None means "not intra-Quip",
        //  expected section anchor)
        let cases = [
            ("https://example.quip.com/AbCd1234/Some-Doc", Some("AbCd1234"), None),
            ("https://quip.com/AbCd1234", Some("AbCd1234"), None),
            ("https://example.quip.com/AbCd1234/Doc#sec-77", Some("AbCd1234"), Some("sec-77")),
            ("https://EXAMPLE.QUIP.COM./AbCd1234/Doc", Some("AbCd1234"), None),
            // Relative hrefs resolve against the Quip base: the input is
            // a thread body fetched from Quip, so it can't be relative
            // to anything else.
            ("/AbCd1234/Some-Doc", Some("AbCd1234"), None),
            ("/AbCd1234/Some-Doc#sec-9", Some("AbCd1234"), Some("sec-9")),
            // No path segment at all — nothing to key a back-patch on.
            ("https://example.quip.com/", None, None),
            ("#sec-3", None, None),
            ("", None, None),
            // Lookalike hosts a naive `ends_with` would have accepted.
            ("https://notquip.com/AbCd1234/Doc", None, None),
            ("https://evil.example/?u=quip.com", None, None),
            // Userinfo spoofing: the *host* here is evil.example, which
            // only parsing (not string-matching) reveals.
            ("https://quip.com@evil.example/AbCd1234", None, None),
            ("https://elsewhere.example/page", None, None),
            ("mailto:someone@example.org", None, None),
        ];
        for (href, thread, section) in cases {
            let got = quip_thread_from_url(href);
            assert_eq!(got.as_ref().map(|m| m.thread_id.as_str()), thread, "thread: {href}");
            assert_eq!(got.as_ref().and_then(|m| m.section_id.as_deref()), section, "sec: {href}");
        }
    }

    #[test]
    fn an_external_link_never_becomes_a_pending_link() {
        let out = from_quip_html("<p><a href=\"https://notquip.com/AbCd1234/x\">n</a></p>");
        assert!(out.pending_links.is_empty(), "lookalike host must stay an ordinary link");
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("https://notquip.com/AbCd1234/x"), "{html}");
    }

    #[test]
    fn a_relative_quip_href_is_stored_resolved_not_same_origin() {
        let out = from_quip_html("<p><a href=\"/AbCd1234/Some-Doc\">rel</a></p>");
        assert_eq!(out.pending_links.len(), 1, "a relative Quip href is still intra-Quip");
        assert_eq!(out.pending_links[0].target_quip_thread_id, "AbCd1234");
        let html = crate::export::to_html(&out.doc);
        // `export::is_safe_url` accepts a leading `/`, so storing the
        // href unresolved would export a *live same-origin link that
        // 404s* — not an inert one. The stored url must be absolute.
        assert!(html.contains("href=\"https://quip.com/AbCd1234/Some-Doc\""), "{html}");
        assert!(!html.contains("href=\"/AbCd1234/Some-Doc\""), "same-origin href leaked: {html}");
    }

    #[test]
    fn hoisted_blocks_from_a_quip_link_keep_document_order() {
        // The link's image must land *between* the text that preceded
        // it and the text that follows, exactly as a bare `<img>` does.
        // Wrapping in `<p>` hides this (`walk_text_container`
        // re-partitions), so the repro is a transparent `<div>`.
        let b = blocks(
            "<div>a <a href=\"https://x.quip.com/T/D\">lbl<img src=\"i.png\"></a> b</div>",
        );
        let kinds: Vec<_> = b.iter().map(|x| x.node_type()).collect();
        assert_eq!(
            kinds,
            vec![NodeType::Paragraph, NodeType::Image, NodeType::Paragraph],
            "{b:?}"
        );
        let QuipBlock::Para { spans, .. } = &b[0] else { panic!("expected para: {b:?}") };
        assert_eq!(spans_text(spans), "a lbl", "the chip stays with the text before it");
        let QuipBlock::Para { spans, .. } = &b[2] else { panic!("expected para: {b:?}") };
        assert_eq!(spans_text(spans), "b");
    }

    // ─── documented losses, pinned ───────────────────────────

    #[test]
    fn sub_sup_and_mark_keep_their_text_but_lose_their_formatting() {
        // Assumption #21: `MarkType` has Subscript/Superscript, but no
        // Quip spelling for them has been observed, so these three stay
        // transparent passthrough. Pinned so the loss stays deliberate.
        let out = from_quip_html("<p>H<sub>2</sub>O and x<sup>2</sup> and <mark>hi</mark></p>");
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("H2O and x2 and hi"), "text survives: {html}");
        assert!(!html.contains("<sub"), "{html}");
        assert!(!html.contains("<sup"), "{html}");
        assert!(!html.contains("<mark"), "{html}");
    }

    #[test]
    fn a_mark_wrapping_an_intra_quip_link_is_lost() {
        // Inherent, not a bug: marks are yrs *text* attributes and the
        // placeholder is an element leaf, so nothing can carry the bold
        // across. Pinned so a later change can't alter it silently.
        let out = from_quip_html("<p><b><a href=\"https://x.quip.com/T3/D\">lbl</a></b></p>");
        assert_eq!(out.pending_links.len(), 1);
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("class=\"doc-mention\""), "{html}");
        assert!(!html.contains("<strong>"), "the bold is dropped: {html}");
    }

    #[test]
    fn an_image_inside_a_quip_link_is_hoisted_not_dropped() {
        let out = from_quip_html(
            "<p><a href=\"https://x.quip.com/T2/D\">lbl<img src=\"i.png\"></a></p>",
        );
        assert_valid_tree(&out.doc);
        assert_eq!(out.images.len(), 1, "the image survives the mention rewrite");
        assert_eq!(out.pending_links.len(), 1);
    }

    #[test]
    fn every_section_id_maps_to_an_element_that_really_exists() {
        let out = from_quip_html(
            "<h1 id=\"a\">T</h1><p id=\"b\">x</p><p data-section-id=\"c\">y</p>",
        );
        assert_eq!(
            out.sections.iter().map(|(q, _)| q.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "document order"
        );
        let txn = out.doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        let mut ids = std::collections::HashSet::new();
        for_each_element(&txn, &frag, &mut |txn, el| {
            ids.insert(el.get_attribute(txn, "blockId").unwrap_or_default());
        });
        for (section, block_id) in &out.sections {
            assert!(ids.contains(block_id), "section {section} points at a live blockId");
        }
    }

    // ─── converter fixture matrix ────────────────────────────

    /// Every fixture must convert without panicking into a non-empty
    /// document. The per-feature assertions live in the unit tests
    /// above; this is the breadth net that catches a walker change
    /// blowing up on a shape no single test covers.
    #[test]
    fn every_fixture_converts_to_a_non_empty_valid_document() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quip");
        let mut seen = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("fixture dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("html") {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            let html = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            let out = from_quip_html(&html);
            assert_valid_tree(&out.doc);
            let txn = out.doc.transact();
            let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
            assert!(frag.len(&txn) > 0, "{name} produced an empty document");
            seen.push(name);
        }
        seen.sort();
        assert_eq!(
            seen,
            vec![
                "checklists.html",
                "code.html",
                "headings.html",
                "images.html",
                "kitchen_sink.html",
                "links.html",
                "lists.html",
                "marks.html",
                "sections.html",
                "tables.html",
            ],
            "the fixture matrix must stay complete"
        );
    }

    #[test]
    fn the_kitchen_sink_fixture_populates_every_side_table() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/quip/kitchen_sink.html");
        let out = from_quip_html(&std::fs::read_to_string(path).expect("kitchen sink"));
        assert!(!out.sections.is_empty(), "sections");
        assert!(!out.images.is_empty(), "images");
        assert!(!out.pending_links.is_empty(), "pending links");
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
        // Same exemption `insert_block` makes, and for the same reason:
        // `valid_children()` is a *block*-containment predicate and
        // never enumerates inline leaves (`HardBreak`, `Mention`,
        // `DocMention` — `schema.rs:184-186`). Without this an
        // otherwise-valid document containing a `DocMention` would fail
        // here spuriously.
        assert!(
            nt.is_inline() || parent.valid_children().contains(&nt),
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
