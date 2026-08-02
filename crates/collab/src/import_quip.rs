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
//! ## The markup shape started as an educated guess
//!
//! No sample of a real `/2/threads/{id}/html` response was available
//! when this was written. Every assumption about how Quip spells a
//! feature is therefore isolated behind a small named helper carrying
//! an `UNVERIFIED MARKUP` note — those helpers are the reconciliation
//! checklist as real markup arrives:
//!
//!   - [`parse_list`] / [`list_is_task`] — a list being numbered or a
//!     checklist at all. **RECONCILED**: real Quip emits a bare `<ul>`
//!     for bullet, numbered *and* checklist alike, and puts the only
//!     discriminator — `data-section-style` — on the wrapping `<div>`.
//!     See the vocabulary table above [`SECTION_STYLE_ORDERED`].
//!   - [`checked_state`]  — checklist item checked/unchecked.
//!     **STILL UNVERIFIED**: the staged corpus contains no checked
//!     item, so no marker for one has ever been seen.
//!   - [`code_language`]  — code-block language tag
//!   - [`section_id`]     — Quip section anchor id
//!   - [`quip_thread_from_url`] — an `<a href>` being an intra-Quip
//!     document link, and where the thread id / section anchor sit
//!     inside it
//!
//! [`walk_control`] is the exception: `<control>` was reconciled against
//! a real staged thread body and is **verified**, not guessed.
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
    types::xml::{XmlElementPrelim, XmlFragment, XmlOut, XmlTextPrelim, XmlTextRef},
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
    /// Quip person mentions found in the source, each materialized as a
    /// `Mention` leaf that is **not yet finished**: it carries the Quip
    /// person's id under [`PENDING_QUIP_USER_ATTR`] and an empty
    /// `user_id`. The caller must run [`resolve_person_mentions`] before
    /// persisting the document — that call is total, so every leaf either
    /// gains a real OgreNotes `user_id` or degrades to plain text.
    pub person_mentions: Vec<QuipPersonMention>,
    /// How many subtrees were flattened for exceeding
    /// [`MAX_NESTING_DEPTH`](crate::import_quip::MAX_NESTING_DEPTH).
    ///
    /// Non-zero means the document nested deeper than the walker will
    /// descend: the text survived, the structure below that point did not.
    /// A named loss, so the caller reports it the way it reports a dropped
    /// image rather than letting it pass silently.
    pub deep_nesting_truncated: usize,
    /// Spreadsheet cells whose [`FORMULA_ATTR`] this import did not carry
    /// into the document (#192).
    ///
    /// Quip keeps the formula on the cell's `<span>` and the last value it
    /// computed as that span's text; only the text survives, so the imported
    /// table shows the numbers Quip last calculated and nothing recomputes.
    /// A named loss for the same reason `deep_nesting_truncated` is one: the
    /// data was in the export and the import did not keep it.
    pub formulas_dropped: usize,
    /// Embedded Quip live-app blocks — Kanban boards and whatever else Quip
    /// spells with [`LIVE_APP_ATTR_PREFIX`] — whose payload this import did
    /// not convert (#191).
    ///
    /// The walker imports whatever ordinary HTML the block rendered into
    /// (for a Kanban board, its column-heading row) and nothing of the
    /// payload that holds the board's actual cards.
    pub live_apps_dropped: usize,
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

/// A Quip **person** mention the source carried, keyed by the blockId of
/// the `Mention` leaf that stands in for it.
///
/// A person is not a document: this is deliberately separate from
/// [`QuipPendingLink`], whose targets are threads and whose back-patch is
/// Phase 2b's. The importer resolves `quip_user_id` to an OgreNotes user id
/// (exact email match, nothing fuzzy) and hands the answer to
/// [`resolve_person_mentions`].
pub struct QuipPersonMention {
    pub block_id: String,
    /// The Quip person's id, taken from the mention anchor's href.
    pub quip_user_id: String,
    /// The anchor's text — the name the document's author saw, e.g. `Joel`.
    pub label: String,
}

/// Attribute holding the still-unresolved Quip person id on a `Mention`
/// leaf the walker emitted. Removed by [`resolve_person_mentions`] once the
/// leaf carries a real OgreNotes `user_id`; its presence is exactly the
/// predicate "this mention has not been resolved yet".
pub const PENDING_QUIP_USER_ATTR: &str = "pending_quip_user";

/// Attribute holding the anchor's resolved href on an unresolved `Mention`
/// leaf, so a chip that turns out not to be a person can become a
/// `DocMention` with the same `url` [`walk_anchor`] would have given it.
///
/// Transient exactly like [`PENDING_QUIP_USER_ATTR`], and removed on all
/// three of [`resolve_person_mentions`]' branches — the matched leaf drops
/// it, and the other two replace the node outright.
pub const PENDING_QUIP_URL_ATTR: &str = "pending_quip_url";

/// Which kind of Quip thread this HTML came from.
///
/// Quip spells an ordinary document table and a whole spreadsheet with the
/// same `<table>` element, and — this is the part that forces the caller to
/// answer — with the same *grid chrome* around it. Across the 56-document
/// staged corpus, 17 of the 47 tables carry the column-header `<thead>` and
/// the row-number gutter column described on [`has_grid_chrome`]; **16 of
/// those 17 are prose tables** whose `<th>` cells hold real headings ("Access
/// Level", "What it means"). Only the one spreadsheet's `<th>` cells hold
/// `A B C D …`.
///
/// So no structural marker separates the two **header rows** — not
/// `class='empty'`, not the 2em corner cell, all of which appear on prose
/// tables that must keep their headers. The thread's own type is the only
/// signal that does, and it lives in Quip's thread metadata rather than in
/// the HTML body, so it has to be carried in from the caller (#230).
///
/// The `#f0f0f0` **gutter column** is not in that predicament and does not
/// consult this type: nothing an author can type produces an id-less cell,
/// so it is stripped on either path (#232 — see [`strip_grid_chrome`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuipThreadKind {
    /// Anything Quip does not call a spreadsheet. A ruled table keeps its
    /// `<th>` header row; every other table is imported exactly as the
    /// markup spells it.
    #[default]
    Document,
    /// Quip `thread_type == "spreadsheet"`. The column-letter header row is
    /// stripped along with the gutter.
    Spreadsheet,
}

/// Import a Quip HTML body into a fresh `Doc`, together with the
/// side-tables the caller needs to finish the job.
///
/// The returned document's person mentions are **unfinished** — see
/// [`QuipDocument::person_mentions`] and [`resolve_person_mentions`].
///
/// Treats the body as an ordinary document; a caller that knows Quip called
/// this thread a spreadsheet must say so via [`from_quip_html_as`], because
/// nothing in the markup reveals it.
pub fn from_quip_html(html: &str) -> QuipDocument {
    from_quip_html_as(html, QuipThreadKind::Document)
}

/// [`from_quip_html`], told what kind of thread the body came from.
///
/// The only thing `kind` changes is whether a ruled table's **header row** is
/// treated as presentation; its row-number gutter is stripped either way —
/// see [`QuipThreadKind`] and [`strip_grid_chrome`].
pub fn from_quip_html_as(html: &str, kind: QuipThreadKind) -> QuipDocument {
    let (mut blocks, losses, anchors) = parse_quip_counting_losses(html);
    strip_grid_chrome(&mut blocks, kind);
    let mut out = QuipDocument {
        deep_nesting_truncated: losses.deep_nesting_truncated,
        formulas_dropped: losses.formulas_dropped,
        live_apps_dropped: losses.live_apps_dropped,
        ..materialize(&blocks)
    };
    // After `materialize`, because an anchor can only be resolved once the
    // blockIds exist; after `strip_grid_chrome`, so an annotation inside a
    // stripped gutter cell resolves to nothing rather than to a block that
    // is no longer in the document.
    resolve_comment_anchors(&mut out.sections, &anchors);
    out
}

// ─── intermediate block model ────────────────────────────────────

/// The inline formatting active over a run of text.
///
/// Deliberately a subset of `MarkType` (`schema.rs:311`): `subscript`
/// and `superscript` are absent because no Quip spelling for them has
/// been observed. `<sub>` / `<sup>` / `<mark>` therefore stay
/// transparent passthrough — their *text* survives, their formatting
/// does not. That is a known, recorded loss, not an oversight.
///
/// `MarkType::Mention` is absent for a different reason: Quip *does*
/// spell person mentions (see [`walk_control`]), but they materialize as
/// `NodeType::Mention` **leaves**, which is the shape this schema's own
/// editor writes — not as a mark over text.
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

/// A Quip **person** mention: `<control><a href="…/USERID">Joel</a></control>`.
///
/// Deliberately its own type rather than a [`QuipMention`] with a flag: the
/// two look identical in the markup (both are an `<a>` at a Quip URL) but
/// mean completely different things, and conflating them is precisely the
/// bug this exists to fix — a person imported as a document link renders as
/// a "Missing document" chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuipPerson {
    pub quip_user_id: String,
    /// The **resolved absolute** anchor href. Carried because a chip that
    /// turns out not to be a person becomes a `DocMention`, and that node
    /// needs the same `url` [`walk_anchor`] would have given it — see
    /// [`PersonOutcome::NotAPerson`]. Reconstructing `https://quip.com/<id>`
    /// instead would silently rewrite a corporate `*.quip.com` host.
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
    /// Set when this run *is* a Quip person mention: it materializes as an
    /// unresolved `Mention` element, not as a text run. Mutually exclusive
    /// with `mention` by construction — `push_mention` and `push_person`
    /// each push their own fresh span.
    pub person: Option<QuipPerson>,
}

impl Span {
    fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Self::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuipCell {
    pub header: bool,
    /// Anchor on the `<td>` / `<th>` itself — see [`section_id`].
    pub section_id: Option<String>,
    pub blocks: Vec<QuipBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuipRow {
    /// Anchor on the `<tr>` itself.
    pub section_id: Option<String>,
    pub cells: Vec<QuipCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuipItem {
    pub checked: Option<bool>,
    /// Anchor on the `<li>` itself — the densest anchor site in the
    /// corpus after `<td>`.
    pub section_id: Option<String>,
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
        section_id: Option<String>,
        items: Vec<QuipItem>,
    },
    Quote {
        section_id: Option<String>,
        blocks: Vec<QuipBlock>,
    },
    Code {
        language: String,
        section_id: Option<String>,
        text: String,
    },
    Rule {
        section_id: Option<String>,
    },
    Table {
        section_id: Option<String>,
        rows: Vec<QuipRow>,
    },
    Image {
        src: String,
        alt: String,
        section_id: Option<String>,
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
            QuipBlock::Rule { .. } => NodeType::HorizontalRule,
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
        // Quip's wrapper around a client-rendered entity. It MUST survive
        // the sanitizer: ammonia strips an unlisted tag but keeps its
        // children, so a stripped `<control><a href="quip.com/…">Joel</a>`
        // reaches the walker byte-identical to a bare document link — and
        // becomes a "Missing document" chip. The wrapper is the only signal
        // that separates a person from a document. See `walk_control`.
        "control", //
        "table", "thead", "tbody", "tfoot", "tr", "th", "td", //
        "strong", "em", "b", "i", "u", "s", "del", "strike", "sub", "sup", "mark",
    ]
    .into_iter()
    .collect()
}

/// Attributes the walker reads. Anything not listed here (and not
/// `data-*`) is stripped before the DOM is built, so the walker never
/// sees an event handler or a style payload.
///
/// `formula` is here for one reason and it is worth stating, because
/// widening this set is the one edit in this file that moves the XSS
/// boundary: without it the attribute is gone before the DOM exists, and
/// [`count_unconverted_content`] cannot tell a spreadsheet that lost 30
/// formulas from a table that never had one (#192). Its **value is never
/// read** — nothing in this module calls `attr(_, "formula")`, so the string
/// reaches no yrs node, no `ReportNote::detail`, and no log. What it admits
/// is therefore an attribute that is (a) not an event handler, (b) not a URL
/// carrier, so no scheme filter applies to it, (c) not `style`, and (d)
/// re-emitted by ammonia with its value HTML-escaped in any case. A
/// `data-`-prefixed spelling of the same attribute would already be admitted
/// by `generic_attribute_prefixes` below, so this is the existing policy
/// applied to the name Quip actually uses, not a new kind of allowance.
///
/// [`ANNOTATION_ATTR`] is here for the same reason and clears the same bar
/// (#194 F-10): not an event handler, not a URL carrier, not `style`, and
/// re-emitted HTML-escaped. It differs from `formula` in one respect — its
/// value *is* read, by [`collect_comment_anchors`], and lands in
/// [`QuipDocument::sections`] and from there in a `SECMAP#` row. It reaches
/// no yrs node and no rendered HTML, so the string is stored and compared,
/// never interpreted; the same is already true of every `id` this allowlist
/// admits, which is the identical trust question.
fn allowed_attributes() -> HashSet<&'static str> {
    [
        "id", "href", "src", "alt", "title", "type", "checked", "value",
        "colspan", "rowspan", "class", "start", "align", "formula",
        ANNOTATION_ATTR,
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

/// The attribute Quip puts on a spreadsheet cell's `<span>` to carry that
/// cell's formula — `formula='=SUM(D8:D10)'` (#192). The span's *text* is
/// the value Quip last computed from it.
pub const FORMULA_ATTR: &str = "formula";

/// Attribute-name prefix Quip uses for an embedded live app's own state
/// (#191): `data-live-app-payload` holds a Kanban board's cards, and the
/// prefix rather than the exact name is matched so a sibling spelling
/// (`data-live-app-id`, `data-live-app-type`) counts the same block once.
///
/// **This prefix is the one thing here not pinned by a fixture.** No corpus
/// document carries a live app — `tests/quip_corpus.rs` names that as a
/// deliberate coverage gap, and the audit's Kanban thread (`dAcAAAm68OG`) was
/// never checked in and its staging prefix has since been purged. The name
/// comes from the audit finding recorded on issue #191. If it turns out to be
/// wrong, the failure mode is the one we already have — a board imports with
/// no note — never a false report, since nothing else in 5 real documents and
/// 56 audited ones spells an attribute this way.
pub const LIVE_APP_ATTR_PREFIX: &str = "data-live-app";

/// The attribute Quip puts on the `<span>` wrapping a **commented** range —
/// `annotationid="temp:C:ffbc9747…"` (#194 F-10). Its value is the id Quip's
/// comment API keys a comment thread by, so it is the join a future Phase 4
/// needs between a fetched comment and the place in the document it belongs.
///
/// **Measured, in all four occurrences across the 56-document staged
/// corpus** (only two documents carry any): the shape is invariant —
/// `<span annotationid="X" class="c9 h2" id="X">`, double-quoted where
/// almost every other Quip attribute in the corpus is single-quoted, and the
/// value is repeated verbatim in the span's own `id`. That repetition is why
/// this belongs in the section map rather than in a table of its own: the
/// annotation id *is* an anchor id in Quip's own namespace, so a
/// `#temp:C:…` deep link at the commented text and a Phase-4 comment lookup
/// are the same question with the same answer.
///
/// Four occurrences is thin evidence and is stated as such. What rests on
/// the shape being right is only whether an anchor is captured; nothing
/// about the imported *content* changes, and an attribute that never appears
/// yields an empty pass.
pub const ANNOTATION_ATTR: &str = "annotationid";

/// Parse Quip HTML into the intermediate block model. Pure: no yrs
/// transaction is live while the DOM is walked.
pub(crate) fn parse_quip(html: &str) -> Vec<QuipBlock> {
    parse_quip_counting_losses(html).0
}

/// One commented range found in the source: the [`ANNOTATION_ATTR`] value,
/// paired with the id of the **nearest enclosing element that carries an
/// anchor** — which is the id [`QuipDocument::sections`] already knows how to
/// turn into a blockId. Resolved to that blockId by
/// [`resolve_comment_anchors`] once materialization has minted one.
type CommentAnchor = (String, String);

/// Everything the parse knows it did not carry across. Each field is a
/// *count of things the source had*, so a caller can name the loss to the
/// user; none of them is an error, and none of them stops an import.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParseLosses {
    /// Subtrees flattened for exceeding [`MAX_NESTING_DEPTH`].
    pub deep_nesting_truncated: usize,
    /// Cells carrying [`FORMULA_ATTR`] (#192).
    pub formulas_dropped: usize,
    /// Elements carrying a [`LIVE_APP_ATTR_PREFIX`] attribute (#191).
    pub live_apps_dropped: usize,
}

/// [`parse_quip`] plus what the parse lost. Split out so `parse_quip` keeps
/// the signature its unit tests use while [`from_quip_html`] can surface the
/// losses on [`QuipDocument`] and the importer can report them.
pub(crate) fn parse_quip_counting_losses(
    html: &str,
) -> (Vec<QuipBlock>, ParseLosses, Vec<CommentAnchor>) {
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::RcDom;

    let safe = sanitize(html);
    let dom: RcDom = html5ever::parse_document(RcDom::default(), html5ever::driver::ParseOpts::default())
        .from_utf8()
        .read_from(&mut safe.as_bytes())
        .expect("html5ever parse is infallible on bytes");

    // Count what this walker is about to not carry, on the tree as Quip
    // wrote it. Before `flatten_below_depth` on purpose: that pass replaces
    // an over-deep subtree with its flattened text, which would take a
    // formula or a live-app payload with it and make the loss report the one
    // thing it must never be — quieter than the loss.
    let mut losses = count_unconverted_content(&dom.document);

    // Drop Quip's per-cell line terminators BEFORE anything reshapes the
    // tree. Order is load-bearing: `normalize_quip_lists` re-parents a
    // sibling `<ul>` onto the end of the `<li>` that owns it, which lands
    // *after* the terminator and would stop it being the item's last child.
    // Deciding the question on the markup as Quip wrote it keeps the rule
    // independent of every later rewrite.
    strip_cell_terminators(&dom.document);

    // Rewrite Quip's two list-structure spellings into ordinary HTML first —
    // nesting a list inside the item that owns it deepens the tree, so this
    // has to happen on the *un*bounded tree for the bound below to still hold
    // when the walker runs. `normalize_quip_lists` is itself iterative, so it
    // survives the same pathological input `flatten_below_depth` exists for.
    normalize_quip_lists(&dom.document);

    // Bound the tree BEFORE the recursive walk rather than threading a depth
    // counter through all twelve `walk_children` call sites. The guarantee is
    // then structural: whatever the walker receives is already shallower than
    // MAX_NESTING_DEPTH, so every recursive pass over it — `walk_element`,
    // `enforce_containment`, `materialize_block` — is bounded by the same
    // constant for free, and no future recursive pass can forget to check.
    losses.deep_nesting_truncated = flatten_below_depth(&dom.document);

    // Deliberately AFTER every reshaping pass above, unlike
    // `count_unconverted_content`. That one is a census of the source and
    // must see the tree as Quip wrote it; this one records an ancestor
    // relationship the walker is about to act on, so it has to read the same
    // tree the walker reads or the ancestor it names may no longer be there.
    let anchors = collect_comment_anchors(&dom.document);

    let mut out = Vec::new();
    let mut pending = InlineBuf::default();
    walk_children(&dom.document, &mut out, &Marks::default(), &mut pending);
    pending.flush(&mut out, None);

    (enforce_containment(out, NodeType::Doc), losses, anchors)
}

/// Pair every [`ANNOTATION_ATTR`] in the tree with the anchor id of the
/// nearest ancestor that has one (#194 F-10), in document order.
///
/// **The span itself is not a candidate.** It carries an `id` equal to its
/// own annotation id, so counting it would resolve the anchor to itself and
/// answer nothing; the question is which *containing* anchor the highlight
/// sits inside, and the answer starts at the parent.
///
/// **Iterative**, like [`count_unconverted_content`] and
/// [`flatten_below_depth`], and for the same reason — though this one runs
/// after the depth bound is imposed, a recursive pass over third-party HTML
/// is a habit worth not acquiring here. Children are pushed in reverse so
/// the stack pops them in document order.
///
/// An annotation whose nearest anchored ancestor did not survive as a block
/// resolves to nothing and is dropped by [`resolve_comment_anchors`]; that is
/// zero occurrences in the corpus, where all four sit directly inside a
/// `<p id='…' class='line'>`.
fn collect_comment_anchors(document: &markup5ever_rcdom::Handle) -> Vec<CommentAnchor> {
    use markup5ever_rcdom::NodeData;

    let mut found = Vec::new();
    // (node, the nearest anchored ancestor's id) still to inspect.
    let mut stack: Vec<(markup5ever_rcdom::Handle, Option<String>)> =
        vec![(document.clone(), None)];
    while let Some((node, enclosing)) = stack.pop() {
        let mut inner = enclosing.clone();
        if let NodeData::Element { attrs, .. } = &node.data {
            let annotation = attrs
                .borrow()
                .iter()
                .find(|a| a.name.local.as_ref().eq_ignore_ascii_case(ANNOTATION_ATTR))
                .map(|a| a.value.trim().to_string())
                .filter(|v| !v.is_empty());
            if let (Some(annotation), Some(enclosing)) = (annotation, enclosing) {
                found.push((annotation, enclosing));
            }
            // Only now does this element become the enclosing anchor for its
            // descendants — see the doc above on why not for itself.
            if let Some(own) = section_id(&node) {
                inner = Some(own);
            }
        }
        stack.extend(
            node.children.borrow().iter().rev().map(|c| (c.clone(), inner.clone())),
        );
    }
    found
}

/// Fold the comment anchors into `sections`, replacing each one's enclosing
/// *anchor id* with the blockId that anchor was minted onto.
///
/// Each anchor is spliced in **immediately after** the entry it resolved
/// through, so `sections` stays in document order — the property
/// `SecMapRow::entries` documents and the order the chunker slices on.
///
/// Two entries are dropped rather than written: an annotation whose
/// enclosing anchor never became a block (nothing to point at), and one
/// whose id already keys the map (a duplicate would make the lookup
/// order-dependent, and a section map with an ambiguous key is worse than
/// one missing a row).
fn resolve_comment_anchors(sections: &mut Vec<(String, String)>, anchors: &[CommentAnchor]) {
    if anchors.is_empty() {
        return;
    }
    let mut known: HashSet<&str> = sections.iter().map(|(s, _)| s.as_str()).collect();
    let mut by_section: HashMap<&str, Vec<&str>> = HashMap::new();
    for (annotation, enclosing) in anchors {
        if known.insert(annotation.as_str()) {
            by_section.entry(enclosing.as_str()).or_default().push(annotation.as_str());
        }
    }
    let mut out = Vec::with_capacity(sections.len() + anchors.len());
    for (section, block) in sections.iter() {
        out.push((section.clone(), block.clone()));
        for annotation in by_section.remove(section.as_str()).unwrap_or_default() {
            out.push((annotation.to_string(), block.clone()));
        }
    }
    *sections = out;
}

/// Count the two kinds of content this walker knowingly leaves behind:
/// spreadsheet formulas (#192) and embedded live-app payloads (#191).
///
/// **A census, not a conversion.** Neither attribute's *value* is read — the
/// job here is to let the importer say "this document had 30 formulas and
/// none of them came over", which is the difference between a lossy import
/// and a silent one. Nothing about a count can fail, so this returns no
/// error and the caller has no branch to get wrong.
///
/// Both counts are per **element**: one element carrying any number of
/// `data-live-app-*` attributes is one live app, and one element carrying
/// `formula` is one formula. In Quip's markup the second of those is a cell
/// — the attribute rides on the `<span>` inside a `<td>`, one per cell — but
/// that is a fact about the input, not a rule this function enforces, and
/// the corpus net states it as such by asserting `formulas_dropped` equals
/// the source's `formula='` count.
///
/// Counted on the sanitized DOM, which is the only DOM there is — `formula`
/// survives `sanitize` by being in `allowed_attributes`, and
/// `data-live-app-*` by the `data-` prefix allowance.
///
/// **Iterative**, like `flatten_below_depth` and for the same reason: this
/// runs *before* the depth bound is imposed, so it sees third-party HTML of
/// unbounded depth, and a recursive census of it would abort the worker.
fn count_unconverted_content(document: &markup5ever_rcdom::Handle) -> ParseLosses {
    use markup5ever_rcdom::NodeData;

    let mut losses = ParseLosses::default();
    let mut stack: Vec<markup5ever_rcdom::Handle> = vec![document.clone()];
    while let Some(node) = stack.pop() {
        if let NodeData::Element { attrs, .. } = &node.data {
            let attrs = attrs.borrow();
            let mut names = attrs.iter().map(|a| a.name.local.as_ref());
            if names.clone().any(|n| n.eq_ignore_ascii_case(FORMULA_ATTR)) {
                losses.formulas_dropped += 1;
            }
            // `get`, never `n[..len]`. An attribute name is arbitrary
            // third-party text: non-ASCII names are legal HTML, html5ever
            // keeps them, and the whole `data-` namespace reaches here. A
            // byte slice panics on any name whose 13th byte lands inside a
            // multi-byte character — `data-emoji-🙂` is enough — and the
            // worker's `catch_unwind` would turn that into a thread that
            // burns its attempts and lands `Failed`. `get` returns `None`
            // both out of bounds and off a char boundary, and `None` is the
            // right answer either way: a name whose 13th byte is mid-character
            // cannot start with an ASCII prefix.
            if names.any(|n| {
                n.get(..LIVE_APP_ATTR_PREFIX.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(LIVE_APP_ATTR_PREFIX))
            }) {
                losses.live_apps_dropped += 1;
            }
        }
        stack.extend(node.children.borrow().iter().cloned());
    }
    losses
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

// ─── stage 2b: Quip list-structure normalization ─────────────────
//
// Quip spells list **structure** — nesting, and continuation of a
// numbered sequence — in two shapes that are not the HTML those things
// normally have. Both are rewritten here, on the sanitized DOM, into the
// ordinary markup the walker already understands. Nothing downstream
// (`walk_element`, `parse_list`, `enclosing_section_style`,
// `enforce_containment`) changes, which is what keeps the corpus's 565
// bullet lists, 46 tables and 1 checklist exactly as they were.

/// Rewrite Quip's two list-structure spellings into ordinary HTML.
///
/// Runs before [`flatten_below_depth`] rather than after: nesting a list
/// inside the item that owns it *deepens* the tree, and the walker's
/// contract is that whatever reaches it is already shallower than
/// [`MAX_NESTING_DEPTH`]. Doing the rewrite first and bounding after
/// keeps that guarantee exact instead of approximately.
///
/// Iterative with an explicit stack, for the reason
/// [`flatten_below_depth`] is: this runs on unbounded third-party markup,
/// so it must not recurse. Both rewrites only ever move a node into one
/// of its own siblings, so the moved node is still reached by the
/// traversal that follows — no node is visited twice and none is missed.
fn normalize_quip_lists(document: &markup5ever_rcdom::Handle) {
    let mut stack = vec![document.clone()];
    while let Some(node) = stack.pop() {
        merge_numbered_sections(&node);
        nest_sibling_lists(&node);
        for child in node.children.borrow().iter() {
            stack.push(child.clone());
        }
    }
}

/// **#187.** Move a list that is a *sibling* of the `<li>` it belongs to
/// inside that `<li>`.
///
/// Quip has exactly one spelling for a nested list, and it is not the
/// standard one:
///
/// ```html
/// <ul><li class='parent'>Queue</li>
///     <ul><li>nested</li></ul>          <!-- sibling of the <li> -->
/// </ul>
/// ```
///
/// **Measured across the 56-document staged corpus.** The sibling shape
/// occurs **470 times in 25 documents**; the standard `<ul>`-inside-`<li>`
/// shape occurs **zero** times. Of the 470, **418** are preceded by an
/// `<li class='parent'>` and the remaining **52** have no preceding `<li>`
/// at all. `class='parent'` is exact where it applies — all 418 `<li>`s
/// carrying it are followed by a nested list, and no `<li>` *without* it
/// is (0 counter-examples in either direction).
///
/// The rule below keys on **position** rather than on that class, because
/// the two agree on all 418 sites and position also answers the other 52:
/// those are `<ul><ul>…</ul></ul>` — an indent wrapper with no owning
/// bullet, which has nothing to nest *into*. Leaving them alone preserves
/// today's behaviour, where [`collect_items`] descends the wrapper and
/// keeps its items at one level. Inventing an empty parent item for them
/// would add 52 blank bullets to the corpus.
fn nest_sibling_lists(list: &markup5ever_rcdom::Handle) {
    if !matches!(tag_of(list).as_deref(), Some("ul" | "ol")) {
        return;
    }
    let children = std::mem::take(&mut *list.children.borrow_mut());
    let mut kept = Vec::new();
    let mut owner: Option<markup5ever_rcdom::Handle> = None;
    for child in children {
        match tag_of(&child).as_deref() {
            Some("li") => {
                owner = Some(child.clone());
                kept.push(child);
            }
            Some("ul" | "ol") => match &owner {
                Some(item) => append_child(item, child),
                // An indent wrapper with no bullet above it: nothing to
                // nest into, so leave it where it is.
                None => kept.push(child),
            },
            _ => kept.push(child),
        }
    }
    *list.children.borrow_mut() = kept;
}

/// **#188.** Merge a run of sibling `data-section-style='6'` sections
/// into a single ordered list, so a numbered sequence numbers 1..n
/// instead of restarting at 1 on every item.
///
/// Quip emits each numbered item as its *own* section, and marks every
/// section after the first with [`NUMBERING_CONTINUES_CLASS`]. Between
/// two consecutive numbered items it puts that item's sub-content —
/// an indented `'5'` section, and sometimes a `<pre>` code sample.
///
/// **Measured across the 56-document staged corpus.** 60 `'6'` sections
/// produce 60 ordered lists today, 47 of them holding a single item;
/// `CVLAAAgSl7Q`'s 7-step "API Endpoints" procedure renders "1." seven
/// times. Applying the rule below yields **25** lists — 17 genuine
/// singletons plus sequences of 3, 4, 4, 5, 5, 7, 7 and 8 — with all 60
/// items preserved. The two 7s are `CVLAAAgSl7Q` and `NceAAAcEiOG`.
///
/// **What may interrupt a run, and what ends one.** In the whole corpus
/// exactly two kinds of element ever sit between a `'6'` section and the
/// next section continuing it: an indent-wrapped `'5'` section (36) and a
/// `<pre>` (6). A heading, paragraph, table or anything else never does —
/// wherever one of those appears, the next `'6'` section lacks the
/// continues-class, i.e. **Quip itself has ended the sequence there**. So
/// the run is ended by anything that is not a continuation or that item's
/// own sub-content, and the continues-class alone decides which.
///
/// Absorbed content becomes the preceding item's sub-content, which
/// `NodeType::ListItem::valid_children` permits for both kinds
/// (`BulletList` and `CodeBlock`). The `'5'` section is moved **whole**,
/// wrapper `<div>` included, so [`enclosing_section_style`] still resolves
/// it to `'5'` and it stays a bullet list rather than inheriting the
/// enclosing `'6'`.
///
/// Absorbing a `<pre>` is not gated on a continuation actually following:
/// the corpus was checked both ways and the two rules agree on all six
/// occurrences, and un-gated is the simpler invariant.
fn merge_numbered_sections(container: &markup5ever_rcdom::Handle) {
    let children = std::mem::take(&mut *container.children.borrow_mut());
    let mut kept = Vec::new();
    // The `<ul>` accumulating the open sequence's items, if one is open.
    let mut open: Option<markup5ever_rcdom::Handle> = None;

    for child in children {
        let style = attr(&child, "data-section-style");
        let style = style.as_deref().map(str::trim);
        let absorbed = match (&open, style, tag_of(&child).as_deref()) {
            // A continuation: its items join the open list and the now
            // empty section disappears.
            (Some(acc), Some(SECTION_STYLE_ORDERED), _)
                if classes(&child).iter().any(|c| c == NUMBERING_CONTINUES_CLASS) =>
            {
                move_items_into(&child, acc);
                !section_still_has_content(&child)
            }
            // A new sequence: this section's own list becomes the
            // accumulator and the section stays where it is.
            (_, Some(SECTION_STYLE_ORDERED), _) => {
                open = items_list_of(&child);
                false
            }
            // The preceding item's sub-content.
            (Some(acc), Some(SECTION_STYLE_BULLET), _) if is_indent_wrapper(&child) => {
                absorb_into_last_item(acc, &child)
            }
            (Some(acc), _, Some("pre")) => absorb_into_last_item(acc, &child),
            // Anything else ends the sequence — but whitespace between
            // two sections is layout, not an element, and must not.
            _ => {
                if !is_layout_whitespace(&child) {
                    open = None;
                }
                false
            }
        };
        if !absorbed {
            kept.push(child);
        }
    }
    *container.children.borrow_mut() = kept;
}

/// Append `section` to the last `<li>` of `acc`, reporting whether it
/// moved. A list with no item yet has nowhere to put it.
fn absorb_into_last_item(
    acc: &markup5ever_rcdom::Handle,
    section: &markup5ever_rcdom::Handle,
) -> bool {
    let Some(item) = last_item_of(acc) else { return false };
    append_child(&item, section.clone());
    true
}

/// Move every child of `section`'s item-bearing list onto the end of
/// `acc`. Nested lists ride along as siblings and are nested into their
/// own item by [`nest_sibling_lists`] when the traversal reaches `acc`.
fn move_items_into(section: &markup5ever_rcdom::Handle, acc: &markup5ever_rcdom::Handle) {
    let Some(list) = items_list_of(section) else { return };
    for child in std::mem::take(&mut *list.children.borrow_mut()) {
        append_child(acc, child);
    }
}

/// The first list under `handle` that actually holds `<li>` children —
/// which is the *inner* one when Quip has interposed an indent wrapper.
fn items_list_of(handle: &markup5ever_rcdom::Handle) -> Option<markup5ever_rcdom::Handle> {
    let mut stack = vec![handle.clone()];
    while let Some(node) = stack.pop() {
        if matches!(tag_of(&node).as_deref(), Some("ul" | "ol"))
            && node.children.borrow().iter().any(|c| tag_of(c).as_deref() == Some("li"))
        {
            return Some(node);
        }
        for child in node.children.borrow().iter().rev() {
            stack.push(child.clone());
        }
    }
    None
}

/// The last `<li>` child of a list.
fn last_item_of(list: &markup5ever_rcdom::Handle) -> Option<markup5ever_rcdom::Handle> {
    list.children.borrow().iter().rev().find(|c| tag_of(c).as_deref() == Some("li")).cloned()
}

/// Whether a section's outermost list is Quip's bare indent wrapper —
/// a `<ul>` holding no `<li>` of its own, only another list. That
/// wrapper is exactly how Quip marks a `'5'` section as *indented under*
/// the numbered item above it rather than a bullet list in its own right.
fn is_indent_wrapper(section: &markup5ever_rcdom::Handle) -> bool {
    let mut stack = vec![section.clone()];
    while let Some(node) = stack.pop() {
        if matches!(tag_of(&node).as_deref(), Some("ul" | "ol")) {
            return !node.children.borrow().iter().any(|c| tag_of(c).as_deref() == Some("li"));
        }
        for child in node.children.borrow().iter().rev() {
            stack.push(child.clone());
        }
    }
    false
}

/// Whether a drained section still carries anything worth keeping. An
/// emptied `<ul>` does not — `flatten_list` drops an item-less list —
/// but a stray paragraph the section also held does, so the section is
/// only discarded when nothing but empty list scaffolding remains.
fn section_still_has_content(section: &markup5ever_rcdom::Handle) -> bool {
    use markup5ever_rcdom::NodeData;
    let mut stack = vec![section.clone()];
    while let Some(node) = stack.pop() {
        match &node.data {
            NodeData::Text { contents } => {
                if !contents.borrow().trim().is_empty() {
                    return true;
                }
            }
            NodeData::Element { .. } => {
                let tag = tag_of(&node);
                if !matches!(tag.as_deref(), Some("ul" | "ol" | "div")) {
                    return true;
                }
            }
            _ => {}
        }
        for child in node.children.borrow().iter() {
            stack.push(child.clone());
        }
    }
    false
}

/// Whitespace between two block elements: layout, not content, and so
/// not something that ends a numbered sequence.
fn is_layout_whitespace(handle: &markup5ever_rcdom::Handle) -> bool {
    use markup5ever_rcdom::NodeData;
    match &handle.data {
        NodeData::Text { contents } => contents.borrow().trim().is_empty(),
        // Comments and doctypes are dropped by the walker outright.
        NodeData::Element { .. } | NodeData::Document => false,
        _ => true,
    }
}

/// Re-parent `child` onto the end of `parent`'s children.
///
/// Setting the parent link is load-bearing, not bookkeeping:
/// [`enclosing_section_style`] walks *up* from a list to find the
/// `data-section-style` that decides whether it is bullets, numbers or
/// checkboxes. A moved node with a stale parent would be read against
/// the section it came from.
fn append_child(parent: &markup5ever_rcdom::Handle, child: markup5ever_rcdom::Handle) {
    child.parent.set(Some(std::rc::Rc::downgrade(parent)));
    parent.children.borrow_mut().push(child);
}

/// Remove the trailing `<br>` Quip emits as a **line terminator** at the
/// end of every `<li>`, `<td>` and `<th>` (#189).
///
/// Quip's editor closes the content of every list item and every table
/// cell with a `<br/>` immediately before the closing tag. It is a
/// serializer artifact, not something the author typed: kept, it renders
/// as a blank line under every bullet and inside every cell. The Phase-2a
/// fidelity audit (F-3) counted 5483 of them across 47 of the 56 staged
/// documents — very nearly every document that has a list or a table; 659
/// of those are in the five threads checked in under `tests/fixtures/`.
///
/// **Discriminate by position, never by presence.** A `<br>` anywhere
/// else inside the same element *is* authored content: `a<br/>b` in a
/// cell is two lines the author wrote and stays two lines. Only the break
/// that is the last meaningful child is a terminator, and exactly one is
/// removed per cell — `a<br/><br/>` keeps the first.
///
/// Runs as a DOM pre-pass rather than inside the walker so the question
/// is answered against the markup Quip actually wrote; see the ordering
/// comment in [`parse_quip_counting_losses`]. Iterative for the same
/// reason [`flatten_below_depth`] is: it runs on the un-bounded tree.
///
/// # This is the exact opposite of #184 — deliberately, do not unify
///
/// #184 fixed the inverse defect: a `<br>` inside `<pre>` contributed
/// **nothing** and had to *become* a newline, which is why [`raw_text`]
/// now emits `\n` for it. Same element, opposite treatment, because the
/// container differs. Inside `<pre>` every `<br>` is a real line
/// separator, the last one included; inside `<li>`/`<td>`/`<th>` the last
/// one is Quip's terminator and every other one is authored. A single
/// shared "how should the importer treat `<br>`?" rule cannot satisfy
/// both — it would either resurrect the blank lines this pass removes or
/// swallow a line of code. `<pre>` is untouched here precisely because
/// this pass only ever looks at the direct children of a cell, and a
/// `<pre>` child ends the scan like any other element.
fn strip_cell_terminators(root: &markup5ever_rcdom::Handle) {
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        if matches!(tag_of(&node).as_deref(), Some("li" | "td" | "th"))
            && let Some(i) = terminal_break_index(&node)
        {
            node.children.borrow_mut().remove(i);
        }
        for child in node.children.borrow().iter() {
            stack.push(child.clone());
        }
    }
}

/// Index, among `handle`'s direct children, of the `<br>` that terminates
/// it — or `None` when it does not end in one.
///
/// "Last meaningful child" means last once the whitespace-only text nodes
/// and comments Quip's serializer leaves behind are ignored. The shape in
/// the corpus is `<span>…</span>\n\n<br/></li>`: the `\n\n` sits *before*
/// the break, so the break is genuinely the final node — but the scan
/// tolerates whitespace on either side rather than betting on that.
///
/// Anything else — a text node with real content, any other element —
/// ends the scan immediately, because then the `<br>` was not the last
/// thing in the cell and is authored content. Only *direct* children are
/// considered: no document in the corpus spells the terminator as
/// `<span>…<br/></span></li>`, and reaching into the final inline wrapper
/// would be guessing at markup nobody has seen.
fn terminal_break_index(handle: &markup5ever_rcdom::Handle) -> Option<usize> {
    use markup5ever_rcdom::NodeData;
    let children = handle.children.borrow();
    for (i, child) in children.iter().enumerate().rev() {
        match &child.data {
            NodeData::Text { contents } => {
                if !contents.borrow().as_ref().trim().is_empty() {
                    return None;
                }
            }
            NodeData::Comment { .. } => {}
            NodeData::Element { name, .. } if name.local.as_ref().eq_ignore_ascii_case("br") => {
                return Some(i);
            }
            _ => return None,
        }
    }
    None
}

/// The lowercased tag name of an element node, or `None` for anything
/// that is not an element.
fn tag_of(handle: &markup5ever_rcdom::Handle) -> Option<String> {
    use markup5ever_rcdom::NodeData;
    match &handle.data {
        NodeData::Element { name, .. } => Some(name.local.as_ref().to_ascii_lowercase()),
        _ => None,
    }
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
            // A placeholder run (`mention` / `person`) owns its `text` as a
            // label; appending into it would swallow the following prose
            // into the chip.
            Some(last)
                if last.mention.is_none() && last.person.is_none() && last.marks == *marks =>
            {
                last.text.push_str(s)
            }
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

    /// Record a Quip person mention as its own run. `label` is the anchor's
    /// text and becomes the chip's `display`.
    fn push_person(&mut self, person: QuipPerson, label: String) {
        self.spans.push(Span { text: label, person: Some(person), ..Span::default() });
    }

    /// Emit the buffer as a paragraph (dropping it when it holds only
    /// whitespace) and reset. A mention placeholder counts as content
    /// even when its label is empty — dropping the paragraph would take
    /// the pending link with it.
    fn flush(&mut self, out: &mut Vec<QuipBlock>, section_id: Option<String>) {
        let spans = std::mem::take(&mut self.spans);
        if spans
            .iter()
            .all(|s| s.text.trim().is_empty() && s.mention.is_none() && s.person.is_none())
        {
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
    // An empty span still marks a `<br>`, an intra-Quip link or a person
    // mention — dropping it here would silently eat a hard break, a pending
    // link, or an unnamed person's chip.
    spans.retain(|s| {
        !s.text.is_empty() || s.hard_break_before || s.mention.is_some() || s.person.is_some()
    });
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
                // The anchor rides on the item, not on the list we
                // invented to hold it.
                section_id: None,
                items: vec![parse_item(handle)],
            });
        }
        "blockquote" => {
            pending.flush(out, None);
            let mut inner = Vec::new();
            let mut buf = InlineBuf::default();
            walk_children(handle, &mut inner, &Marks::default(), &mut buf);
            buf.flush(&mut inner, None);
            out.push(QuipBlock::Quote { section_id: section_id(handle), blocks: inner });
        }
        "pre" => {
            pending.flush(out, None);
            out.push(QuipBlock::Code {
                language: code_language(handle),
                section_id: section_id(handle),
                text: raw_text(handle),
            });
        }
        "hr" => {
            pending.flush(out, None);
            out.push(QuipBlock::Rule { section_id: section_id(handle) });
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
                section_id: section_id(handle),
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
        "control" => walk_control(handle, out, marks, pending),
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

/// A Quip `<control>` — the wrapper Quip puts around an entity its own
/// client renders. **VERIFIED MARKUP** against a real staged `/2` thread
/// body; two shapes occur:
///
/// 1. **A person mention.** The control wraps an anchor at a Quip URL whose
///    first path segment is the *person's* id:
///
///    ```html
///    <control data-remapped="true" id="SSfACAGTvYT"><a href="https://quip.com/XYJAEA0Sgev">Joel</a></control>
///    ```
///
///    This becomes a `Mention` placeholder carrying the Quip person id and
///    the anchor's label. It is emphatically *not* [`walk_anchor`]'s
///    intra-Quip document link: a **bare** `<a href="https://quip.com/…">`
///    is a folder/thread link and keeps its existing `DocMention` handling.
///    The `<control>` wrapper has to survive `allowed_tags` for the two to
///    be distinguishable at all.
///
///    **`<control>` is NOT exclusive to people — measured.** Across the
///    56-document staged corpus (`s3://…/imports/…/threads/`) there are
///    exactly four `<control>`-wrapped `quip.com` anchors, and only one is a
///    person. There are **zero bare** `quip.com` anchors:
///
///    | href | label | actually |
///    |---|---|---|
///    | `quip.com/XYJAEA0Sgev` | `Joel` | a person |
///    | `quip.com/JAdAOAxYGcQ` | `Family` | a **folder** |
///    | `quip.com/nxkiAdYH4Nvj` | `SDM Opportunity - Alertus Technologies` | a **thread** |
///    | `quip.com/81aBAkO87SsN#temp:C:KdF95ad…` | (section text) | a **thread section** |
///
///    So "wrapped ⇒ person" would be wrong three times in four, and treating
///    a wrapped document link as a person would degrade it to plain `@Title`
///    text — destroying a back-patchable link, the mirror of the bug this
///    fixes. Two rules keep that from happening, and **neither guesses**:
///
///    1. A **fragment** disqualifies a person here and now:
///       [`quip_person_from_url`] rejects it, because sections belong to
///       threads. That decides the fourth row in the walker.
///    2. The other two are not separable from a person in the markup — same
///       tag, same `data-remapped="true"`, same attributes, same URL shape —
///       so the walker deliberately does not try. It emits a *provisional*
///       person and the **worker** decides: an id `/1/users/` returns no
///       profile for is not a person, which becomes
///       [`PersonOutcome::NotAPerson`] and is rewritten back into the
///       `DocMention` [`walk_anchor`] would have produced. Deciding it there
///       costs nothing extra — the lookup already happens, and its answer
///       already distinguishes the two cases.
///
/// 2. **A client-rendered entity with no export content** — a Quip date, say:
///
///    ```html
///    Complete by <control data-remapped="true" id="SSfACAsTxeJ"></control>.
///    ```
///
///    There is nothing to import, so it contributes nothing and the text
///    on either side is left exactly as Quip gave it (`Complete by .`).
///    Recovering the date would mean re-rendering it, which the export
///    simply does not carry.
///
/// Anything else inside a control is transparent passthrough: its text
/// survives, which is what every unknown wrapper in this walker does.
fn walk_control(
    handle: &markup5ever_rcdom::Handle,
    out: &mut Vec<QuipBlock>,
    marks: &Marks,
    pending: &mut InlineBuf,
) {
    if let Some(anchor) = find_descendant(handle, "a")
        && let Some(person) = quip_person_from_url(&attr(&anchor, "href").unwrap_or_default())
    {
        // The label folds into the chip; a *block* nested inside the anchor
        // is hoisted exactly as `walk_anchor` hoists one.
        let (spans, rest) = walk_text_container(&anchor);
        let label: String = spans.iter().map(|s| s.text.as_str()).collect();
        pending.push_person(person, label.trim().to_string());
        if !rest.is_empty() {
            pending.flush(out, None);
            out.extend(rest);
        }
        return;
    }
    walk_children(handle, out, marks, pending);
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

/// Build a list block. `tag_is_ol` is what the *tag* said; the list
/// kind is not decided by the tag alone, because Quip spells all three
/// kinds of list as a bare `<ul>` and puts the discriminator on the
/// wrapping section `<div>` instead (see the section-style table above
/// [`SECTION_STYLE_ORDERED`]).
fn parse_list(handle: &markup5ever_rcdom::Handle, tag_is_ol: bool) -> QuipBlock {
    let mut items = Vec::new();
    collect_items(handle, &mut items);

    // One ancestor walk answers both questions. A section style is a
    // single value, so "numbered" and "checklist" are mutually
    // exclusive by construction — no precedence rule needed.
    let section = enclosing_section_style(handle);
    let ordered = tag_is_ol || section.as_deref() == Some(SECTION_STYLE_ORDERED);

    // A list is a checklist if any item carries real checked state, or
    // if the list itself is marked as one — except for an ordered list,
    // where a checklist-ish class alone is more likely styling on a
    // numbered list, so per-item state is required.
    let has_item_state = items.iter().any(|i| i.checked.is_some());
    let task = has_item_state || (!ordered && list_is_task(handle, section.as_deref()));
    if task {
        // A list marked as a checklist whose items say nothing: the
        // items default to unchecked.
        for item in &mut items {
            item.checked = Some(item.checked.unwrap_or(false));
        }
    }

    QuipBlock::List { ordered, task, section_id: section_id(handle), items }
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
    QuipItem { checked: checked_state(handle), section_id: section_id(handle), blocks }
}

fn parse_table(handle: &markup5ever_rcdom::Handle) -> QuipBlock {
    let mut rows = Vec::new();
    collect_rows(handle, &mut rows);
    QuipBlock::Table { section_id: section_id(handle), rows }
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
        cells.push(QuipCell { header: tag == "th", section_id: section_id(child), blocks });
    }
    QuipRow { section_id: section_id(handle), cells }
}

// ─── UNVERIFIED-MARKUP readers ───────────────────────────────────
//
// Each helper below encodes a guess about how Quip spells a feature.
// They accept every plausible spelling rather than betting on one, and
// they are the only places that need to change when a real `/2` HTML
// sample lands.

/// **UNVERIFIED MARKUP — still unverified after the first real corpus.**
/// Checked state of a checklist item. Accepts:
///
///   - `<li><input type="checkbox" checked>` (presence of the
///     attribute means checked, per the HTML spec)
///   - `<li class="checked">` / `<li class="unchecked">`, and the
///     `-item` / `_item` suffixed variants
///   - `<li data-checked="true|false">`
///
/// Returns `None` when the item carries no checklist signal at all —
/// that's what makes an ordinary bullet stay an ordinary bullet.
///
/// Unlike [`list_is_task`], this one could **not** be reconciled against
/// the 56 staged `/2` documents: the corpus holds exactly one checklist
/// (`data-section-style='7'`) and every one of its four items is
/// *unchecked*. A search of all 56 documents finds no `<input>` element,
/// no `checked` attribute, and no `check`-ish class token anywhere. So
/// how Quip spells a **checked** item remains unknown, and none of the
/// three spellings above has been seen in the wild.
///
/// The consequence is deliberate: a real Quip checklist imports with
/// every item unchecked, because [`parse_list`] defaults a
/// section-marked checklist's items to `false`. Guessing a marker here
/// would silently mis-import task state, which is worse than uniformly
/// unchecked — do not add one without a sample that contains a ticked
/// item.
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

// ─── the Quip `data-section-style` vocabulary ────────────────────
//
// Quip wraps each document *section* in a `<div data-section-style='N'>`.
// For lists that number is the **only** discriminator: a bullet list, a
// numbered list, and a checklist all emit the identical inner markup —
// a bare `<ul>` of `<li id=… class='' style='' value='1'><span>`, with
// no `<ol>`, no `<input type="checkbox">`, no class token, and no
// `data-type`. `value='1'` is boilerplate on the first item of every
// list of every style (60/60, 565/565, 2/2 in the corpus), so it
// carries no ordering information and there is no `start` to honour.
//
// Counted across the 57 real `/2/threads/{id}/html` documents the
// import worker staged to S3 (`imports/*/threads/*.html`):
//
//   | value | wraps     | count | reading                  | status    |
//   |-------|-----------|-------|--------------------------|-----------|
//   | `5`   | `<ul>`    | 565   | bullet list              | CONFIRMED |
//   | `6`   | `<ul>`    | 60    | numbered list            | CONFIRMED |
//   | `7`   | `<ul>`    | 2     | checklist                | CONFIRMED |
//   | `11`  | `<img>`   | 27    | image section            | observed  |
//   | `13`  | `<table>` | 46    | table section            | observed  |
//   | `22`  | `<div>`   | 1     | slide/presentation title | observed  |
//   | `24`  | `<div>`   | 1     | slide/presentation wrap  | observed  |
//
// CONFIRMED means the rendered source text was read back against the
// staged HTML for that value: `6` against a section whose source reads
// `1. Public ownership of infrastructure…` (`CXdAAA3rE44`), `5` against
// one reading `* **Decentralization**: …`, `7` against the stock
// "Welcome to Quip" checklist. Corroborated by content: all 13
// multi-item `6` sections read as ordered sequences (protocol steps,
// game-turn phases, an auth lifecycle) and none reads as an unordered
// bullet list.
//
// `observed` means the value was seen in the corpus and its wrapped
// element noted, but the walker does not act on it and no source text
// was read back — those three readings are inference from the wrapped
// element alone. Values absent from the table were not seen at all;
// do not invent readings for them.

/// Section style of a **bullet** list. Also the style Quip gives the
/// *sub-content* of a numbered item — see [`merge_numbered_sections`].
const SECTION_STYLE_BULLET: &str = "5";

/// Section style of a **numbered** list. Its `<ul>` is indistinguishable
/// from a bullet list's — see the table above.
const SECTION_STYLE_ORDERED: &str = "6";

/// Section style of a **checklist**. Its `<ul>` is indistinguishable
/// from a bullet list's — see the table above.
const SECTION_STYLE_CHECKLIST: &str = "7";

/// Quip's own name for "this numbered section continues the previous
/// one" — it appears on a [`SECTION_STYLE_ORDERED`] section together
/// with `style="--indent0: N"`, where `N` is the number Quip renders.
///
/// **Measured, not guessed.** Across the 56-document staged corpus the
/// class and `--indent0` co-occur exactly: 35 sections carry both, 25
/// carry neither, and no section carries one without the other. In all
/// 25 resulting sequences `N` equals the item's 1-based position, with
/// zero mismatches — so the *class* is a sufficient signal and the
/// number it restarts at is redundant. That is why nothing here reads
/// `style`, which the sanitizer strips (`allowed_attributes`).
///
/// Despite the name, `--indent0` is **not** an indent level: every one
/// of the 35 sections carrying it holds a flat, unnested list. List
/// *nesting* is spelled the other way entirely — see
/// [`nest_sibling_lists`].
const NUMBERING_CONTINUES_CLASS: &str = "list-numbering-restart-at";

/// Whether a list is a checklist independent of its items. `section` is
/// the nearest enclosing `data-section-style`, already resolved by
/// [`parse_list`].
///
/// The load-bearing signal is **confirmed**: the nearest enclosing
/// section wrapper carries [`SECTION_STYLE_CHECKLIST`]. Resolving the
/// *nearest* wrapper is what makes nesting behave — a `5` section inside
/// a `7` section is still a bullet list.
///
/// **UNVERIFIED MARKUP** (retained, additive): a `checklist` /
/// `task-list` / `tasklist` / `todo` class token, or
/// `data-type="taskList"` (the spelling our own HTML export uses). No
/// Quip document in the corpus spells it either way, but keeping them
/// costs nothing and covers other Quip versions and re-imported exports.
fn list_is_task(list: &markup5ever_rcdom::Handle, section: Option<&str>) -> bool {
    if section == Some(SECTION_STYLE_CHECKLIST) {
        return true;
    }
    if let Some(t) = attr(list, "data-type")
        && t.eq_ignore_ascii_case("tasklist")
    {
        return true;
    }
    classes(list).iter().any(|c| {
        matches!(c.as_str(), "checklist" | "task-list" | "tasklist" | "task_list" | "todo" | "todo-list")
    })
}

/// The `data-section-style` of the nearest section wrapper at or above
/// `handle`, or `None` if no ancestor carries one.
///
/// Walking *up* is what keeps this proportionate: the alternative —
/// threading the section style down as a walker parameter — would touch
/// every `walk_*` signature and every recursion site for one attribute
/// that only [`parse_list`] reads.
///
/// The walk stops at the first ancestor bearing the attribute (so the
/// innermost section governs), at a non-element node (the document root),
/// or after [`MAX_NESTING_DEPTH`] hops — the same liveness bound the
/// downward walk uses, since the ancestor chain is third-party markup too.
fn enclosing_section_style(handle: &markup5ever_rcdom::Handle) -> Option<String> {
    use markup5ever_rcdom::NodeData;
    let mut node = Some(handle.clone());
    for _ in 0..MAX_NESTING_DEPTH {
        let current = node?;
        if !matches!(current.data, NodeData::Element { .. }) {
            return None;
        }
        if let Some(style) = attr(&current, "data-section-style")
            && !style.trim().is_empty()
        {
            return Some(style.trim().to_string());
        }
        node = parent_of(&current);
    }
    None
}

/// The parent node of `handle`, if it still has one.
///
/// `markup5ever_rcdom` keeps the parent link in a `Cell<Option<Weak<_>>>`,
/// which has no `get` because `Weak` is not `Copy`. Take-then-put-back is
/// how rcdom itself reads the field; nothing runs between the two halves,
/// so the cell is never observed empty.
fn parent_of(handle: &markup5ever_rcdom::Handle) -> Option<markup5ever_rcdom::Handle> {
    let weak = handle.parent.take();
    let parent = weak.as_ref().and_then(std::rc::Weak::upgrade);
    handle.parent.set(weak);
    parent
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

/// The Quip section anchor of a block element: the plain `id`
/// attribute, with `data-section-id` accepted as an alternative
/// spelling. Quip section ids are **opaque** — no charset validation,
/// no case folding, no length assumption, and never a change to an
/// interior byte. The corpus alone spells them three ways
/// (`SSfACA046uk`, `temp:C:CVL146925f6…`, and the 78-byte composite
/// `temp:s:temp:C:QGYe66f…_temp:C:QGY4a33…` a spreadsheet cell carries),
/// so anything this function "knew" about their shape would be wrong for
/// some document.
///
/// **One exception, and it is a trim, not a normalization.** Surrounding
/// whitespace is stripped and an id that trims to nothing is treated as
/// no id at all. Both are safe for the same reason: a URL fragment
/// cannot carry raw whitespace and is matched against the attribute
/// value as-is, so a padded `id` is markup noise that no anchor could
/// ever name. Trimming makes the map key equal to the string a Quip
/// anchor will actually carry — dropping it would file the entry under a
/// key the back-patch can never look up. Pinned by
/// `a_padded_id_is_trimmed_to_the_string_an_anchor_carries` and
/// `an_empty_id_is_not_an_anchor`.
///
/// # Which elements carry one — measured, not assumed (#190)
///
/// Every block-producing element in the corpus carries an anchor, and
/// each is now read: `<p>` `<h1>`–`<h6>` `<ul>`/`<ol>` `<li>` `<table>`
/// `<tr>` `<td>` `<th>` `<pre>` `<blockquote>` `<img>` (and `<hr>`,
/// which the corpus never emits). Counted across the five checked-in
/// fixtures — 1481 `id` attributes:
///
/// | tag | ids | recorded |
/// |---|---|---|
/// | `span` | 643 | via its parent — see below |
/// | `td` | 518 | yes |
/// | `li` | 125 | yes |
/// | `p` | 61 | yes (already) |
/// | `tr` | 47 | yes |
/// | `ul` | 36 | yes, bar 10 — see below |
/// | `th` | 16 | yes |
/// | `h1`/`h2`/`h3` | 23 | yes (already) |
/// | `pre` | 4 | yes |
/// | `table` | 3 | yes |
/// | `control` | 3 | no — see below |
/// | `blockquote` | 1 | yes |
/// | `img` | 1 | yes |
///
/// **`<span>` needs no entry of its own.** Quip repeats the enclosing
/// `<li>`/`<td>` id verbatim on the inner `<span>` — 643 of 643 span ids
/// in the corpus are byte-identical to their parent's, with zero
/// counter-examples. The map is keyed by section id, so a lookup of a
/// span id already hits its parent's entry. Recording the span
/// separately would add 643 duplicate keys pointing at the same block.
///
/// **Two residues, both measured and both deliberate.**
///
/// 1. Ten `<ul>` ids in `CVLAAAgSl7Q` (170 of 180 distinct ids
///    captured). Those are the numbered sections
///    [`merge_numbered_sections`] absorbs: the continuation `<ul>` is
///    emptied into the accumulator and the now-contentless section is
///    dropped before the walker ever sees it, so its id has no block to
///    name. Corpus-wide that is 35 of 60 `'6'` sections — 0.24% of the
///    14 439 source ids. Their *content* is untouched; only the
///    section-level anchor on a list that no longer exists separately is.
/// 2. Three `<control>` ids in `SSfAAALs7fy`. A `<control>` is an inline
///    entity wrapper, not a section (see [`walk_control`]); two here
///    become `Mention` leaves and the third is empty and materializes
///    nothing at all. No corpus anchor targets one.
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

/// The Quip **person** id in a mention anchor's href.
///
/// Same URL shape as [`quip_thread_from_url`] — `https://quip.com/<ID>` —
/// because Quip addresses a person and a thread identically; only the
/// enclosing `<control>` says which one this is (see [`walk_control`]).
/// Kept as its own helper rather than folded into `quip_thread_from_url` so
/// no change here can alter how a bare document link is classified.
///
/// **A fragment disqualifies a person.** `#temp:C:…` is a *section* anchor,
/// and sections belong to threads; a person has none. This is the one
/// discriminator the staged corpus actually supplies (see [`walk_control`]),
/// so a `<control>`-wrapped `quip.com/<ID>#<SECTION>` falls through to
/// [`walk_anchor`] and keeps its back-patchable `DocMention`.
fn quip_person_from_url(href: &str) -> Option<QuipPerson> {
    let url = resolve_href(href)?;
    if !is_quip_host(url.host_str()?) {
        return None;
    }
    if url.fragment().is_some_and(|f| !f.is_empty()) {
        return None;
    }
    let quip_user_id = url.path_segments()?.find(|s| !s.is_empty())?.to_string();
    Some(QuipPerson { quip_user_id, url: url.to_string() })
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
/// whitespace is content. `<br>` becomes `\n`, including a trailing
/// one: that blank last line is authored, unlike the leading newline
/// html5ever inserts after `<pre>` (stripped below).
fn raw_text(handle: &markup5ever_rcdom::Handle) -> String {
    use markup5ever_rcdom::NodeData;
    let mut out = String::new();
    fn go(handle: &markup5ever_rcdom::Handle, out: &mut String) {
        use markup5ever_rcdom::NodeData;
        match &handle.data {
            NodeData::Text { contents } => out.push_str(contents.borrow().as_ref()),
            // Quip separates code-block lines with `<br>` elements, not
            // literal newlines. A `<br>` carries no text and no children,
            // so without this arm every line of a code block collapses
            // onto one. Unlike the walker's `"br"` arm, a code block has
            // no inline layer to hang a `HardBreak` on — inside `<pre>`
            // the newline *is* the text.
            NodeData::Element { name, .. } if name.local.as_ref().eq_ignore_ascii_case("br") => {
                out.push('\n');
            }
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
        QuipBlock::Quote { section_id, blocks } => {
            let (kept, escaped) = split_for(blocks, NodeType::Blockquote);
            let mut out = vec![QuipBlock::Quote { section_id, blocks: kept }];
            // A blockquote can't be split without changing what reads
            // as quoted, so escapees follow the quote.
            out.extend(escaped);
            out
        }
        QuipBlock::List { ordered, task, section_id, items } => {
            flatten_list(ordered, task, section_id, items)
        }
        QuipBlock::Table { section_id, rows } => flatten_table(section_id, rows),
        other => vec![other],
    }
}

/// Lists split at item boundaries: when an item holds something a list
/// item can't contain, the list closes, the hoisted blocks are emitted,
/// and a fresh list of the same kind resumes. That keeps document order
/// exact at item granularity.
/// `section_id` names the source `<ul>`/`<ol>`, of which there was
/// exactly one; a split produces several lists, so it rides on the
/// **first** of them. Copying it onto each fragment would map one Quip
/// anchor to several blocks, and the back-patch resolves an anchor to a
/// single destination.
fn flatten_list(
    ordered: bool,
    task: bool,
    mut section_id: Option<String>,
    items: Vec<QuipItem>,
) -> Vec<QuipBlock> {
    let item_ctx = if task { NodeType::TaskItem } else { NodeType::ListItem };
    let mut out = Vec::new();
    let mut run: Vec<QuipItem> = Vec::new();

    for item in items {
        let QuipItem { checked, section_id: item_section_id, blocks } = item;
        let (mut kept, escaped) = split_for(blocks, item_ctx);
        if kept.is_empty() {
            // An item whose entire content was hoisted still needs a
            // body — an empty item is legal but renders as a ghost.
            kept.push(empty_para());
        }
        run.push(QuipItem { checked, section_id: item_section_id, blocks: kept });
        if !escaped.is_empty() {
            out.push(QuipBlock::List {
                ordered,
                task,
                section_id: section_id.take(),
                items: std::mem::take(&mut run),
            });
            out.extend(escaped);
        }
    }

    if !run.is_empty() {
        out.push(QuipBlock::List { ordered, task, section_id: section_id.take(), items: run });
    }
    out
}

/// Tables can't be split the way lists can — closing a row mid-table
/// would mangle the grid — so a cell's escapees are emitted *after*
/// the whole table, in row-major order.
fn flatten_table(section_id: Option<String>, rows: Vec<QuipRow>) -> Vec<QuipBlock> {
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
            cells.push(QuipCell { header: cell.header, section_id: cell.section_id, blocks: kept });
        }
        clean_rows.push(QuipRow { section_id: row.section_id, cells });
    }
    let mut out = vec![QuipBlock::Table { section_id, rows: clean_rows }];
    out.extend(after);
    out
}

// ─── #230 / #232: Quip's grid chrome ─────────────────────────────

/// Drop the grid chrome Quip draws around a table: the row-number cell that
/// leads each row, always; and the column-letter header row **only** when the
/// thread is a spreadsheet. Both are renderings of the grid's rulers rather
/// than the user's data, and importing them shifts every value down one row
/// and right one column (#230, #232).
///
/// The two halves of the chrome are removed under different conditions
/// because only one of them is ambiguous, and the corpus says which:
///
/// * The **header row** cannot be told from content by its markup. 17 of the
///   47 tables in the 56-document staged corpus carry this chrome
///   byte-identically and **16 are prose tables** whose `<th>` cells hold
///   real headings ("Access Level", "What it means"). Deleting that row on
///   shape alone would delete real content, so it takes
///   [`QuipThreadKind::Spreadsheet`] — a signal that is not in the HTML at
///   all (#230).
/// * The **gutter column** has no such twin. Of the corpus's 2 492 table
///   cells exactly 151 have no `id`: the 131 row-number cells, the 17 empty
///   2em corners, and 3 `<th>`. Every one of the 131 is a bare `<td>` with no
///   `<span>` and no terminating `<br/>`, and every one of the 2 341 anchored
///   cells — including the 8 whose text is digits only — carries all three.
///   Nothing an author can type produces the gutter's shape, so it needs no
///   thread type and goes on both paths (#232).
///
/// One pass, one predicate, both paths: [`has_grid_chrome`] decides whether
/// this table is ruled at all, and `kind` decides only whether the header row
/// goes with the gutter. Two independent strips keyed on the same shape would
/// be free to disagree.
fn strip_grid_chrome(blocks: &mut [QuipBlock], kind: QuipThreadKind) {
    for block in blocks.iter_mut() {
        match block {
            QuipBlock::Table { rows, .. } => {
                if has_grid_chrome(rows) {
                    if kind == QuipThreadKind::Spreadsheet {
                        rows.remove(0);
                    }
                    // The header row's share of the gutter column is the
                    // empty 2em corner, so this removes it too when the row
                    // is still there — leaving a table one column narrower
                    // with all of its headings.
                    for row in rows.iter_mut() {
                        row.cells.remove(0);
                    }
                }
            }
            // A sheet's table is a top-level block in every real sample, but
            // the walker can nest a table inside either block container, and
            // a pass that silently skipped those would be chrome-stripping
            // that depends on where the table happens to sit.
            QuipBlock::Quote { blocks, .. } => strip_grid_chrome(blocks, kind),
            QuipBlock::List { items, .. } => {
                for item in items.iter_mut() {
                    strip_grid_chrome(&mut item.blocks, kind);
                }
            }
            _ => {}
        }
    }
}

/// Does this table carry Quip's grid chrome?
///
/// The shape, verbatim from `QGYAAAjicgG` and identical in all 17 chrome
/// -bearing tables of the staged corpus:
///
/// ```text
/// <thead><tr>
///   <th class='empty' style='width: 2em'/>                    ← corner
///   <th id='…' class='empty' style='width: 6em'>A<br/></th>   ← column letter
///   …
/// </tr></thead>
/// <tbody><tr id='…'>
///   <td style='background-color:#f0f0f0'>1</td>               ← row number
///   <td id='…' style=''><span id='…'>value</span><br/></td>
///   …
/// ```
///
/// Checked, in order: a leading all-`<th>` row of at least two cells whose
/// first cell is empty and anchorless (the corner); at least one body row;
/// and every body row exactly as wide as the header row and led by an
/// anchorless `<td>` whose only content is digits.
///
/// **Anchorlessness is the load-bearing half**, and the only part of the cell
/// shape that survives into [`QuipCell`]: `section_id` is the `<td>`'s `id`,
/// and Quip mints one for every cell an author typed into. The corpus's two
/// tables with a genuinely numeric leading column (`3 4 5 6 8 9 10` and a
/// year) spell it `<td id='…'><span id='…'>3</span><br/></td>` — id, span and
/// terminator all present, so they fail this predicate on the `id` alone.
/// "The first column is numeric" would have taken them; "the first column is
/// anchorless" does not.
///
/// This says nothing about whether the table is a *spreadsheet*: 16 of the 17
/// are prose. It says the table is **ruled**. What follows from that differs
/// per thread kind — see [`strip_grid_chrome`].
///
/// Deliberately **not** checked: that the row numbers read `1, 2, 3 … N`,
/// which they do in all 17 real tables. The committed corpus fixtures are
/// content-scrubbed, and the scrubber rewrites every digit run to different
/// digits of the same length — so a sequence check would be untestable
/// against the only real markup this repository holds. Width and
/// anchorlessness survive scrubbing; the digits' values do not.
fn has_grid_chrome(rows: &[QuipRow]) -> bool {
    let [header, body @ ..] = rows else { return false };
    if body.is_empty() || header.cells.len() < 2 || !header.cells.iter().all(|c| c.header) {
        return false;
    }
    let corner = &header.cells[0];
    if corner.section_id.is_some() || single_para_text(corner).is_none_or(|t| !t.is_empty()) {
        return false;
    }
    body.iter().all(|row| {
        row.cells.len() == header.cells.len()
            && row.cells.first().is_some_and(|c| {
                !c.header
                    && c.section_id.is_none()
                    && single_para_text(c)
                        .is_some_and(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
            })
    })
}

/// The text of a cell that holds exactly one paragraph — the only shape a
/// chrome cell ever has. `None` for a cell holding anything else, which is
/// itself reason enough to decline to read the table as a grid.
fn single_para_text(cell: &QuipCell) -> Option<String> {
    match cell.blocks.as_slice() {
        [QuipBlock::Para { spans, .. }] => Some(spans.iter().map(|s| s.text.as_str()).collect()),
        _ => None,
    }
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
    person_mentions: Vec<QuipPersonMention>,
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
        match (&span.mention, &span.person) {
            (Some(mention), _) => {
                insert_doc_mention(txn, &scope, container, &span.text, mention, side)
            }
            (None, Some(person)) => {
                insert_person_mention(txn, &scope, container, &span.text, person, side)
            }
            (None, None) => {
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

/// Materialize a Quip person mention as an **unresolved** `Mention` leaf
/// and record it for [`resolve_person_mentions`].
///
/// `user_id` is written empty and [`PENDING_QUIP_USER_ATTR`] carries the
/// Quip id, because the OgreNotes identity behind a Quip person can only be
/// discovered by an async lookup the walker cannot perform. This node shape
/// is therefore an intermediate that must not be persisted: it is
/// `resolve_person_mentions` — a *total* function over these leaves — that
/// either finishes it or replaces it with plain text. Keeping the Quip id
/// on the node (rather than only in the side table) is what makes that
/// function total: it needs no side car to find its own work.
fn insert_person_mention(
    txn: &mut yrs::TransactionMut<'_>,
    scope: &XmlOpenable<'_>,
    container: NodeType,
    label: &str,
    person: &QuipPerson,
    side: &mut SideTables,
) {
    let el = insert_block(txn, scope, container, NodeType::Mention);
    // Matches the editor's own mention shape (`user_id` + `display`, see
    // `frontend/src/editor/commands.rs::insert_user_mention`): `display` is
    // the bare name, the `@` is the chip's styling, not its text.
    el.insert_attribute(txn, "user_id", "");
    el.insert_attribute(txn, "display", label.to_string());
    el.insert_attribute(txn, PENDING_QUIP_USER_ATTR, person.quip_user_id.clone());
    el.insert_attribute(txn, PENDING_QUIP_URL_ATTR, person.url.clone());
    side.person_mentions.push(QuipPersonMention {
        block_id: block_id_of(&*txn, &el),
        quip_user_id: person.quip_user_id.clone(),
        label: label.to_string(),
    });
}

/// Finish every unresolved person mention in `doc`, keyed by Quip person id.
///
/// **Total by construction.** The work list is read off the document itself
/// (every `Mention` leaf still carrying [`PENDING_QUIP_USER_ATTR`]), not
/// from a caller-supplied list, so there is no way to leave a half-built
/// mention behind by forgetting an entry — passing an empty map degrades
/// every one of them. That matters because the alternative failure mode is
/// a chip that names a person and points at nobody.
///
/// - **[`PersonOutcome::User`]**: the leaf gains the real OgreNotes `user_id`
///   and drops the pending attributes. Nothing about the matching — no email,
///   no Quip id — survives on the node; the OgreNotes user id is the only
///   identity a stored `Mention` carries.
/// - **[`PersonOutcome::NoAccount`]** (and any id absent from the map): the
///   leaf is *replaced* by the plain text `@<display>` (or simply removed
///   when there is no label to show). Not left as an empty-`user_id` chip,
///   which would render as a mention of nobody, and emphatically not as a
///   `DocMention` — a real person shown as a missing *document* is wrong
///   however the lookup went. The reader still sees who was mentioned; only
///   the link is missing.
/// - **[`PersonOutcome::NotAPerson`]**: the leaf is replaced by the
///   `DocMention` [`walk_anchor`] would have produced, and the caller is
///   handed the [`QuipPendingLink`] to record. `<control>` is not exclusive
///   to people (see [`walk_control`]), so this is the path a wrapped folder
///   or thread chip takes — degrading it to text instead would destroy a
///   back-patchable link, which is the mirror of the bug this feature fixes.
///
/// Each rewrite re-finds its node by `blockId` under the write transaction
/// rather than by the index collected in phase one, because two of the three
/// branches change a parent's child count. See [`find_pending_mention`].
///
/// Two-phase (collect handles under a read txn, mutate under a write txn)
/// for the same borrow reason as [`crate::blob_ref::rewrite_blob_refs`].
pub fn resolve_person_mentions(
    doc: &Doc,
    resolved: &HashMap<String, PersonOutcome>,
) -> PersonMentionOutcome {
    let pending: Vec<PendingPerson> = {
        let txn = doc.transact();
        let Some(fragment) = txn.get_xml_fragment("content") else {
            return PersonMentionOutcome::default();
        };
        let mut out = Vec::new();
        // A `Mention` is an inline leaf, so it only ever sits inside a
        // text container (`Paragraph` / `Heading`) — never as a direct
        // child of the root fragment. Descending into the fragment's
        // element children therefore reaches all of them.
        for i in 0..fragment.len(&txn) {
            if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
                collect_person_mentions(&txn, &el, &mut out);
            }
        }
        out
    };
    if pending.is_empty() {
        return PersonMentionOutcome::default();
    }

    let mut result = PersonMentionOutcome::default();
    let mut txn = doc.transact_mut();
    for found in &pending {
        // Re-find the node instead of trusting the index collected in phase
        // one. The degrade branch below removes a child and only re-inserts
        // when there is a label to show, so a *sibling* rewritten earlier in
        // this loop can shift every later index in the same parent. Keying on
        // the node's own `blockId` — minted by `insert_block`, unique, and
        // already the handle the side table records — makes each rewrite
        // independent of every other, in any order. It also confirms the
        // target really is a still-pending `Mention` before writing a user id
        // onto it, which an index alone cannot.
        let Some((index, el)) = find_pending_mention(&txn, &found.parent, &found.block_id) else {
            debug_assert!(false, "a collected pending mention vanished before it was decided");
            continue;
        };
        match resolved.get(&found.quip_user_id) {
            Some(PersonOutcome::User(user_id)) => {
                el.insert_attribute(&mut txn, "user_id", user_id.as_str());
                el.remove_attribute(&mut txn, &PENDING_QUIP_USER_ATTR);
                el.remove_attribute(&mut txn, &PENDING_QUIP_URL_ATTR);
            }
            // Not a person at all — a `<control>`-wrapped folder or thread
            // chip. Restore exactly what `walk_anchor` would have built for
            // the same href, so Phase 2b can still back-patch it.
            Some(PersonOutcome::NotAPerson) => {
                found.parent.remove_range(&mut txn, index, 1);
                let doc_el = found
                    .parent
                    .insert(&mut txn, index, XmlElementPrelim::empty(NodeType::DocMention.tag_name()));
                // The blockId is carried over rather than re-minted: it is
                // the handle the pending-link record points at, and reusing
                // it keeps the node's identity stable across the rewrite.
                doc_el.insert_attribute(&mut txn, "blockId", found.block_id.clone());
                doc_el.insert_attribute(&mut txn, "doc_id", "");
                doc_el.insert_attribute(&mut txn, "url", found.url.clone());
                if !found.display.is_empty() {
                    doc_el.insert_attribute(&mut txn, "title", found.display.clone());
                }
                doc_el.insert_attribute(
                    &mut txn,
                    "pending_quip_thread",
                    found.quip_user_id.clone(),
                );
                // No `pending_quip_section`: a fragment-bearing href never
                // reaches this function at all — `quip_person_from_url`
                // rejects it, so it stayed a `DocMention` from the start.
                result.doc_links.push(QuipPendingLink {
                    source_block_id: found.block_id.clone(),
                    target_quip_thread_id: found.quip_user_id.clone(),
                    target_quip_section_id: None,
                });
            }
            Some(PersonOutcome::NoAccount) | None => {
                found.parent.remove_range(&mut txn, index, 1);
                // An empty label (an avatar-only or deleted-user mention)
                // leaves nothing to say, so the chip simply disappears
                // rather than becoming a bare `@`.
                if !found.display.is_empty() {
                    found.parent.insert(
                        &mut txn,
                        index,
                        XmlTextPrelim::new(format!("@{}", found.display)),
                    );
                }
                result.degraded += 1;
            }
        }
    }
    result
}

/// What the importer learned about one Quip person id, as far as
/// [`resolve_person_mentions`] needs to know.
///
/// The three cases have genuinely different right answers, and collapsing
/// any two of them loses content: a real person we cannot link must stay
/// readable text, while an id that is not a person at all must stay a
/// back-patchable document link.
pub enum PersonOutcome {
    /// Matched to this OgreNotes user.
    User(String),
    /// Quip confirms this is a person, but no OgreNotes account matches.
    NoAccount,
    /// Quip returned no profile for the id — it is a folder or a thread that
    /// happened to be wrapped in `<control>`.
    ///
    /// **KNOWN GAP (ticketed).** "Quip showed us no profile" is not only
    /// true of non-people: a **deactivated, cross-org, or token-invisible
    /// person** is omitted the same way, and lands here — becoming a
    /// missing-document chip titled with their name, which is the literal
    /// symptom #175 exists to fix. The two are separable with one extra
    /// batched request per run: ask `/1/threads/?ids=` about the omitted
    /// ids; an id that *is* a thread is a document, and an id that is
    /// neither a user nor a thread is an invisible person and should degrade
    /// to plain text. Deliberately not done on this branch — it is new API
    /// surface, and shipping the verified part first was the call.
    NotAPerson,
}

/// What [`resolve_person_mentions`] did to the document.
#[derive(Default)]
pub struct PersonMentionOutcome {
    /// How many chips degraded to plain `@name` text, so the caller can log
    /// the loss.
    pub degraded: usize,
    /// Links created from chips that turned out not to be people. The caller
    /// **must** append these to [`QuipDocument::pending_links`] before it
    /// writes the unresolved-link row, or Phase 2b cannot back-patch them.
    pub doc_links: Vec<QuipPendingLink>,
}

/// The still-pending `Mention` child of `parent` carrying `block_id`, with
/// its *current* index.
///
/// Matching on the pending attribute as well as the block id means a node
/// this pass has already decided can never be picked up twice: a match drops
/// the attribute and a degrade removes the node outright.
fn find_pending_mention<T: ReadTxn>(
    txn: &T,
    parent: &XmlElementRef,
    block_id: &str,
) -> Option<(u32, XmlElementRef)> {
    (0..parent.len(txn)).find_map(|i| {
        let Some(XmlOut::Element(child)) = parent.get(txn, i) else {
            return None;
        };
        let is_target = child.tag().as_ref() == NodeType::Mention.tag_name()
            && child.get_attribute(txn, PENDING_QUIP_USER_ATTR).is_some()
            && child.get_attribute(txn, "blockId").as_deref() == Some(block_id);
        is_target.then_some((i, child))
    })
}

/// One unresolved `Mention` leaf plus the handle and block id needed to
/// re-find it and rewrite it in place.
struct PendingPerson {
    parent: XmlElementRef,
    block_id: String,
    quip_user_id: String,
    display: String,
    /// The anchor's resolved href, needed only by the
    /// [`PersonOutcome::NotAPerson`] branch.
    url: String,
}

fn collect_person_mentions<T: ReadTxn>(
    txn: &T,
    parent: &XmlElementRef,
    out: &mut Vec<PendingPerson>,
) {
    for i in 0..parent.len(txn) {
        let Some(XmlOut::Element(child)) = parent.get(txn, i) else {
            continue;
        };
        if child.tag().as_ref() == NodeType::Mention.tag_name() {
            if let Some(quip_user_id) = child.get_attribute(txn, PENDING_QUIP_USER_ATTR) {
                out.push(PendingPerson {
                    parent: parent.clone(),
                    block_id: child.get_attribute(txn, "blockId").unwrap_or_default(),
                    quip_user_id,
                    display: child.get_attribute(txn, "display").unwrap_or_default(),
                    url: child.get_attribute(txn, PENDING_QUIP_URL_ATTR).unwrap_or_default(),
                });
            }
            // A `Mention` is a leaf atom — nothing to descend into.
            continue;
        }
        collect_person_mentions(txn, &child, out);
    }
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
        person_mentions: side.person_mentions,
        // Set by `from_quip_html`, which is the only caller that has the
        // parse-time counts; `materialize` alone cannot know.
        deep_nesting_truncated: 0,
        formulas_dropped: 0,
        live_apps_dropped: 0,
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
        QuipBlock::List { task, items, section_id, .. } => {
            let list_type = block.node_type();
            let item_type = if *task { NodeType::TaskItem } else { NodeType::ListItem };
            let list = insert_block(txn, parent, parent_type, list_type);
            side.record_section(&*txn, &list, section_id.as_ref());
            for item in items {
                let li = insert_block(txn, &XmlOpenable::Element(list.clone()), list_type, item_type);
                if item_type == NodeType::TaskItem {
                    li.insert_attribute(txn, "checked", item.checked.unwrap_or(false).to_string());
                }
                side.record_section(&*txn, &li, item.section_id.as_ref());
                let scope = XmlOpenable::Element(li);
                for child in &item.blocks {
                    materialize_block(txn, &scope, item_type, child, side);
                }
            }
        }
        QuipBlock::Quote { section_id, blocks } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Blockquote);
            side.record_section(&*txn, &el, section_id.as_ref());
            let scope = XmlOpenable::Element(el);
            for child in blocks {
                materialize_block(txn, &scope, NodeType::Blockquote, child, side);
            }
        }
        QuipBlock::Code { language, section_id, text } => {
            let el = insert_block(txn, parent, parent_type, NodeType::CodeBlock);
            if !language.is_empty() {
                el.insert_attribute(txn, "language", language.clone());
            }
            side.record_section(&*txn, &el, section_id.as_ref());
            // A code block carries no marks (`NodeType::is_code`), so
            // the returned text handle is deliberately unused.
            insert_text(txn, &el, text);
        }
        QuipBlock::Rule { section_id } => {
            let el = insert_block(txn, parent, parent_type, NodeType::HorizontalRule);
            side.record_section(&*txn, &el, section_id.as_ref());
        }
        QuipBlock::Table { section_id, rows } => {
            let table = insert_block(txn, parent, parent_type, NodeType::Table);
            side.record_section(&*txn, &table, section_id.as_ref());
            for row in rows {
                let row_el = insert_block(
                    txn,
                    &XmlOpenable::Element(table.clone()),
                    NodeType::Table,
                    NodeType::TableRow,
                );
                side.record_section(&*txn, &row_el, row.section_id.as_ref());
                for cell in &row.cells {
                    let cell_type =
                        if cell.header { NodeType::TableHeader } else { NodeType::TableCell };
                    let cell_el = insert_block(
                        txn,
                        &XmlOpenable::Element(row_el.clone()),
                        NodeType::TableRow,
                        cell_type,
                    );
                    side.record_section(&*txn, &cell_el, cell.section_id.as_ref());
                    let scope = XmlOpenable::Element(cell_el);
                    for child in &cell.blocks {
                        materialize_block(txn, &scope, cell_type, child, side);
                    }
                }
            }
        }
        QuipBlock::Image { src, alt, section_id } => {
            let el = insert_block(txn, parent, parent_type, NodeType::Image);
            // Left as the raw Quip value on purpose — the blob
            // side-load pass rewrites it to a durable blob reference,
            // keyed on the blockId recorded alongside it here.
            el.insert_attribute(txn, "src", src.clone());
            if !alt.is_empty() {
                el.insert_attribute(txn, "alt", alt.clone());
            }
            side.record_section(&*txn, &el, section_id.as_ref());
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

    // ─── the sanitizer allowlist (the actual XSS boundary) ───────
    //
    // These live HERE, next to `allowed_tags`/`allowed_attributes`, rather
    // than in `tests/import_fuzz.rs`, for a reason worth stating: the fuzz
    // suite can only observe the **materialized yrs document**, whose element
    // names come from the closed `NodeType` enum. A doc-level assertion
    // therefore cannot fail no matter what the allowlist admits — widen
    // `allowed_tags()` with `iframe` and a doc-level "no iframe survived"
    // property still passes, because materialization was never going to emit
    // one. The boundary that actually decides what a `<script>` in a Quip
    // document does is `sanitize()`, and the only place `sanitize()` is
    // reachable is here.

    /// Tags that must never be admitted: script execution, framing, and
    /// external-resource loading.
    const NEVER_ALLOWED_TAGS: &[&str] =
        &["script", "iframe", "object", "embed", "style", "form", "link", "base", "meta"];

    /// The allowlist sets themselves — the sharpest possible failure, naming
    /// the exact entry someone added.
    #[test]
    fn the_allowlist_admits_no_script_framing_or_event_handler() {
        let tags = allowed_tags();
        for forbidden in NEVER_ALLOWED_TAGS {
            assert!(
                !tags.contains(forbidden),
                "{forbidden:?} must never be an allowed tag — it is script/framing/resource-loading",
            );
        }
        let attrs = allowed_attributes();
        for attr in &attrs {
            assert!(
                !attr.to_ascii_lowercase().starts_with("on"),
                "{attr:?} is an event-handler attribute and must not be allowlisted",
            );
            assert_ne!(attr.to_ascii_lowercase(), "style", "inline style is a payload vector");
        }
    }

    /// The same guarantee behaviorally, through `sanitize` — which also
    /// covers the parts the set assertions cannot see: the `data-` attribute
    /// *prefix* allowance, and ammonia's own URL-scheme filtering.
    #[test]
    fn sanitize_strips_script_framing_and_event_handlers() {
        for tag in NEVER_ALLOWED_TAGS {
            let html = format!("<p>keep</p><{tag}>payload()</{tag}><p>keep2</p>");
            let out = sanitize(&html).to_ascii_lowercase();
            assert!(
                !out.contains(&format!("<{tag}")),
                "sanitize admitted <{tag}>: {out:?}",
            );
            assert!(out.contains("keep"), "sanitize must keep ordinary content: {out:?}");
        }

        // Event handlers, on both an allowed tag and an allowed-with-content
        // one. `img` and `td` are in the Quip allowlist precisely because
        // this importer needs them, which is what makes them the interesting
        // carriers.
        let out = sanitize(
            "<img src=x onerror='steal()' alt=a>             <table><tr><td onclick='steal()'>c</td></tr></table>             <p onmouseover='steal()'>t</p>",
        )
        .to_ascii_lowercase();
        for handler in ["onerror", "onclick", "onmouseover"] {
            assert!(!out.contains(handler), "sanitize admitted {handler}: {out:?}");
        }

        // Script-bearing URL schemes on the tags that carry URLs.
        let out = sanitize(
            "<a href=\"javascript:steal()\">x</a><img src=\"javascript:steal()\">             <a href=\"data:text/html;base64,PHNjcmlwdD4=\">y</a>",
        )
        .to_ascii_lowercase();
        assert!(!out.contains("javascript:"), "sanitize admitted a javascript: URL: {out:?}");
        assert!(!out.contains("data:text/html"), "sanitize admitted a data: HTML URL: {out:?}");

        // The `data-` prefix allowance must not become an `on*` allowance.
        let out = sanitize("<p data-section-id=s1 onfocus='steal()'>t</p>").to_ascii_lowercase();
        assert!(out.contains("data-section-id"), "data-* hints must survive: {out:?}");
        assert!(!out.contains("onfocus"), "the data- prefix must not admit handlers: {out:?}");
    }

    use yrs::types::GetString;
    use yrs::types::xml::XmlOut;

    fn blocks(html: &str) -> Vec<QuipBlock> {
        parse_quip(html)
    }

    /// [`blocks`], as a **spreadsheet** thread — the full grid-chrome strip
    /// of #230 applied: header row and gutter both.
    fn sheet_blocks(html: &str) -> Vec<QuipBlock> {
        let (mut b, _, _) = parse_quip_counting_losses(html);
        strip_grid_chrome(&mut b, QuipThreadKind::Spreadsheet);
        b
    }

    /// [`blocks`], as a **document** thread — what plain `from_quip_html`
    /// does, which since #232 is the gutter strip without the header row.
    /// Distinct from [`blocks`], which is the raw walker with no strip at
    /// all and so cannot say what production produces.
    fn doc_blocks(html: &str) -> Vec<QuipBlock> {
        let (mut b, _, _) = parse_quip_counting_losses(html);
        strip_grid_chrome(&mut b, QuipThreadKind::Document);
        b
    }

    /// A table's cells as `(is_header, text)`, row-major.
    fn table_grid(block: &QuipBlock) -> Vec<Vec<(bool, String)>> {
        let QuipBlock::Table { rows, .. } = block else { panic!("expected a table: {block:?}") };
        rows.iter()
            .map(|r| {
                r.cells
                    .iter()
                    .map(|c| (c.header, single_para_text(c).unwrap_or_else(|| "<multi>".into())))
                    .collect()
            })
            .collect()
    }

    fn doc_xml(quip: &QuipDocument) -> String {
        let txn = quip.doc.transact();
        let root = txn.get_xml_fragment("content").expect("root fragment");
        root.get_string(&txn)
    }

    // ─── content the import knows it does not carry (#191, #192) ──
    //
    // Both of these are *silent* losses today: the data is in the export,
    // nothing reads it, and the imported document gives the reader no hint
    // that anything is missing. The counts below are what lets the worker
    // say so. They are counts and nothing more — neither attribute's value
    // is read, and the assertions that the values do NOT reach the document
    // are as load-bearing as the counts themselves.

    /// Two spreadsheet cells, **verbatim** from
    /// `tests/fixtures/quip/corpus/QGYAAAjicgG.html` — the `<span
    /// formula=…>` inside a `<td>`, its text being the value Quip last
    /// computed, and Quip's `<br/>` cell terminator. Byte-exact including
    /// the `style=''`, the blank lines, and the `temp:s:temp:C:` ids.
    const REAL_FORMULA_CELLS: &str = "<table><tr><td id='temp:s:temp:C:QGY02ded512fb3c4b019236db16b_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd' style=''><span id='temp:s:temp:C:QGY02ded512fb3c4b019236db16b_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd' formula='=D8*D9'>3</span>\n\n<br/></td><td id='temp:s:temp:C:QGY332cf016b08748b09637afc75_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd' style=''><span id='temp:s:temp:C:QGY332cf016b08748b09637afc75_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd' formula='=SUM(D8:D10)'>4</span>\n\n<br/></td></tr></table>";

    #[test]
    fn a_cell_formula_is_counted_as_a_named_loss() {
        let quip = from_quip_html(REAL_FORMULA_CELLS);
        assert_eq!(quip.formulas_dropped, 2, "one per `formula`-bearing cell");
        assert_eq!(quip.live_apps_dropped, 0, "a spreadsheet is not a live app");
    }

    /// The other half of #192, and the half a count cannot state: the
    /// formula does not silently become *content*. Only the value Quip
    /// cached survives into the document, so the reader sees the same
    /// numbers they saw in Quip — and the `=…` string, which would render
    /// as literal text in a document table and as a live formula if the
    /// table were ever loaded as a sheet, reaches nothing.
    #[test]
    fn a_formula_never_reaches_the_document_only_the_value_quip_cached_does() {
        let quip = from_quip_html(REAL_FORMULA_CELLS);
        let xml = doc_xml(&quip);
        assert!(!xml.contains("=D8*D9"), "the formula string must not become content: {xml}");
        assert!(!xml.contains("SUM("), "the formula string must not become content: {xml}");
        assert!(!xml.contains("formula"), "no `formula` attribute reaches a yrs node: {xml}");
        assert!(xml.contains('3') && xml.contains('4'), "the cached values survive: {xml}");
    }

    /// `formula` has to clear the sanitizer or the count above is always
    /// zero — that is the whole reason it was added to `allowed_attributes`.
    #[test]
    fn the_sanitizer_admits_formula_so_the_loss_can_be_counted() {
        let out = sanitize("<td><span formula='=D8*D9'>3</span></td>");
        assert!(out.contains("formula="), "sanitize must keep `formula`: {out:?}");
        assert_eq!(
            from_quip_html("<td><span formula='=D8*D9'>3</span></td>").formulas_dropped,
            1,
            "a stripped attribute would make this a silent loss again",
        );
    }

    /// **Synthetic markup, deliberately.** No corpus fixture carries a live
    /// app (`tests/quip_corpus.rs` records that as a known coverage gap) and
    /// the audit's Kanban thread `dAcAAAm68OG` was never checked in, so
    /// there is no real board to pin this to and inventing one would only
    /// pin the invention. What this asserts is the *mechanism*: an element
    /// carrying `data-live-app-*` is counted once, whatever it renders as
    /// and however many such attributes it has. See `LIVE_APP_ATTR_PREFIX`
    /// for where the attribute name comes from and what happens if it is
    /// wrong.
    #[test]
    fn a_live_app_block_is_counted_once_however_many_attributes_it_carries() {
        let quip = from_quip_html(
            "<div data-live-app-id='kanban' data-live-app-payload='{\"cards\":[]}'>\
             <table><tr><th>To do</th><th>Done</th></tr></table></div>",
        );
        assert_eq!(quip.live_apps_dropped, 1, "one block, not one per attribute");
        assert_eq!(quip.formulas_dropped, 0);
        // The block's rendered scaffolding still imports; the count says
        // what rode in the payload did not.
        let xml = doc_xml(&quip);
        assert!(xml.contains("<table"), "the headings Quip rendered still import: {xml}");
    }

    #[test]
    fn two_live_app_blocks_are_two_losses() {
        let quip = from_quip_html(
            "<div data-live-app-payload='a'>x</div><div data-live-app-payload='b'>y</div>",
        );
        assert_eq!(quip.live_apps_dropped, 2);
    }

    /// An attribute name that is not ASCII must not take the importer down.
    ///
    /// `LIVE_APP_ATTR_PREFIX` is 13 bytes, and the prefix test used to be a
    /// **byte** slice — `n[..13]` — which panics on any name whose 13th byte
    /// falls inside a multi-byte character. `data-emoji-🙂` is exactly that:
    /// `data-emoji-` is 11 bytes and the emoji occupies 11..15, so 13 splits
    /// it. Nothing about the name is live-app-ish; it only has to be the
    /// wrong length in the wrong place.
    ///
    /// This is reachable input, not a curiosity. Non-ASCII attribute names
    /// are legal HTML, html5ever preserves them, and the whole `data-`
    /// namespace is admitted by `generic_attribute_prefixes`, so the
    /// sanitizer hands them straight to the census. The per-thread
    /// `catch_unwind` in the worker would contain the panic, which is
    /// precisely what makes it nasty: the import does not crash, the thread
    /// just burns its attempts and lands `Failed`. A document that imports
    /// today would stop importing — in the change whose entire purpose is to
    /// stop losing content.
    ///
    /// `from_quip_html_never_panics` does not cover this: proptest is not
    /// going to invent a 13-byte-prefixed attribute name.
    #[test]
    fn a_non_ascii_attribute_name_is_not_a_live_app_and_does_not_panic() {
        // Names that must NOT match. The first two straddle byte 13 exactly
        // — `data-emoji-` is 11 bytes and `data-live-ap` is 12, so the emoji
        // spans it either way — and the rest are shorter than the prefix or
        // diverge before it. Every one of these panicked before the fix.
        for name in ["data-emoji-🙂", "data-live-ap🙂", "data-liv🙂-app-payload", "🙂", "data-🙂"] {
            let html = format!("<div {name}='x'>hi</div>");
            let quip = from_quip_html(&html);
            assert_eq!(
                quip.live_apps_dropped, 0,
                "{name:?} is not a live app; it is only an awkward length",
            );
            assert_eq!(quip.formulas_dropped, 0, "{name:?}");
        }

        // Names that must still match — the fix must not turn the detector
        // off. `data-live-app🙂` is one of these, not a near-miss: it really
        // does begin with the 13 ASCII bytes of the prefix, and a prefix
        // match is what this detector promises. What it is followed by,
        // multi-byte or not, is the same "sibling spelling" case as
        // `data-live-app-id`.
        for name in ["data-live-app🙂", "data-live-app-payload", "DATA-LIVE-APP-PAYLOAD"] {
            let html = format!("<div {name}='🙂'>hi</div>");
            let quip = from_quip_html(&html);
            assert_eq!(quip.live_apps_dropped, 1, "{name:?} carries the prefix");
        }
    }

    /// The count has to be a *loss* signal, not a noise generator: an
    /// ordinary document must report nothing, or the report teaches the
    /// reader to ignore it.
    #[test]
    fn an_ordinary_document_reports_neither_loss() {
        let quip = from_quip_html(
            "<h1>Title</h1><p>text</p><table><tr><td>cell</td></tr></table>\
             <div data-section-style='5'><ul><li>item</li></ul></div>",
        );
        assert_eq!(quip.formulas_dropped, 0);
        assert_eq!(quip.live_apps_dropped, 0);
    }

    /// Both counts are of what the *source* had. `flatten_below_depth`
    /// replaces an over-deep subtree with its text, which would take the
    /// attributes with it — so the census runs first, and a document that
    /// hides its spreadsheet under 200 wrappers still reports the formula
    /// it lost rather than reporting only that it was deep.
    #[test]
    fn a_loss_below_the_depth_bound_is_still_counted() {
        let deep = format!(
            "{}<span formula='=A1+A2'>7</span>{}",
            "<div>".repeat(MAX_NESTING_DEPTH + 20),
            "</div>".repeat(MAX_NESTING_DEPTH + 20),
        );
        let quip = from_quip_html(&deep);
        assert!(quip.deep_nesting_truncated > 0, "the fixture must actually exceed the bound");
        assert_eq!(quip.formulas_dropped, 1, "the census must precede the flattening");
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
        assert!(matches!(b[3], QuipBlock::Rule { .. }), "{b:?}");
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
        let QuipBlock::Quote { blocks: inner, .. } = &b[0] else { panic!("expected quote: {b:?}") };
        assert!(matches!(inner[0], QuipBlock::Para { .. }), "{inner:?}");
    }

    #[test]
    fn nested_lists_stay_inside_their_item() {
        // The *standard* HTML nesting shape, `<ul>` inside `<li>`. It
        // pins a real property and stays — but note it has **zero**
        // occurrences in the 56-document staged Quip corpus. Quip
        // spells nesting the other way, as a sibling of the `<li>`;
        // that shape is covered by
        // `a_sibling_nested_list_lands_inside_the_item_that_owns_it`.
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
        let QuipBlock::Table { rows, .. } = &b[0] else { panic!("expected table: {b:?}") };
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
        let QuipBlock::List { ordered: _, task, items, .. } = &b[0] else { panic!("expected list") };
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

    // ─── real Quip section markup ────────────────────────────
    //
    // The fixtures below are copied verbatim out of the HTML the import
    // worker staged to S3 (`imports/*/threads/*.html`), not simplified —
    // the whole bug class this guards against came from inventing a
    // shape that did not match reality.

    /// The checklist section of the stock "Welcome to Quip" document,
    /// exactly as `/2/threads/{id}/html` returns it. The `<div>`'s
    /// `data-section-style='7'` is the *only* checklist signal: the
    /// `<ul>` and `<li>`s are indistinguishable from a bullet list.
    const REAL_CHECKLIST_SECTION: &str = "<div data-section-style='7' class=\"\" style=\"\">\
         <ul id='SSfACAKV4zR'>\
         <li id='SSfACA046uk' class='' style='' value='1'>\
         <span id='SSfACA046uk'>Check off these items in this interactive list as you go.</span>\
         <br/></li>\
         <li id='SSfACASS8II' class='' style=''>\
         <span id='SSfACASS8II'>Click on this line and then click on the comment icon to add \
         comments.</span>\
         <br/></li>\
         </ul></div>";

    /// A bullet section from a different real document in the same
    /// account. Structurally identical to the checklist above apart
    /// from the section-style number.
    const REAL_BULLET_SECTION_5: &str = "<div data-section-style='5' class=\"\" style=\"\">\
         <ul id='temp:C:AAMe9d2056e1b0147cba5889c08b'>\
         <li id='temp:C:AAMb20d17ce24b7405eab4a62b1d' class='' style='' value='1'>\
         <span id='temp:C:AAMb20d17ce24b7405eab4a62b1d'><b>Framework</b>: React.js with \
         TypeScript</span>\
         <br/></li>\
         <li id='temp:C:AAM82481f948c87415b92702a9cc' class='' style=''>\
         <span id='temp:C:AAM82481f948c87415b92702a9cc'><b>State Management</b>: TanStack \
         Query</span>\
         <br/></li>\
         </ul></div>";

    /// A **numbered** section from a third real document — the one whose
    /// source text was read back as `1. Public ownership of
    /// infrastructure…`. Note it is a `<ul>`, not an `<ol>`: only
    /// `data-section-style='6'` says this list is numbered.
    const REAL_ORDERED_SECTION_6: &str = "<div data-section-style='6' class=\"\" style=\"\">\
         <ul id='temp:C:CXdbd91aeab18a64bb9a191e8ff3'>\
         <li id='temp:C:CXd2e8b30bb043c45909e45b2a4a' class='' style='' value='1'>\
         <span id='temp:C:CXd2e8b30bb043c45909e45b2a4a'>Public ownership of infrastructure to \
         avoid de-platforming and complicate regulatory interference</span>\
         <br/></li>\
         <li id='temp:C:CXd20e128a9c8634c12acbd1e584' class='' style=''>\
         <span id='temp:C:CXd20e128a9c8634c12acbd1e584'>End-user controlled trust \
         relationships</span>\
         <br/></li>\
         </ul></div>";

    /// The same numbered style, but with the extra `<ul>` Quip interposes
    /// for an indent level — which puts two hops between the parsed list
    /// and the section wrapper.
    const REAL_ORDERED_SECTION_6_INDENTED: &str =
        "<div data-section-style='6' class=\"\" style=\"\">\
         <ul id='temp:C:AAMba25b35f15e54908801e80b9f'><ul>\
         <li id='temp:C:AAM7cc8c0e0f47640129a85c85ab' class='' style='' value='1'>\
         <span id='temp:C:AAM7cc8c0e0f47640129a85c85ab'>Client connects with JWT in query \
         param</span>\
         <br/></li>\
         </ul></ul></div>";

    #[test]
    fn a_real_quip_checklist_section_is_a_task_list() {
        let b = blocks(REAL_CHECKLIST_SECTION);
        let QuipBlock::List { ordered, task, items, .. } = &b[0] else {
            panic!("expected a list, got {b:?}")
        };
        assert!(!*ordered);
        assert!(*task, "data-section-style='7' on the wrapping div means checklist: {b:?}");
        assert_eq!(items.len(), 2, "{b:?}");
        // Checked state stays uniformly false: no sample of a *checked*
        // Quip item exists, so `checked_state` finds nothing and
        // `parse_list` defaults a section-marked checklist to unchecked.
        assert_eq!(items[0].checked, Some(false));
        assert_eq!(items[1].checked, Some(false));
    }

    #[test]
    fn a_real_quip_bullet_section_stays_a_bullet_list() {
        // The regression that matters: widening list detection must not
        // sweep in the 565 `5` sections that share the shape.
        let b = blocks(REAL_BULLET_SECTION_5);
        let QuipBlock::List { ordered, task, items, .. } = &b[0] else {
            panic!("expected a list, got {b:?}")
        };
        assert!(!*ordered, "data-section-style='5' is a bullet list: {b:?}");
        assert!(!*task, "data-section-style='5' is not a checklist: {b:?}");
        assert!(items.iter().all(|i| i.checked.is_none()), "{b:?}");
    }

    #[test]
    fn a_real_quip_numbered_section_is_an_ordered_list() {
        for (label, html) in [
            ("flat", REAL_ORDERED_SECTION_6),
            ("indented", REAL_ORDERED_SECTION_6_INDENTED),
        ] {
            let b = blocks(html);
            let QuipBlock::List { ordered, task, items, .. } = &b[0] else {
                panic!("expected a list for {label}, got {b:?}")
            };
            assert!(*ordered, "{label}: data-section-style='6' means numbered: {b:?}");
            assert!(!*task, "{label}: a numbered list is not a checklist: {b:?}");
            assert!(items.iter().all(|i| i.checked.is_none()), "{label}: {b:?}");
        }
    }

    #[test]
    fn an_ol_tag_stays_ordered_inside_a_bullet_section() {
        // The tag is still authoritative when it says `<ol>`; the
        // section style only ever *adds* orderedness.
        let b = blocks("<div data-section-style='5'><ol><li>a</li></ol></div>");
        let QuipBlock::List { ordered, .. } = &b[0] else { panic!("expected list: {b:?}") };
        assert!(*ordered, "{b:?}");
    }

    #[test]
    fn the_nearest_section_wrapper_decides_the_list_kind() {
        // A bullet section nested inside a checklist section: the inner
        // `data-section-style` governs, so walking up must stop at the
        // first ancestor that carries one.
        let html = "<div data-section-style='7'><ul><li>outer\
                    <div data-section-style='5'><ul><li>inner</li></ul></div>\
                    </li></ul></div>";
        let b = blocks(html);
        let QuipBlock::List { task, items, .. } = &b[0] else {
            panic!("expected outer list, got {b:?}")
        };
        assert!(*task, "the outer list is inside the '7' section: {b:?}");
        let Some(QuipBlock::List { task: inner_task, .. }) =
            items[0].blocks.iter().find(|blk| matches!(blk, QuipBlock::List { .. }))
        else {
            panic!("expected a nested list inside the item: {b:?}")
        };
        assert!(!*inner_task, "the nested '5' section is a bullet list: {b:?}");
    }

    #[test]
    fn the_nearest_section_wrapper_decides_orderedness_too() {
        // The mirror of the checklist nesting case, both directions:
        // a bullet section inside a numbered one and the reverse. The
        // *inner* `data-section-style` governs each list.
        for (label, html, outer_ordered) in [
            (
                "'5' inside '6'",
                "<div data-section-style='6'><ul><li>outer\
                 <div data-section-style='5'><ul><li>inner</li></ul></div>\
                 </li></ul></div>",
                true,
            ),
            (
                "'6' inside '5'",
                "<div data-section-style='5'><ul><li>outer\
                 <div data-section-style='6'><ul><li>inner</li></ul></div>\
                 </li></ul></div>",
                false,
            ),
        ] {
            let b = blocks(html);
            let QuipBlock::List { ordered, items, .. } = &b[0] else {
                panic!("expected outer list for {label}, got {b:?}")
            };
            assert_eq!(*ordered, outer_ordered, "{label} outer: {b:?}");
            let Some(QuipBlock::List { ordered: inner_ordered, .. }) =
                items[0].blocks.iter().find(|blk| matches!(blk, QuipBlock::List { .. }))
            else {
                panic!("expected a nested list inside the item for {label}: {b:?}")
            };
            assert_eq!(*inner_ordered, !outer_ordered, "{label} inner: {b:?}");
        }
    }

    // ─── #187: Quip's sibling-list nesting ───────────────────
    //
    // Verbatim from `AeOAAAcV1hg`. The nested `<ul>` is a **sibling**
    // of the `<li>` that owns it, and that `<li>` carries
    // `class='parent'`. This is Quip's only nesting spelling: it occurs
    // 470 times across 25 of the 56 staged documents, while the
    // standard `<ul>`-inside-`<li>` shape occurs zero times.
    //
    // The inner `<span>` holds a zero-width space (U+200B), which is
    // what Quip emits for an empty list item — kept as-is rather than
    // tidied away, since tidying fixtures is what hid this bug.
    const REAL_NESTED_LIST_SECTION: &str = "<div data-section-style='5' class=\"\" style=\"\">\
         <ul id='temp:C:AeO6b3a4714314f44579cbb3cf0c'>\
         <li id='temp:C:AeObe85961cb2d4496ea374e229d' class='parent' style='' value='1'>\
         <span id='temp:C:AeObe85961cb2d4496ea374e229d'>Queue</span>\
         <br/></li>\
         <ul><li id='temp:C:AeOff88d93bfbbd411981d8990df' class='' style=''>\
         <span id='temp:C:AeOff88d93bfbbd411981d8990df'>\u{200b}</span>\
         <br/></li></ul>\
         </ul></div>";

    #[test]
    fn a_sibling_nested_list_lands_inside_the_item_that_owns_it() {
        let b = blocks(REAL_NESTED_LIST_SECTION);
        assert_eq!(b.len(), 1, "one list, not a list plus a hoisted one: {b:?}");
        let QuipBlock::List { ordered, task, items, .. } = &b[0] else {
            panic!("expected a list, got {b:?}")
        };
        assert!(!*ordered && !*task, "a '5' section is plain bullets: {b:?}");
        // Before the fix this was 2: `collect_items` hoisted the nested
        // item up to sit beside `Queue` instead of under it.
        assert_eq!(items.len(), 1, "the nested list is not a sibling item: {items:?}");

        let QuipBlock::Para { spans, .. } = &items[0].blocks[0] else {
            panic!("expected the item's own text first: {:?}", items[0].blocks)
        };
        assert_eq!(spans_text(spans), "Queue");

        let QuipBlock::List { items: inner, .. } = &items[0].blocks[1] else {
            panic!("expected the nested list inside the item: {:?}", items[0].blocks)
        };
        assert_eq!(inner.len(), 1, "{inner:?}");
    }

    #[test]
    fn nesting_survives_to_the_materialized_document() {
        // The parse-level shape is only half the claim — the nested list
        // has to be a legal child of `list_item` and survive
        // `enforce_containment` all the way into the yrs tree.
        let doc = from_quip_html(REAL_NESTED_LIST_SECTION);
        let txn = doc.doc.transact();
        let root = txn.get_xml_fragment("content").expect("root fragment");
        let xml = root.get_string(&txn);
        // Two *opening* tags: the outer list and the one inside `Queue`.
        // Counting `bullet_list` unqualified would match the closing tag
        // too and pass on a flattened tree — the exact shape this guards.
        assert_eq!(
            xml.matches("<bullet_list").count(),
            2,
            "the nested list must still be nested in the materialized tree: {xml}"
        );
        let outer = xml.find("<bullet_list").expect("a bullet list");
        let item = xml.find("<list_item").expect("a list item");
        assert!(outer < item, "the nested list sits inside an item, not beside one: {xml}");
        assert_eq!(doc.deep_nesting_truncated, 0, "real nesting must not trip the depth bound");
    }

    // ─── #188: numbered sequences split across sections ──────
    //
    // Verbatim from `CVLAAAgSl7Q` — the seven `data-section-style='6'`
    // sections of its "API Endpoints" procedure, in document order.
    // Only the first lacks `class="list-numbering-restart-at"`; that
    // class is Quip's own marker for "this continues the list above",
    // and it is what makes the run one list rather than seven.
    //
    // The interleaved `'5'` sub-content sections are omitted here so the
    // fixture stays readable — `REAL_NUMBERED_RUN_WITH_SUBCONTENT`
    // below is a contiguous, unedited slice that keeps them.
    const REAL_NUMBERED_RUN: &str = "\
        <div data-section-style='6' class=\"\" style=\"\">\
        <ul id='temp:C:CVLe2c5aea1202145569d907b219'>\
        <li id='temp:C:CVL10e4418acac74dceb9576b131' class='' style='' value='1'>\
        <span id='temp:C:CVL10e4418acac74dceb9576b131'><b>Start Game (POST /games)</b></span>\
        <br/></li></ul></div>\
        <div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 2\">\
        <ul id='temp:C:CVLcd86ff895ce94ad0b32c599ff'>\
        <li id='temp:C:CVLbd9b01872b9e43dc8e479934c' class='' style='' value='1'>\
        <span id='temp:C:CVLbd9b01872b9e43dc8e479934c'><b>Get Game Details (GET /games/{game_id})</b>\
        </span><br/></li></ul></div>\
        <div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 3\">\
        <ul id='temp:C:CVLfef72b21e32c41a0b00d2f5a0'>\
        <li id='temp:C:CVLed73a007acf941fdba391e23f' class='' style='' value='1'>\
        <span id='temp:C:CVLed73a007acf941fdba391e23f'>\
        <b>Submit Event (POST /games/{game_id}/events)</b></span><br/></li></ul></div>\
        <div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 4\">\
        <ul id='temp:C:CVL1c9aa8b08d0d47beb00a1964d'>\
        <li id='temp:C:CVLe148365344114ec1a4374819d' class='' style='' value='1'>\
        <span id='temp:C:CVLe148365344114ec1a4374819d'>\
        <b>Get Event History (GET /games/{game_id}/events?start_index={optional})</b></span>\
        <br/></li></ul></div>\
        <div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 5\">\
        <ul id='temp:C:CVL2191941b354b45268277ac976'>\
        <li id='temp:C:CVL63c5d4f375134f4da09c65247' class='' style='' value='1'>\
        <span id='temp:C:CVL63c5d4f375134f4da09c65247'>\
        <b>Pause/Resume Game (PATCH /games/{game_id}/status)</b></span><br/></li></ul></div>\
        <div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 6\">\
        <ul id='temp:C:CVL3c5efdeac55e400796cb0c145'>\
        <li id='temp:C:CVLb251cf6315f246aabf1645e99' class='' style='' value='1'>\
        <span id='temp:C:CVLb251cf6315f246aabf1645e99'><b>End Game (POST /games/{game_id}/end)</b>\
        </span><br/></li></ul></div>\
        <div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 7\">\
        <ul id='temp:C:CVL51895294c0584c31ba623f201'>\
        <li id='temp:C:CVLf6b0a4e142ee4768877cb98c0' class='' style='' value='1'>\
        <span id='temp:C:CVLf6b0a4e142ee4768877cb98c0'>\
        <b>Send Chat Message (POST /games/{game_id}/chat)</b></span><br/></li></ul></div>";

    /// A **contiguous, unedited** slice of `CVLAAAgSl7Q`: numbered items
    /// 2 and 3 with the `'5'` section that sits between them. That
    /// section is Quip's spelling for item 2's sub-content — note its
    /// `<ul><ul>` indent wrapper, the signal that separates it from a
    /// bullet list standing on its own.
    const REAL_NUMBERED_RUN_WITH_SUBCONTENT: &str = "\
        <div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 2\">\
        <ul id='temp:C:CVLcd86ff895ce94ad0b32c599ff'>\
        <li id='temp:C:CVLbd9b01872b9e43dc8e479934c' class='' style='' value='1'>\
        <span id='temp:C:CVLbd9b01872b9e43dc8e479934c'><b>Get Game Details (GET /games/{game_id})</b>\
        </span><br/></li></ul></div>\
        <div data-section-style='5' class=\"\" style=\"\">\
        <ul id='temp:C:CVL0248652808cc4b1fa0916d9df'><ul>\
        <li id='temp:C:CVLd71640f09c004f7bba2c593a5' class='' style='' value='1'>\
        <span id='temp:C:CVLd71640f09c004f7bba2c593a5'><b>Path Param</b>: game_id.</span>\
        <br/></li>\
        <li id='temp:C:CVL501af88b11394bb592d926964' class='' style=''>\
        <span id='temp:C:CVL501af88b11394bb592d926964'><b>Logic</b>: Fetch from DynamoDB. \
        Optionally reconstruct full state by reducing events (server-side for security).</span>\
        <br/></li>\
        <li id='temp:C:CVL24d7678bf8e6485c828cd6b3d' class='' style=''>\
        <span id='temp:C:CVL24d7678bf8e6485c828cd6b3d'><b>Response</b>: 200 OK with Game JSON \
        (events truncated if large; client requests full if needed).</span><br/></li>\
        <li id='temp:C:CVLe0be914f68f245e3bfe5fbd22' class='' style=''>\
        <span id='temp:C:CVLe0be914f68f245e3bfe5fbd22'><b>Error Handling</b>: 404 if not found, \
        403 if not player.</span><br/></li>\
        </ul></ul></div>\
        <div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 3\">\
        <ul id='temp:C:CVLfef72b21e32c41a0b00d2f5a0'>\
        <li id='temp:C:CVLed73a007acf941fdba391e23f' class='' style='' value='1'>\
        <span id='temp:C:CVLed73a007acf941fdba391e23f'>\
        <b>Submit Event (POST /games/{game_id}/events)</b></span><br/></li></ul></div>";

    /// The text of each item's leading paragraph.
    fn item_texts(items: &[QuipItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| match &i.blocks[0] {
                QuipBlock::Para { spans, .. } => spans_text(spans),
                other => panic!("expected an item paragraph, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_split_numbered_sequence_becomes_one_list_of_seven() {
        let b = blocks(REAL_NUMBERED_RUN);
        // Before the fix: seven separate `ordered_list` blocks, each
        // holding one item — so the reader saw "1." seven times.
        assert_eq!(b.len(), 1, "the seven sections must merge into one list: {b:?}");
        let QuipBlock::List { ordered, task, items, .. } = &b[0] else {
            panic!("expected a list, got {b:?}")
        };
        assert!(*ordered, "a '6' run is numbered: {b:?}");
        assert!(!*task, "a numbered list is not a checklist: {b:?}");
        assert_eq!(
            item_texts(items),
            vec![
                "Start Game (POST /games)",
                "Get Game Details (GET /games/{game_id})",
                "Submit Event (POST /games/{game_id}/events)",
                "Get Event History (GET /games/{game_id}/events?start_index={optional})",
                "Pause/Resume Game (PATCH /games/{game_id}/status)",
                "End Game (POST /games/{game_id}/end)",
                "Send Chat Message (POST /games/{game_id}/chat)",
            ],
            "all seven steps, in source order, numbered 1-7 by position"
        );
    }

    #[test]
    fn an_interleaved_bullet_section_is_the_previous_items_sub_content() {
        let b = blocks(REAL_NUMBERED_RUN_WITH_SUBCONTENT);
        assert_eq!(b.len(), 1, "the '5' section must not terminate the run: {b:?}");
        let QuipBlock::List { ordered, items, .. } = &b[0] else {
            panic!("expected a list, got {b:?}")
        };
        assert!(*ordered, "{b:?}");
        assert_eq!(
            item_texts(items),
            vec![
                "Get Game Details (GET /games/{game_id})",
                "Submit Event (POST /games/{game_id}/events)",
            ],
            "{b:?}"
        );

        // The sub-content hangs off item 1, and stays *bullets* — the
        // moved section keeps its own `'5'` wrapper, so walking up for a
        // section style must not reach the enclosing `'6'`.
        assert_eq!(
            items[0].blocks.len(),
            2,
            "item 1 keeps its text and gains the '5' section: {:?}",
            items[0].blocks
        );
        let QuipBlock::List { ordered: sub_ordered, task: sub_task, items: sub, .. } =
            &items[0].blocks[1]
        else {
            panic!("expected the '5' section nested under item 1: {:?}", items[0].blocks)
        };
        assert!(!*sub_ordered, "sub-content of a numbered item is bullets: {sub:?}");
        assert!(!*sub_task, "{sub:?}");
        assert_eq!(sub.len(), 4, "{sub:?}");
        assert!(item_texts(sub)[0].starts_with("Path Param"), "{sub:?}");
        assert_eq!(items[1].blocks.len(), 1, "item 2 has no sub-content here: {:?}", items[1]);
    }

    #[test]
    fn a_numbered_section_without_the_continues_class_starts_a_new_list() {
        // Quip restarts numbering by *omitting* the class. Two runs must
        // stay two lists, or `CVLAAAgSl7Q`'s 7-step procedure and its
        // 5-component list would merge into one list of twelve.
        let html = format!("{REAL_NUMBERED_RUN}{REAL_NUMBERED_RUN}");
        let b = blocks(&html);
        assert_eq!(b.len(), 2, "a run without the continues-class opens a new list: {b:?}");
        for block in &b {
            let QuipBlock::List { ordered, items, .. } = block else { panic!("{b:?}") };
            assert!(*ordered);
            assert_eq!(items.len(), 7, "{b:?}");
        }
    }

    /// Verbatim `CVLAAAgSl7Q` again — its "Key Components" run, where a
    /// `<pre>` code sample sits *between* two numbered items. Quip's
    /// continues-class spans the code block (the next section is still
    /// `--indent0: 3`), so the sample illustrates item 2 rather than
    /// ending the sequence. The interleaved `'5'` section is omitted; the
    /// three sections kept are unedited.
    ///
    /// A `<pre>` is one of exactly two things the corpus ever puts inside
    /// a numbered run — the other being an indent-wrapped `'5'` section.
    /// It occurs 6 times.
    #[rustfmt::skip]
    const REAL_NUMBERED_RUN_ACROSS_A_CODE_BLOCK: &str = concat!(
        "<div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 2\">\
         <ul id='temp:C:CVLbc2723c01b024e1894cbe3cd4'>\
         <li id='temp:C:CVL4f59e56d490b4640a0d690d98' class='' style='' value='1'>\
         <span id='temp:C:CVL4f59e56d490b4640a0d690d98'><b>GameDetail Component</b></span>\
         <br/></li></ul></div>",
        // One source line: a `<pre>`'s leading spaces are content, and a
        // `\` continuation would eat them.
        "<pre id='temp:C:CVL09a9aeedb1db4947b4fabcc84' class='prettyprint'>#[component]<br>pub fn GameDetail(game_id: String) -&gt; impl IntoView {<br>    let game = create_resource(move || game_id.clone(), |id| async { fetch_game(&amp;id).await });<br>    let state = create_memo(move || reduce_events(&amp;game.get().unwrap().events));<br>    view! {<br>        &lt;div&gt;<br>            &lt;GameBoard state=state /&gt;<br>        &lt;/div&gt;<br>    }<br>}</pre>",
        "<div data-section-style='6' class=\"list-numbering-restart-at\" style=\"--indent0: 3\">\
         <ul id='temp:C:CVL9824e70cfe1c4b0bbe6ed0e56'>\
         <li id='temp:C:CVL7d39064289c345bf88c7b5bb5' class='' style='' value='1'>\
         <span id='temp:C:CVL7d39064289c345bf88c7b5bb5'><b>GameBoard Component</b></span>\
         <br/></li></ul></div>",
    );

    #[test]
    fn a_code_block_between_two_numbered_items_does_not_end_the_run() {
        let b = blocks(REAL_NUMBERED_RUN_ACROSS_A_CODE_BLOCK);
        // Without the `<pre>` arm the code block closes the sequence and
        // this is three blocks — list, code, list — and the corpus's
        // 5-item "Key Components" run splits into 2 + 3.
        assert_eq!(b.len(), 1, "a `<pre>` inside a numbered run is not a terminator: {b:?}");
        let QuipBlock::List { ordered, items, .. } = &b[0] else {
            panic!("expected one list, got {b:?}")
        };
        assert!(*ordered, "{b:?}");
        assert_eq!(
            item_texts(items),
            vec!["GameDetail Component", "GameBoard Component"],
            "the run continues across the code block: {b:?}"
        );

        // The sample becomes item 1's sub-content — legal, since
        // `ListItem::valid_children` includes `CodeBlock`.
        assert_eq!(
            items[0].blocks.len(),
            2,
            "the code block hangs off the item it illustrates: {:?}",
            items[0].blocks
        );
        let QuipBlock::Code { text, .. } = &items[0].blocks[1] else {
            panic!("expected a code block inside item 1: {:?}", items[0].blocks)
        };
        assert!(text.contains("pub fn GameDetail"), "code text survives verbatim: {text:?}");
        assert_eq!(items[1].blocks.len(), 1, "item 2 gains nothing: {:?}", items[1].blocks);
    }

    /// Verbatim, contiguous `AAMAAAUv1cp`: a numbered section followed
    /// immediately by a **flat** `'5'` section — one whose `<ul>` has its
    /// own `<li>` children rather than Quip's bare `<ul><ul>` indent
    /// wrapper. Three such pairs exist in the corpus, against 44 where
    /// the following `'5'` *is* wrapped.
    ///
    /// The wrapper is the whole discriminator: without it a `'5'` section
    /// is a bullet list standing on its own, and swallowing it into the
    /// numbered item above would be a content-structure regression.
    const REAL_FLAT_BULLET_SECTION_AFTER_A_NUMBERED_ONE: &str =
        "<div data-section-style='6' class=\"\" style=\"\">\
         <ul id='temp:C:AAMba25b35f15e54908801e80b9f'><ul>\
         <li id='temp:C:AAM7cc8c0e0f47640129a85c85ab' class='' style='' value='1'>\
         <span id='temp:C:AAM7cc8c0e0f47640129a85c85ab'>Client connects with JWT in query \
         param: ?token=ey...</span><br/></li>\
         <li id='temp:C:AAM70224200fde24de09ab630db2' class='' style=''>\
         <span id='temp:C:AAM70224200fde24de09ab630db2'>Server validates JWT → extracts \
         userId</span><br/></li>\
         <li id='temp:C:AAM5fed81e8312f455d99961c48a' class='' style=''>\
         <span id='temp:C:AAM5fed81e8312f455d99961c48a'>Client sends JSON message: \
         { \"type\": \"subscribe\", \"sessionId\": \"abc123\" }</span><br/></li>\
         <li id='temp:C:AAM0e1c44390d4446b7870876176' class='' style=''>\
         <span id='temp:C:AAM0e1c44390d4446b7870876176'>Server registers actor to session \
         broadcast group</span><br/></li>\
         </ul></ul></div>\
         <div data-section-style='5' class=\"\" style=\"\">\
         <ul id='temp:C:AAM3e8ed59e730b4d2aa6941b14d'>\
         <li id='temp:C:AAMf5fa52d7d2c24fe5af779320f' class='parent' style='' value='1'>\
         <span id='temp:C:AAMf5fa52d7d2c24fe5af779320f'>Message types (JSON):</span>\
         <br/></li>\
         <ul><li id='temp:C:AAM5ca9f0bc4485483980f6fc4f6' class='' style=''>\
         <span id='temp:C:AAM5ca9f0bc4485483980f6fc4f6'>{ \"type\": \"chat\", \"content\": \
         \"Hello!\" }</span><br/></li>\
         <li id='temp:C:AAM5871d4747e6343f2a782ea077' class='' style=''>\
         <span id='temp:C:AAM5871d4747e6343f2a782ea077'>{ \"type\": \"typing\", \"isTyping\": \
         true } (optional)</span><br/></li>\
         <li id='temp:C:AAMd60ebc26c0c6458c836963b9e' class='' style=''>\
         <span id='temp:C:AAMd60ebc26c0c6458c836963b9e'>Server broadcasts to all in session + \
         persists to DynamoDB</span><br/></li></ul>\
         </ul></div>";

    #[test]
    fn a_flat_bullet_section_after_a_numbered_one_is_not_absorbed() {
        let b = blocks(REAL_FLAT_BULLET_SECTION_AFTER_A_NUMBERED_ONE);
        // Without the `is_indent_wrapper` gate this is one block: the
        // bullet list is swallowed into the numbered list's last item.
        assert_eq!(b.len(), 2, "an unwrapped '5' section stands on its own: {b:?}");

        let QuipBlock::List { ordered, items, .. } = &b[0] else { panic!("{b:?}") };
        assert!(*ordered, "{b:?}");
        assert_eq!(items.len(), 4, "the numbered section keeps its four items: {items:?}");
        assert!(
            items.iter().all(|i| i.blocks.len() == 1),
            "no numbered item gains the bullet section: {items:?}"
        );

        let QuipBlock::List { ordered: o, task, items: bullets, .. } = &b[1] else { panic!("{b:?}") };
        assert!(!*o && !*task, "a '5' section is bullets: {b:?}");
        assert_eq!(item_texts(bullets), vec!["Message types (JSON):"], "{bullets:?}");
        // Its own `class='parent'` nesting still applies inside it.
        let QuipBlock::List { items: sub, .. } = &bullets[0].blocks[1] else {
            panic!("expected the nested list inside it: {:?}", bullets[0].blocks)
        };
        assert_eq!(sub.len(), 3, "{sub:?}");
    }

    // ─── regressions: the shapes that already worked ─────────

    #[test]
    fn adjacent_bullet_sections_are_untouched_by_the_numbering_merge() {
        // 565 bullet sections in the corpus; none may be merged, nested
        // or absorbed. Two adjacent `'5'` sections stay two lists.
        let html = format!("{REAL_BULLET_SECTION_5}{REAL_BULLET_SECTION_5}");
        let b = blocks(&html);
        assert_eq!(b.len(), 2, "bullet sections must not merge: {b:?}");
        for block in &b {
            let QuipBlock::List { ordered, task, items, .. } = block else { panic!("{b:?}") };
            assert!(!*ordered && !*task, "{b:?}");
            assert_eq!(items.len(), 2, "{b:?}");
            assert!(
                items.iter().all(|i| i.blocks.len() == 1),
                "a flat bullet list gains no nesting: {items:?}"
            );
        }
    }

    #[test]
    fn a_checklist_next_to_a_numbered_run_stays_a_checklist() {
        // `'7'` is neither a continuation nor sub-content, so it ends the
        // run and keeps its own kind.
        let html = format!("{REAL_NUMBERED_RUN}{REAL_CHECKLIST_SECTION}");
        let b = blocks(&html);
        assert_eq!(b.len(), 2, "{b:?}");
        let QuipBlock::List { ordered, task, items, .. } = &b[1] else { panic!("{b:?}") };
        assert!(*task, "the '7' section is still a checklist: {b:?}");
        assert!(!*ordered, "{b:?}");
        assert_eq!(items.len(), 2, "{b:?}");
        assert!(items.iter().all(|i| i.checked == Some(false)), "{items:?}");
    }

    #[test]
    fn an_explicit_ol_still_works_and_nests() {
        // The `<ol>` tag never appears in the corpus (0 of 1096 list
        // tags), but it is still honoured — and the sibling-nesting
        // rewrite applies to it exactly as it does to `<ul>`.
        let b = blocks("<ol><li>a</li><ol><li>b</li></ol></ol>");
        assert_eq!(b.len(), 1, "{b:?}");
        let QuipBlock::List { ordered, items, .. } = &b[0] else { panic!("{b:?}") };
        assert!(*ordered, "{b:?}");
        assert_eq!(items.len(), 1, "{b:?}");
        let QuipBlock::List { ordered: inner_ordered, .. } = &items[0].blocks[1] else {
            panic!("expected the nested ol inside the item: {:?}", items[0].blocks)
        };
        assert!(*inner_ordered, "{b:?}");
    }

    #[test]
    fn an_indent_wrapper_with_no_bullet_above_it_keeps_its_items() {
        // 52 of the 470 sibling-nested lists in the corpus have no `<li>`
        // before them — Quip's bare `<ul><ul>` indent wrapper. There is
        // nothing to nest into, so the items stay at this level rather
        // than gaining an invented empty parent bullet.
        let b = blocks(REAL_ORDERED_SECTION_6_INDENTED);
        assert_eq!(b.len(), 1, "{b:?}");
        let QuipBlock::List { ordered, items, .. } = &b[0] else { panic!("{b:?}") };
        assert!(*ordered, "{b:?}");
        assert_eq!(items.len(), 1, "the wrapper contributes no empty item: {items:?}");
        assert!(matches!(items[0].blocks[0], QuipBlock::Para { .. }), "{items:?}");
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

    /// #184: Quip writes code-block line breaks as `<br>` elements,
    /// never as literal newlines. This markup is verbatim from the
    /// staged corpus (`imports/*/threads/aLeAAAuK0hD.html`), down to
    /// the NBSP indentation Quip emits and the `&lt;`/`&gt;` entities.
    #[test]
    fn code_block_br_separated_lines_keep_breaks_and_indentation() {
        let html = "<pre id='temp:C:aLe6d44fc8fe4b74926a3ecbbf35' class='prettyprint'>\
            #[derive(Serialize, Deserialize, Clone)]<br>\
            pub struct Lobby {<br>\
            \u{a0}\u{a0} \u{a0}pub lobby_id: String,<br>\
            \u{a0}\u{a0} \u{a0}pub game_type: String,<br>\
            \u{a0}\u{a0} \u{a0}pub host_user_id: String,<br>\
            \u{a0}\u{a0} \u{a0}pub players: Vec&lt;Player&gt;,<br>\
            \u{a0}\u{a0} \u{a0}pub max_players: usize,<br>\
            \u{a0}\u{a0} \u{a0}pub status: String,<br>\
            \u{a0}\u{a0} \u{a0}pub chat_history: Vec&lt;ChatMessage&gt;,<br>\
            \u{a0}\u{a0} \u{a0}pub created_at: u64,<br>\
            \u{a0}\u{a0} \u{a0}pub updated_at: u64,<br>\
            }<br>\
            <br>\
            #[derive(Serialize, Deserialize, Clone)]<br>\
            pub struct Player {<br>\
            \u{a0}\u{a0} \u{a0}pub user_id: String,<br>\
            \u{a0}\u{a0} \u{a0}pub user_name: String,<br>\
            \u{a0}\u{a0} \u{a0}pub status: String, \u{a0}// \"ready\" | \"not_ready\"<br>\
            }<br>\
            <br>\
            #[derive(Serialize, Deserialize, Clone)]<br>\
            pub struct ChatMessage {<br>\
            \u{a0}\u{a0} \u{a0}pub user_id: String,<br>\
            \u{a0}\u{a0} \u{a0}pub message: String,<br>\
            \u{a0}\u{a0} \u{a0}pub timestamp: u64,<br>\
            }</pre>";
        let b = blocks(html);
        let QuipBlock::Code { text, .. } = &b[0] else { panic!("expected code: {b:?}") };
        assert_eq!(
            text,
            "#[derive(Serialize, Deserialize, Clone)]\n\
            pub struct Lobby {\n\
            \u{a0}\u{a0} \u{a0}pub lobby_id: String,\n\
            \u{a0}\u{a0} \u{a0}pub game_type: String,\n\
            \u{a0}\u{a0} \u{a0}pub host_user_id: String,\n\
            \u{a0}\u{a0} \u{a0}pub players: Vec<Player>,\n\
            \u{a0}\u{a0} \u{a0}pub max_players: usize,\n\
            \u{a0}\u{a0} \u{a0}pub status: String,\n\
            \u{a0}\u{a0} \u{a0}pub chat_history: Vec<ChatMessage>,\n\
            \u{a0}\u{a0} \u{a0}pub created_at: u64,\n\
            \u{a0}\u{a0} \u{a0}pub updated_at: u64,\n\
            }\n\
            \n\
            #[derive(Serialize, Deserialize, Clone)]\n\
            pub struct Player {\n\
            \u{a0}\u{a0} \u{a0}pub user_id: String,\n\
            \u{a0}\u{a0} \u{a0}pub user_name: String,\n\
            \u{a0}\u{a0} \u{a0}pub status: String, \u{a0}// \"ready\" | \"not_ready\"\n\
            }\n\
            \n\
            #[derive(Serialize, Deserialize, Clone)]\n\
            pub struct ChatMessage {\n\
            \u{a0}\u{a0} \u{a0}pub user_id: String,\n\
            \u{a0}\u{a0} \u{a0}pub message: String,\n\
            \u{a0}\u{a0} \u{a0}pub timestamp: u64,\n\
            }"
        );
        assert_eq!(text.lines().count(), 26, "one line per <br>");
    }

    /// A `<br>` immediately before `</pre>` is authored content — the
    /// blank last line the writer typed — so it is kept, unlike the
    /// leading newline html5ever inserts after `<pre>`. Markup verbatim
    /// from `imports/*/threads/SVbAAAXPtZK.html`.
    #[test]
    fn code_block_trailing_br_keeps_the_blank_last_line() {
        let html = "<pre id='temp:C:SVbd79494ad826748a3b95756e53' class='prettyprint'>\
            GAME # game-name -&gt; { game-id }<br>\
            game-id # STATUS # game-status -&gt; { change-log... }<br></pre>";
        let b = blocks(html);
        let QuipBlock::Code { text, .. } = &b[0] else { panic!("expected code: {b:?}") };
        assert_eq!(
            text,
            "GAME # game-name -> { game-id }\n\
             game-id # STATUS # game-status -> { change-log... }\n"
        );
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

    // ─── #190: every block kind records its anchor ───────────
    //
    // Every `html` literal in this block is **verbatim** markup lifted
    // out of `tests/fixtures/quip/corpus/`, thread id named in each
    // test. That is the point: `<p>` and `<h1>` were the only two kinds
    // handled before #190 precisely because they were the only two any
    // hand-authored fixture ever put an id on, while 6763 of the real
    // ids sit on `<li>`, `<td>`, `<ul>`, `<table>`, `<pre>` and `<img>`.
    // A simplified re-spelling here would reproduce that blind spot.
    //
    // These assert on the parse stage (`QuipBlock`), which is where the
    // anchor is captured; `from_quip_html` pairing it with a live
    // blockId is asserted separately by
    // `every_block_kinds_anchor_reaches_the_section_map`.

    /// `CVLAAAgSl7Q`, one `data-section-style='5'` section, verbatim.
    /// The `<ul>` and its `<li>` carry **different** ids, and the
    /// `<span>` repeats the `<li>`'s byte for byte.
    #[test]
    fn a_list_and_each_of_its_items_record_their_own_anchor() {
        let html = "<div data-section-style='5' class=\"\" style=\"\"><ul id='temp:C:CVL73809743db7745b7a64a37dc1'><li id='temp:C:CVLdf56cde36a9d40d6b910f0d53' class='' style='' value='1'><span id='temp:C:CVLdf56cde36a9d40d6b910f0d53'><b>Dozen-Turadipi</b>: Stand his ingelitse is ipisci-long dozen drop in Iusmodte.</span>\n\n<br/></li><li id='temp:C:CVL6a4f566a19aa4607b8dbe64a9' class='' style=''><span id='temp:C:CVL6a4f566a19aa4607b8dbe64a9'>Uradipis uradipiscinge, adipisci, man noon adipiscing.</span>\n\n<br/></li></ul></div>";
        let b = blocks(html);
        let QuipBlock::List { section_id, items, .. } = &b[0] else {
            panic!("expected list: {b:?}")
        };
        assert_eq!(
            section_id.as_deref(),
            Some("temp:C:CVL73809743db7745b7a64a37dc1"),
            "the <ul>'s own anchor"
        );
        assert_eq!(items.len(), 2);
        assert_eq!(
            items.iter().map(|i| i.section_id.as_deref()).collect::<Vec<_>>(),
            vec![
                Some("temp:C:CVLdf56cde36a9d40d6b910f0d53"),
                Some("temp:C:CVL6a4f566a19aa4607b8dbe64a9"),
            ],
            "each <li> carries its own anchor, distinct from the list's"
        );
    }

    /// `SSfAAALs7fy`, the corpus's only checklist, verbatim. A
    /// `task_item` is anchored exactly like a `list_item` — the two
    /// take different `NodeType`s in `materialize_block`, so the
    /// checklist path needs its own statement.
    #[test]
    fn a_checklist_item_records_its_anchor_like_any_other_item() {
        let html = "<div data-section-style='7' class=\"\" style=\"\"><ul id='SSfACAKV4zR'><li id='SSfACA046uk' class='' style='' value='1'><span id='SSfACA046uk'>Prize did round night in kind porloremips read is any do.</span>\n\n<br/></li><li id='SSfACASS8II' class='' style=''><span id='SSfACASS8II'>White go kind like man hold white go for ctetura wait he you scingeli.</span>\n\n<br/></li></ul></div>";
        let b = blocks(html);
        let QuipBlock::List { task, section_id, items, .. } = &b[0] else {
            panic!("expected list: {b:?}")
        };
        assert!(*task, "data-section-style='7' is a checklist");
        assert_eq!(section_id.as_deref(), Some("SSfACAKV4zR"));
        assert_eq!(
            items.iter().map(|i| i.section_id.as_deref()).collect::<Vec<_>>(),
            vec![Some("SSfACA046uk"), Some("SSfACASS8II")],
        );
    }

    /// `AeOAAAcV1hg`, the head of one `data-section-style='13'` table
    /// section, verbatim. Table, row and cell each carry a distinct
    /// anchor; the cell's is the composite `temp:s:<row>_<col>` form.
    #[test]
    fn a_table_its_rows_and_its_cells_each_record_their_own_anchor() {
        let html = "<div data-section-style='13'><table id='temp:C:AeOfdbce4eabb6e41df873200fa6' title='Iusmod' style='width: 39.0667em'><tbody><tr id='temp:C:AeO2f2ceb0afd0a41419a2e9fae8'><td id='temp:s:temp:C:AeO2f2ceb0afd0a41419a2e9fae8_temp:C:AeO641271c1e4ec417fa93c8d029' style='text-align: left;vertical-align: middle;' class='bold'><span id='temp:s:temp:C:AeO2f2ceb0afd0a41419a2e9fae8_temp:C:AeO641271c1e4ec417fa93c8d029'>Eiusmod</span>\n\n<br/></td></tr></tbody></table></div>";
        let b = blocks(html);
        let QuipBlock::Table { section_id, rows } = &b[0] else { panic!("expected table: {b:?}") };
        assert_eq!(section_id.as_deref(), Some("temp:C:AeOfdbce4eabb6e41df873200fa6"));
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].section_id.as_deref(),
            Some("temp:C:AeO2f2ceb0afd0a41419a2e9fae8"),
            "the <tr>'s anchor — <tbody> is transparent and must not swallow it"
        );
        assert_eq!(
            rows[0].cells[0].section_id.as_deref(),
            Some(
                "temp:s:temp:C:AeO2f2ceb0afd0a41419a2e9fae8_temp:C:AeO641271c1e4ec417fa93c8d029"
            ),
            "the <td>'s composite anchor, byte for byte"
        );
    }

    /// `QGYAAAjicgG`'s `<thead>`, verbatim. Two things at once: a `<th>`
    /// records its anchor, and the corner `<th class='empty'/>` — which
    /// genuinely has no `id` — records **none**. The `<tr>` inside
    /// `<thead>` has no id either, and must not inherit one.
    #[test]
    fn header_cells_record_their_anchor_and_an_id_less_one_records_none() {
        let html = "<table id='temp:C:QGYcfc9c8f7c7714f4a9955e1b7f'><thead><tr><th class='empty' style='width: 2em'/><th id='temp:C:QGY04be7f796bf1483e87f847ed3' class='empty' style='width: 6em'>A<br/></th></tr></thead></table>";
        let b = blocks(html);
        let QuipBlock::Table { section_id, rows } = &b[0] else { panic!("expected table: {b:?}") };
        assert_eq!(section_id.as_deref(), Some("temp:C:QGYcfc9c8f7c7714f4a9955e1b7f"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].section_id, None, "this <tr> has no id and must not invent one");
        assert!(rows[0].cells.iter().all(|c| c.header), "both cells are <th>");
        assert_eq!(
            rows[0].cells.iter().map(|c| c.section_id.as_deref()).collect::<Vec<_>>(),
            vec![None, Some("temp:C:QGY04be7f796bf1483e87f847ed3")],
        );
    }

    /// `QGYAAAjicgG`'s first body row, verbatim: the row-number `<td>`
    /// carries no `id` while every data cell does. 30 of the sheet's
    /// 510 cells are this shape, so "a block with no id gains none" is
    /// a real corpus case rather than a hypothetical.
    #[test]
    fn a_cell_without_an_id_records_no_anchor() {
        let html = "<table><tbody><tr id='temp:C:QGYe66f22cd7b834833a7ee9dc58'><td style='background-color:#f0f0f0'>1</td><td id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d' style=''><span id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d'>as</span>\n\n<br/></td></tr></tbody></table>";
        let b = blocks(html);
        let QuipBlock::Table { section_id, rows } = &b[0] else { panic!("expected table: {b:?}") };
        assert_eq!(section_id, &None, "this <table> has no id");
        assert_eq!(rows[0].cells[0].section_id, None, "the row-number cell has no id");
        assert_eq!(
            rows[0].cells[1].section_id.as_deref(),
            Some(
                "temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d"
            ),
        );
    }

    /// `CVLAAAgSl7Q`, one `<pre class='prettyprint'>`, verbatim —
    /// including the U+00A0 indentation and the `<br>` line separators
    /// (#184), so capturing the anchor is asserted against the same
    /// bytes the code-block path already has to survive.
    #[test]
    fn a_code_block_records_its_anchor() {
        let html = "<pre id='temp:C:CVLab76d47d4f6b483da9a484729' class='prettyprint'>#[cingel(Itseddoei, Porloremips, Drain)]<br>can rsitam Note {<br>\u{a0}\u{a0} \u{a0}can note_if: Tempor,<br>}</pre>";
        let b = blocks(html);
        let QuipBlock::Code { section_id, text, .. } = &b[0] else {
            panic!("expected code: {b:?}")
        };
        assert_eq!(section_id.as_deref(), Some("temp:C:CVLab76d47d4f6b483da9a484729"));
        assert_eq!(text.lines().count(), 4, "the <br> separators still make lines: {text:?}");
    }

    /// `ZaNAAAU4ELc`, the corpus's one image section, verbatim. Quip
    /// emits a bare `<img>` inside `data-section-style='11'` — never
    /// wrapped in a `<p>` — so the anchor is on the `<img>` itself.
    #[test]
    fn an_image_records_its_anchor() {
        let html = "<div data-section-style='11' style='max-width:100%' class='tall'><img src='https://quip.com/blob/ZaNAAAU4ELc/jG4ISoLLsz9JZ2nahGsoSg' id='temp:C:ZaNb6a7b9e7bd634c549b899bae4' alt=\"2555949508.had\"></img></div>";
        let b = blocks(html);
        let QuipBlock::Image { section_id, src, .. } = &b[0] else {
            panic!("expected image: {b:?}")
        };
        assert_eq!(section_id.as_deref(), Some("temp:C:ZaNb6a7b9e7bd634c549b899bae4"));
        assert_eq!(src, "https://quip.com/blob/ZaNAAAU4ELc/jG4ISoLLsz9JZ2nahGsoSg");
    }

    /// `ZaNAAAU4ELc`, the corpus's one blockquote, verbatim.
    #[test]
    fn a_blockquote_records_its_anchor() {
        let html = "<blockquote id='temp:C:ZaN2ba9bae380d64c8392cc6ae88'>A ounce full a room in Split Cloud are beach stood if pull man let a alike.</blockquote>";
        let b = blocks(html);
        let QuipBlock::Quote { section_id, blocks: inner } = &b[0] else {
            panic!("expected quote: {b:?}")
        };
        assert_eq!(section_id.as_deref(), Some("temp:C:ZaN2ba9bae380d64c8392cc6ae88"));
        assert_eq!(inner.len(), 1, "the quoted paragraph survives");
    }

    /// **Not verbatim, and it cannot be.** `<hr>` occurs **zero** times
    /// in all 56 staged documents (the gap is recorded in
    /// `quip_corpus.rs`'s "known coverage gaps"), so no real anchor
    /// spelling exists to copy. The id below is a corpus `temp:C:` id
    /// moved onto an invented tag: it pins that the `hr` arm reads the
    /// attribute at all, and nothing more. If a Quip document with a
    /// divider ever lands, re-spell this from it.
    #[test]
    fn a_horizontal_rule_records_its_anchor() {
        let b = blocks("<hr id='temp:C:ZaN0b3778496abe4a69842e91aff'>");
        let QuipBlock::Rule { section_id } = &b[0] else { panic!("expected rule: {b:?}") };
        assert_eq!(section_id.as_deref(), Some("temp:C:ZaN0b3778496abe4a69842e91aff"));
        let b = blocks("<hr>");
        assert_eq!(b, vec![QuipBlock::Rule { section_id: None }], "no id, no anchor");
    }

    /// An **empty** id is not an anchor. Quip writes empty attribute
    /// values freely — every `<li>` in the corpus carries `class=''
    /// style=''` — and while none of the five fixtures happens to spell
    /// `id=''`, the walker now reads `id` off nine more element kinds
    /// than it did, so the number of places one could appear went up
    /// nine-fold. An `id=''` recorded as `Some("")` would put a key that
    /// matches no anchor into the map and, worse, collide across every
    /// block that had one.
    #[test]
    fn an_empty_id_is_not_an_anchor() {
        let b = blocks(
            "<ul id=''><li id='' class='' style='' value='1'><span>x</span></li></ul>\
             <table id=''><tr id=''><td id=''>c</td></tr></table>\
             <pre id=''>k</pre><blockquote id=''>q</blockquote>\
             <img src='s' id=''><hr id=''>",
        );
        let out = from_quip_html(
            "<ul id=''><li id='' class='' style='' value='1'><span>x</span></li></ul>\
             <table id=''><tr id=''><td id=''>c</td></tr></table>\
             <pre id=''>k</pre><blockquote id=''>q</blockquote>\
             <img src='s' id=''><hr id=''>",
        );
        assert!(!b.is_empty(), "the content itself still parses");
        assert_eq!(out.sections, Vec::new(), "an empty id records no anchor: {:?}", out.sections);
    }

    /// A padded id is trimmed down to the string an anchor can actually
    /// name. `#temp:C:X` is matched against the attribute value as-is
    /// and a URL fragment cannot carry raw whitespace, so `id=' X '`
    /// could only ever be reached as `X` — filing it under `" X "` would
    /// be a key the Phase-2b back-patch can never look up. The interior
    /// is untouched, which the composite cell id here (spaces would be
    /// invalid inside it, so its bytes must come through unchanged)
    /// states alongside.
    #[test]
    fn a_padded_id_is_trimmed_to_the_string_an_anchor_carries() {
        let b = blocks(
            "<p id='  temp:C:ZaN06198313ef4a4ffc9068829e0\n  '>a</p>\
             <p id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d'>b</p>",
        );
        let QuipBlock::Para { section_id, .. } = &b[0] else { panic!("expected para: {b:?}") };
        assert_eq!(
            section_id.as_deref(),
            Some("temp:C:ZaN06198313ef4a4ffc9068829e0"),
            "padding is stripped, so the key equals what a `#…` fragment carries"
        );
        let QuipBlock::Para { section_id, .. } = &b[1] else { panic!("expected para: {b:?}") };
        assert_eq!(
            section_id.as_deref(),
            Some("temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d"),
            "an unpadded id is passed through byte for byte"
        );
    }

    /// A list that `enforce_containment` **splits** must not put its
    /// anchor on both halves.
    ///
    /// `flatten_list` closes the list, emits the hoisted block and
    /// resumes a fresh list of the same kind — so one source `<ul>`
    /// becomes two `bullet_list` blocks with two different blockIds.
    /// The `<ul>` carried one anchor, and an anchor resolves to one
    /// destination, so `section_id.take()` gives it to the first
    /// fragment only. Cloning it instead would put the **first
    /// duplicate key** into the section map: the same Quip id mapped to
    /// two blocks, with nothing to say which one a link should land on
    /// — exactly the outcome the `<span>` decision was made to avoid.
    ///
    /// **Deliberately synthetic, and it has to be.** Quip never nests a
    /// `<table>` inside an `<li>` — zero occurrences in 56 documents —
    /// so no verbatim markup can reach this path at all. The ids are
    /// real corpus ids (`CVLAAAgSl7Q`'s bullet section) so the *keys*
    /// under test are the shape the map really holds; only the nesting
    /// that triggers the split is invented.
    #[test]
    fn a_split_list_gives_its_anchor_to_the_first_fragment_only() {
        let html = "<ul id='temp:C:CVL73809743db7745b7a64a37dc1'>\
             <li id='temp:C:CVLdf56cde36a9d40d6b910f0d53'>a<table><tr><td>c</td></tr></table></li>\
             <li id='temp:C:CVL6a4f566a19aa4607b8dbe64a9'>b</li></ul>";

        // The split really happens: one <ul> in, two lists out.
        let b = blocks(html);
        assert_eq!(
            b.iter().map(|x| x.node_type()).collect::<Vec<_>>(),
            vec![NodeType::BulletList, NodeType::Table, NodeType::BulletList],
            "the table is hoisted between two halves of the list: {b:?}"
        );

        let out = from_quip_html(html);
        let ul = "temp:C:CVL73809743db7745b7a64a37dc1";
        let landed: Vec<&str> =
            out.sections.iter().filter(|(q, _)| q == ul).map(|(_, b)| b.as_str()).collect();
        assert_eq!(
            landed.len(),
            1,
            "the <ul>'s anchor must name exactly one block; it named {landed:?} — a duplicate \
             key in the section map, which resolves to nothing decidable"
        );

        // Stated in full so the *whole* map is pinned, not just the count:
        // both items keep their own anchors, and the split adds nothing.
        assert_eq!(
            out.sections.iter().map(|(q, _)| q.as_str()).collect::<Vec<_>>(),
            vec![
                ul,
                "temp:C:CVLdf56cde36a9d40d6b910f0d53",
                "temp:C:CVL6a4f566a19aa4607b8dbe64a9",
            ],
            "one entry per anchored block, in document order"
        );

        // The first fragment is the one that got it — the anchor should
        // land where the list started, not where it resumed.
        let txn = out.doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        let first_list = match frag.get(&txn, 0) {
            Some(XmlOut::Element(el)) => el.get_attribute(&txn, "blockId").unwrap_or_default(),
            other => panic!("expected a list first: {other:?}"),
        };
        assert_eq!(landed[0], first_list, "the anchor stayed on the first fragment");
    }

    /// The parse-stage tests above stop at `QuipBlock`; this one runs
    /// the whole pipeline over one document containing every newly
    /// covered kind and asserts each anchor reaches `sections` paired
    /// with a **live** blockId. Markup is verbatim per fragment, from
    /// the four fixtures named inline, concatenated.
    #[test]
    fn every_block_kinds_anchor_reaches_the_section_map() {
        let html = concat!(
            // CVLAAAgSl7Q — bullet section: <ul> + <li>
            "<div data-section-style='5' class=\"\" style=\"\"><ul id='temp:C:CVL73809743db7745b7a64a37dc1'>",
            "<li id='temp:C:CVLdf56cde36a9d40d6b910f0d53' class='' style='' value='1'>",
            "<span id='temp:C:CVLdf56cde36a9d40d6b910f0d53'>Stand his ingelitse.</span>\n\n<br/></li></ul></div>",
            // CVLAAAgSl7Q — <pre>
            "<pre id='temp:C:CVLab76d47d4f6b483da9a484729' class='prettyprint'>can rsitam Note {<br>}</pre>",
            // AeOAAAcV1hg — table section: <table> + <tr> + <td>
            "<div data-section-style='13'><table id='temp:C:AeOfdbce4eabb6e41df873200fa6' title='Iusmod' style='width: 39.0667em'>",
            "<tbody><tr id='temp:C:AeO2f2ceb0afd0a41419a2e9fae8'>",
            "<td id='temp:s:temp:C:AeO2f2ceb0afd0a41419a2e9fae8_temp:C:AeO641271c1e4ec417fa93c8d029' style='text-align: left;vertical-align: middle;' class='bold'>",
            "<span id='temp:s:temp:C:AeO2f2ceb0afd0a41419a2e9fae8_temp:C:AeO641271c1e4ec417fa93c8d029'>Eiusmod</span>\n\n<br/></td></tr></tbody></table></div>",
            // ZaNAAAU4ELc — image section and blockquote
            "<div data-section-style='11' style='max-width:100%' class='tall'><img src='https://quip.com/blob/ZaNAAAU4ELc/jG4ISoLLsz9JZ2nahGsoSg' id='temp:C:ZaNb6a7b9e7bd634c549b899bae4' alt=\"2555949508.had\"></img></div>",
            "<blockquote id='temp:C:ZaN2ba9bae380d64c8392cc6ae88'>A ounce full a room.</blockquote>",
        );
        let out = from_quip_html(html);
        let captured: Vec<&str> = out.sections.iter().map(|(q, _)| q.as_str()).collect();
        assert_eq!(
            captured,
            vec![
                "temp:C:CVL73809743db7745b7a64a37dc1", // <ul>
                "temp:C:CVLdf56cde36a9d40d6b910f0d53", // <li> (and its <span>)
                "temp:C:CVLab76d47d4f6b483da9a484729", // <pre>
                "temp:C:AeOfdbce4eabb6e41df873200fa6", // <table>
                "temp:C:AeO2f2ceb0afd0a41419a2e9fae8", // <tr>
                "temp:s:temp:C:AeO2f2ceb0afd0a41419a2e9fae8_temp:C:AeO641271c1e4ec417fa93c8d029", // <td>
                "temp:C:ZaNb6a7b9e7bd634c549b899bae4", // <img>
                "temp:C:ZaN2ba9bae380d64c8392cc6ae88", // <blockquote>
            ],
            "document order, one entry per anchored block"
        );

        // Every recorded blockId names an element that exists.
        let txn = out.doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        let mut ids = std::collections::HashSet::new();
        for_each_element(&txn, &frag, &mut |txn, el| {
            ids.insert(el.get_attribute(txn, "blockId").unwrap_or_default());
        });
        for (section, block_id) in &out.sections {
            assert_eq!(block_id.len(), 10, "{section}: minted blockId shape");
            assert!(ids.contains(block_id), "{section} points at a live blockId");
        }
    }

    /// Quip repeats the `<li>`/`<td>` id verbatim on the inner
    /// `<span>` — 643 of 643 span ids in the corpus, zero
    /// counter-examples. So the span needs **no entry of its own**: a
    /// lookup of the span's id already hits the item's entry. This
    /// pins the reasoning, and would go red if Quip ever started
    /// giving the span a distinct id (at which point spans need
    /// capturing too).
    #[test]
    fn a_cell_span_repeats_its_parents_anchor_so_needs_no_entry_of_its_own() {
        // QGYAAAjicgG, verbatim: <td id=X><span id=X>.
        let html = "<table><tr id='temp:C:QGYe66f22cd7b834833a7ee9dc58'><td id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d' style=''><span id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d'>as</span>\n\n<br/></td></tr></table>";
        let out = from_quip_html(html);
        let captured: Vec<&str> = out.sections.iter().map(|(q, _)| q.as_str()).collect();
        assert_eq!(
            captured,
            vec![
                "temp:C:QGYe66f22cd7b834833a7ee9dc58",
                "temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d",
            ],
            "the span's id resolves through its cell's entry, not a duplicate one"
        );
    }

    // ─── tables ──────────────────────────────────────────────

    #[test]
    fn table_rows_and_header_cells_parse() {
        let b = blocks("<table><tr><th>H</th></tr><tr><td>C</td></tr></table>");
        let QuipBlock::Table { rows, .. } = &b[0] else { panic!("expected table") };
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
        let QuipBlock::Table { rows, .. } = &b[0] else { panic!("expected table") };
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows[0].cells[0].header);
    }

    // ─── #230 / #232: the grid-chrome detector declines ──────
    //
    // The positive case — a whole real sheet losing its chrome, and five real
    // prose tables losing their gutter — is pinned against the committed
    // fixtures in `tests/quip_corpus.rs`. What belongs here is the other
    // half: the tables the detector must NOT touch, on either thread path.
    // Every input below is markup that already appears verbatim in this file
    // or in a committed fixture, so each is a shape Quip is known to emit
    // rather than one invented to make a branch fire.

    /// `QGYAAAjicgG`'s two-cell `<thead>`, verbatim — the same slice
    /// [`a_header_row_with_no_body_row_is_not_a_grid`] uses.
    const REAL_HEAD: &str = "<table id='temp:C:QGYcfc9c8f7c7714f4a9955e1b7f'><thead><tr><th class='empty' style='width: 2em'/><th id='temp:C:QGY04be7f796bf1483e87f847ed3' class='empty' style='width: 6em'>A<br/></th></tr></thead><tbody>";

    /// A body row of `QGYAAAjicgG` led by its `#f0f0f0` row-number cell,
    /// verbatim — the same slice
    /// [`a_numeric_leading_column_with_no_header_row_is_not_a_grid`] uses.
    const REAL_RULED_ROW: &str = "<tr id='temp:C:QGYe66f22cd7b834833a7ee9dc58'><td style='background-color:#f0f0f0'>1</td><td id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d' style=''><span id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d'>as</span>\n\n<br/></td></tr>";

    /// The same row with its leading cell replaced by an **authored** cell
    /// whose text is digits only — `<td id='…'><span id='…'>1</span><br/>`,
    /// lifted verbatim from `QGYAAAjicgG`'s D-column numeric run.
    ///
    /// This is the case the whole #232 discriminator turns on, and the corpus
    /// says it is real: `DbFAAApjMFp` and `fbTAAAkPTCa` between them hold 8
    /// leading cells of this exact shape, carrying `3 4 5 6 8 9 10` and a
    /// year. All 8 have an `id`; none of the 131 gutter cells does.
    const REAL_AUTHORED_NUMERIC_ROW: &str = "<tr id='temp:C:QGYe66f22cd7b834833a7ee9dc58'><td id='temp:s:temp:C:QGYc5941d52725344448a2cfd883_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd' style=''><span id='temp:s:temp:C:QGYc5941d52725344448a2cfd883_temp:C:QGY2d8b7a16c9bb42f5bf0bc2efd'>1</span>\n\n<br/></td><td id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d' style=''><span id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d'>as</span>\n\n<br/></td></tr>";

    /// **#232, the whole of it in one comparison.** The same ruled table down
    /// both paths: the gutter column goes either way, and the header row goes
    /// only for a spreadsheet.
    ///
    /// The corpus states this on five real prose tables; this states it on
    /// the smallest table that can carry the chrome, which is where an
    /// off-by-one in the strip is easiest to read.
    #[test]
    fn the_gutter_goes_on_both_paths_and_the_header_row_only_on_the_sheets() {
        let html = format!("{REAL_HEAD}{REAL_RULED_ROW}</tbody></table>");

        // Document: header row kept, one column narrower. The corner `<th>`
        // is the header row's share of the gutter column, so `A` — a real
        // heading's position in a prose table — is what is left.
        assert_eq!(
            table_grid(&doc_blocks(&html)[0]),
            vec![vec![(true, "A".to_string())], vec![(false, "as".to_string())]],
            "the ruler goes, the heading stays",
        );

        // Spreadsheet: the header row goes too, leaving only the data cell.
        assert_eq!(
            table_grid(&sheet_blocks(&html)[0]),
            vec![vec![(false, "as".to_string())]],
            "#230's outcome, unchanged",
        );

        // The unstripped walk, for contrast — this is what both used to be.
        assert_eq!(
            table_grid(&blocks(&html)[0]),
            vec![
                vec![(true, String::new()), (true, "A".to_string())],
                vec![(false, "1".to_string()), (false, "as".to_string())],
            ],
            "the source shape",
        );

        // Again through the public entry point rather than the pass, because
        // `from_quip_html_as` is where the thread kind is read and a strip
        // that ran only for spreadsheets would look identical above.
        let xml = doc_xml(&from_quip_html(&html));
        assert_eq!(xml.matches("<table_header").count(), 1, "the heading, not the corner: {xml}");
        assert_eq!(xml.matches("<table_cell").count(), 1, "the data cell, not the ruler: {xml}");
        let sheet_xml = doc_xml(&from_quip_html_as(&html, QuipThreadKind::Spreadsheet));
        assert_eq!(sheet_xml.matches("<table_header").count(), 0, "{sheet_xml}");
        assert_eq!(sheet_xml.matches("<table_cell").count(), 1, "{sheet_xml}");
    }

    /// **#232's negative control, and the sharpest one.** Byte-identical
    /// chrome above it, a leading column that is digits only — and it stays,
    /// on both paths, because the cell is *anchored*.
    ///
    /// This is what makes the discriminator "the cell shape" and not "the
    /// first column is numeric". A prose table whose author typed `1`, `2`,
    /// `3` down column A produces exactly this markup, and 8 such cells exist
    /// in the staged corpus.
    #[test]
    fn an_authored_numeric_leading_column_under_the_same_head_is_not_the_gutter() {
        let html = format!("{REAL_HEAD}{REAL_AUTHORED_NUMERIC_ROW}</tbody></table>");
        let source = table_grid(&blocks(&html)[0]);
        assert_eq!(
            source,
            vec![
                vec![(true, String::new()), (true, "A".to_string())],
                vec![(false, "1".to_string()), (false, "as".to_string())],
            ],
            "the shape under test — a numeric leading column beneath a real <thead>",
        );
        assert_eq!(table_grid(&doc_blocks(&html)[0]), source, "the document path takes nothing");
        assert_eq!(table_grid(&sheet_blocks(&html)[0]), source, "nor does the spreadsheet path");

        // And through the public entry point, for the same reason as above.
        for xml in [
            doc_xml(&from_quip_html(&html)),
            doc_xml(&from_quip_html_as(&html, QuipThreadKind::Spreadsheet)),
        ] {
            assert_eq!(xml.matches("<table_header").count(), 2, "both <th> stay: {xml}");
            assert_eq!(xml.matches("<table_cell").count(), 2, "both <td> stay: {xml}");
        }
    }

    /// `AeOAAAcV1hg`'s table, verbatim — a prose table, no `<thead>`, no
    /// gutter — pushed through the **spreadsheet** path.
    ///
    /// Nothing about it may move. This is the test that says the strip is
    /// gated on the table's shape and not merely on the thread's type: a
    /// spreadsheet thread that also holds an ordinary table keeps it whole.
    #[test]
    fn a_spreadsheet_threads_non_grid_table_is_left_alone() {
        let html = "<div data-section-style='13'><table id='temp:C:AeOfdbce4eabb6e41df873200fa6' title='Iusmod' style='width: 39.0667em'><tbody><tr id='temp:C:AeO2f2ceb0afd0a41419a2e9fae8'><td id='temp:s:temp:C:AeO2f2ceb0afd0a41419a2e9fae8_temp:C:AeO641271c1e4ec417fa93c8d029' style='text-align: left;vertical-align: middle;' class='bold'><span id='temp:s:temp:C:AeO2f2ceb0afd0a41419a2e9fae8_temp:C:AeO641271c1e4ec417fa93c8d029'>Eiusmod</span>\n\n<br/></td></tr></tbody></table></div>";
        assert_eq!(
            table_grid(&sheet_blocks(html)[0]),
            table_grid(&blocks(html)[0]),
            "a prose table imports identically either way",
        );
    }

    /// `QGYAAAjicgG`'s `<thead>` with no `<tbody>` after it, verbatim — the
    /// same slice `header_cells_record_their_anchor_and_an_id_less_one_
    /// records_none` asserts against.
    ///
    /// Half the chrome is not the chrome. A header row alone says nothing
    /// about whether the row beneath it is a ruler or data, and a detector
    /// that stripped on this alone would eat the first row of any table
    /// whose header happened to start with an empty cell.
    #[test]
    fn a_header_row_with_no_body_row_is_not_a_grid() {
        let html = "<table id='temp:C:QGYcfc9c8f7c7714f4a9955e1b7f'><thead><tr><th class='empty' style='width: 2em'/><th id='temp:C:QGY04be7f796bf1483e87f847ed3' class='empty' style='width: 6em'>A<br/></th></tr></thead></table>";
        let grid = table_grid(&sheet_blocks(html)[0]);
        assert_eq!(grid.len(), 1, "the header row survives: {grid:?}");
        assert_eq!(grid[0], vec![(true, String::new()), (true, "A".into())]);
    }

    /// `QGYAAAjicgG`'s first body row with no `<thead>` above it, verbatim —
    /// the same slice `a_cell_without_an_id_records_no_anchor` uses.
    ///
    /// The other half, and the more dangerous one: a numeric leading column
    /// is an ordinary thing for a table to have. Without the column-letter
    /// header row above it there is nothing to say those numbers are a
    /// ruler, so they stay.
    #[test]
    fn a_numeric_leading_column_with_no_header_row_is_not_a_grid() {
        let html = "<table><tbody><tr id='temp:C:QGYe66f22cd7b834833a7ee9dc58'><td style='background-color:#f0f0f0'>1</td><td id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d' style=''><span id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d'>as</span>\n\n<br/></td></tr></tbody></table>";
        let grid = table_grid(&sheet_blocks(html)[0]);
        assert_eq!(grid, vec![vec![(false, "1".into()), (false, "as".into())]], "{grid:?}");
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
            section_id: None,
            items: vec![QuipItem {
                checked: None,
                section_id: None,
                blocks: vec![QuipBlock::Image {
                    src: "x".into(),
                    alt: String::new(),
                    section_id: None,
                }],
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

    // ─── #189: Quip's per-cell line terminator ───────────────
    //
    // Every string below starts as markup copied byte-for-byte out of
    // `tests/fixtures/quip/corpus/` — the same real thread bodies the
    // regression net loads.
    //
    // Four of them then edit that markup, because the shape being tested
    // does not occur in the corpus at all: every `<br>` outside a `<pre>`
    // in all five fixtures is a terminator, so a mid-content break, a
    // doubled break, a break-only item and a whitespace-after-the-break
    // item all have to be constructed. Each edit is a single insertion or
    // deletion, called out in that test's own doc comment. Everything
    // around it — the `id`s, the quote style, the `\n\n` before the
    // break — is untouched, because those are exactly the details
    // hand-authored fixtures got wrong in this feature seven times
    // running.

    /// Rendered shape of a span run: text, with `⏎` marking a span that
    /// carries `hard_break_before`. Assert-on-this rather than on a count,
    /// so a test says *where* a break is, not merely how many there are.
    fn spans_shape(spans: &[Span]) -> String {
        spans
            .iter()
            .map(|s| if s.hard_break_before { format!("⏎{}", s.text) } else { s.text.clone() })
            .collect()
    }

    fn only_item_shape(html: &str) -> String {
        match blocks(html).as_slice() {
            [QuipBlock::List { items, .. }] => match items.as_slice() {
                [QuipItem { blocks, .. }] => match blocks.as_slice() {
                    [QuipBlock::Para { spans, .. }] => spans_shape(spans),
                    other => panic!("expected one paragraph, got {other:?}"),
                },
                other => panic!("expected one item, got {other:?}"),
            },
            other => panic!("expected one list, got {other:?}"),
        }
    }

    /// `SSfAAALs7fy`, first checklist item, verbatim.
    #[test]
    fn the_break_that_terminates_an_li_is_dropped() {
        let html = "<div data-section-style='7' class=\"\" style=\"\"><ul id='SSfACAKV4zR'>\
                    <li id='SSfACA046uk' class='' style='' value='1'><span id='SSfACA046uk'>\
                    Prize did round night in kind porloremips read is any do.</span>\n\n\
                    <br/></li></ul></div>";
        assert_eq!(
            only_item_shape(html),
            "Prize did round night in kind porloremips read is any do.",
            "the `\\n\\n<br/>` before </li> is Quip's line terminator, not content"
        );
    }

    /// `QGYAAAjicgG`, first body row, verbatim: a row-number `<td>` with
    /// **no** terminator followed by two cells that have one. Both halves
    /// matter — the rule must not touch the cell that never had a break.
    #[test]
    fn the_break_that_terminates_a_td_is_dropped_and_a_cell_without_one_is_untouched() {
        let html = "<table id='temp:C:QGYfc8d4eb3ee71488795cd938e6' title='eager port' \
             style='width: 96em'><tbody>\
             <tr id='temp:C:QGYe66f22cd7b834833a7ee9dc58'>\
             <td style='background-color:#f0f0f0'>1</td>\
             <td id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d' \
             style=''><span id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGY4a3392935297410e89f835d1d'>\
             as</span>\n\n<br/></td>\
             <td id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGYf52ff49275694bfe880cf8613' \
             style=''><span id='temp:s:temp:C:QGYe66f22cd7b834833a7ee9dc58_temp:C:QGYf52ff49275694bfe880cf8613'>\
             no</span>\n\n<br/></td></tr></tbody></table>";
        let parsed = blocks(html);
        let [QuipBlock::Table { rows, .. }] = parsed.as_slice() else { panic!("expected a table") };
        let [row] = rows.as_slice() else { panic!("expected one row") };
        let shapes: Vec<String> = row
            .cells
            .iter()
            .map(|c| match c.blocks.as_slice() {
                [QuipBlock::Para { spans, .. }] => spans_shape(spans),
                other => panic!("expected one paragraph per cell, got {other:?}"),
            })
            .collect();
        assert_eq!(shapes, vec!["1", "as", "no"], "three cells, no break in any of them");
    }

    /// `QGYAAAjicgG`, a `<thead>` column header, verbatim. A `<th>` spells
    /// the terminator with no whitespace at all — `A<br/></th>` — so the
    /// scan cannot depend on the `\n\n` being there.
    #[test]
    fn the_break_that_terminates_a_th_is_dropped() {
        let html = "<table id='temp:C:QGYfc8d4eb3ee71488795cd938e6' title='eager port' \
             style='width: 96em'><thead><tr><th class='empty' style='width: 2em'/>\
             <th id='temp:C:QGY04be7f796bf1483e87f847ed3' class='empty' style='width: 6em'>A<br/>\
             </th></tr></thead></table>";
        let parsed = blocks(html);
        let [QuipBlock::Table { rows, .. }] = parsed.as_slice() else { panic!("expected a table") };
        let [row] = rows.as_slice() else { panic!("expected one row") };
        assert!(row.cells.iter().all(|c| c.header), "both cells are <th>");
        let shapes: Vec<String> = row
            .cells
            .iter()
            .map(|c| match c.blocks.as_slice() {
                [QuipBlock::Para { spans, .. }] => spans_shape(spans),
                other => panic!("expected one paragraph per cell, got {other:?}"),
            })
            .collect();
        assert_eq!(shapes, vec!["", "A"], "the self-closing <th> stays empty; 'A' keeps no break");
    }

    /// **The load-bearing one.** Same verbatim `SSfAAALs7fy` item as
    /// `the_break_that_terminates_an_li_is_dropped`, with one `<br/>`
    /// added in the middle of the span. That break is something a human
    /// typed and must survive; only the one before `</li>` goes.
    ///
    /// The corpus has no such item — every `<br>` outside a `<pre>` in all
    /// five fixtures is a terminator, which is why this shape has to be
    /// constructed. Constructed *from* a real item, not from scratch.
    #[test]
    fn a_mid_item_break_survives_while_the_terminator_goes() {
        let html = "<div data-section-style='7' class=\"\" style=\"\"><ul id='SSfACAKV4zR'>\
                    <li id='SSfACA046uk' class='' style='' value='1'><span id='SSfACA046uk'>\
                    Prize did round night<br/>in kind porloremips read is any do.</span>\n\n\
                    <br/></li></ul></div>";
        assert_eq!(
            only_item_shape(html),
            "Prize did round night⏎in kind porloremips read is any do.",
            "one break in, one break out: the mid-item break is authored content"
        );
    }

    /// Two breaks in a row at the end: the last is the terminator, the one
    /// before it is a blank line the author asked for. Exactly one goes.
    #[test]
    fn only_the_last_of_two_consecutive_trailing_breaks_is_dropped() {
        let html = "<div data-section-style='7' class=\"\" style=\"\"><ul id='SSfACAKV4zR'>\
                    <li id='SSfACA046uk' class='' style='' value='1'><span id='SSfACA046uk'>\
                    Prize did round night in kind porloremips read is any do.</span>\n\n\
                    <br/><br/></li></ul></div>";
        // The trailing space is the `\n\n` before the first break, which is
        // now *interior* to the run rather than at its end — `trim_spans`
        // only ever trimmed the outermost span, and a surviving break is
        // content that comes after it. Same shape any mid-content break
        // produces; recorded rather than tidied so a future change to that
        // trimming is visible here.
        assert_eq!(
            only_item_shape(html),
            "Prize did round night in kind porloremips read is any do. ⏎",
            "the inner break stays; a per-cell rule removes one terminator, not a run"
        );
    }

    /// The whitespace-node case, stated on its own because it is the shape
    /// the corpus actually has: `</span>\n\n<br/></li>`. A naive "is the
    /// final child a `<br>`?" reading of *idealized* markup would work here
    /// only by luck — the `\n\n` sits before the break — so this also pins
    /// the mirror case, whitespace *after* the break, which the scan skips.
    #[test]
    fn whitespace_text_nodes_around_the_terminator_do_not_hide_it() {
        let before = "<div data-section-style='7' class=\"\" style=\"\"><ul id='SSfACAKV4zR'>\
                      <li id='SSfACA046uk' class='' style='' value='1'><span id='SSfACA046uk'>\
                      Prize did round night in kind porloremips read is any do.</span>\n\n\
                      <br/></li></ul></div>";
        let after = "<div data-section-style='7' class=\"\" style=\"\"><ul id='SSfACAKV4zR'>\
                     <li id='SSfACA046uk' class='' style='' value='1'><span id='SSfACA046uk'>\
                     Prize did round night in kind porloremips read is any do.</span>\
                     <br/>\n\n</li></ul></div>";
        assert_eq!(only_item_shape(before), only_item_shape(after));
        assert_eq!(
            only_item_shape(after),
            "Prize did round night in kind porloremips read is any do.",
            "a trailing whitespace text node must not shield the terminator"
        );
    }

    /// An `<li>` whose entire content is the terminator. **Decision: the
    /// item survives as an empty one**, it is not dropped.
    ///
    /// Quip renders that markup as a bullet with nothing next to it — the
    /// author pressed Enter and left the line blank — so the bullet is the
    /// content and deleting it would silently renumber or shorten a list.
    /// `flatten_list` already gives a body-less item an `empty_para()`, so
    /// this falls out of the existing rule rather than needing one of its
    /// own; the test exists to pin the choice, not the mechanism.
    ///
    /// Constructed by emptying the `<span>` of the same verbatim item.
    #[test]
    fn an_li_holding_only_the_terminator_stays_an_empty_item() {
        let html = "<div data-section-style='7' class=\"\" style=\"\"><ul id='SSfACAKV4zR'>\
                    <li id='SSfACA046uk' class='' style='' value='1'><br/></li></ul></div>";
        let parsed = blocks(html);
        let [QuipBlock::List { items, .. }] = parsed.as_slice() else {
            panic!("expected one list, got {parsed:?}")
        };
        assert_eq!(items.len(), 1, "the empty bullet is still a bullet");
        assert_eq!(items[0].blocks, vec![empty_para()], "with an empty body, not a hard break");
    }

    /// `AeOAAAcV1hg`'s whole bullet section, verbatim — the shape that makes
    /// the *ordering* of the pass load-bearing, and the only mutation of
    /// this fix the corpus net caught on its own.
    ///
    /// `<li class='parent'>…<br/></li><ul>…</ul>` is how Quip nests. #187
    /// re-parents that sibling `<ul>` onto the end of the `<li>`, which puts
    /// a `<ul>` *after* the terminator; run the terminator scan afterwards
    /// and the break is no longer the item's last child, so it survives on
    /// every parent item in the corpus. Hence `strip_cell_terminators` runs
    /// before `normalize_quip_lists` — the question is asked of the markup
    /// Quip wrote, not of the tree a later pass reshaped.
    #[test]
    fn a_parent_item_that_owns_a_nested_list_still_loses_its_terminator() {
        let html = "<div data-section-style='5' class=\"\" style=\"\">\
             <ul id='temp:C:AeO6b3a4714314f44579cbb3cf0c'>\
             <li id='temp:C:AeObe85961cb2d4496ea374e229d' class='parent' style='' value='1'>\
             <span id='temp:C:AeObe85961cb2d4496ea374e229d'>Broad</span>\n\n<br/></li><ul>\
             <li id='temp:C:AeOff88d93bfbbd411981d8990df' class='' style=''>\
             <span id='temp:C:AeOff88d93bfbbd411981d8990df'>\u{200b}</span>\n\n<br/></li>\
             </ul></ul></div>";
        let parsed = blocks(html);
        let [QuipBlock::List { items, .. }] = parsed.as_slice() else {
            panic!("expected one list, got {parsed:?}")
        };
        let [outer] = items.as_slice() else { panic!("expected one outer item, got {items:?}") };
        let [QuipBlock::Para { spans, .. }, QuipBlock::List { items: inner, .. }] =
            outer.blocks.as_slice()
        else {
            panic!("expected text then a nested list, got {:?}", outer.blocks)
        };
        assert_eq!(spans_shape(spans), "Broad", "the parent item's terminator is gone");
        let [QuipBlock::Para { spans: inner_spans, .. }] = inner[0].blocks.as_slice() else {
            panic!("expected a nested item paragraph, got {:?}", inner[0].blocks)
        };
        assert_eq!(
            spans_shape(inner_spans),
            "\u{200b}",
            "and so is the nested item's — the U+200B spacer stays, the break goes"
        );
    }

    /// The #184 boundary, asserted from this side: a `<br>` inside a
    /// `<pre>` is a line separator and stays one, terminator rule or not.
    /// `CVLAAAgSl7Q`'s code-block markup, shortened but otherwise verbatim.
    #[test]
    fn the_cell_terminator_rule_does_not_reach_into_a_pre() {
        let html = "<pre id='temp:C:CVL09a9aeedb1db4947b4fabcc84' class='prettyprint'>\
                    #[component]<br>pub fn GameDetail() {<br>}</pre>";
        let parsed = blocks(html);
        let [QuipBlock::Code { text, .. }] = parsed.as_slice() else {
            panic!("expected a code block, got {parsed:?}")
        };
        assert_eq!(text, "#[component]\npub fn GameDetail() {\n}", "every <br> here is a newline");
    }

    /// And the same boundary with the `<pre>` inside a table cell, which is
    /// where a future "unify the two `<br>` rules" change would do its
    /// damage: the cell's last child is the `<pre>`, so there is no
    /// terminator to remove and the code block's own breaks are untouched.
    #[test]
    fn a_pre_at_the_end_of_a_cell_keeps_all_of_its_line_breaks() {
        let html = "<table><tbody><tr><td id='temp:s:temp:C:QGY4a3392935297410e89f835d1d' style=''>\
                    <pre id='temp:C:CVL09a9aeedb1db4947b4fabcc84' class='prettyprint'>\
                    a<br>b<br>c</pre></td></tr></tbody></table>";
        let blocks = blocks(html);
        let code: Vec<&String> = blocks
            .iter()
            .flat_map(|b| match b {
                QuipBlock::Table { rows, .. } => rows
                    .iter()
                    .flat_map(|r| r.cells.iter())
                    .flat_map(|c| c.blocks.iter())
                    .collect::<Vec<_>>(),
                other => vec![other],
            })
            .filter_map(|b| match b {
                QuipBlock::Code { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(code.len(), 1, "one code block, got {blocks:?}");
        assert_eq!(code[0].lines().count(), 3, "all three lines survive: {:?}", code[0]);
    }

    #[test]
    fn br_exports_as_a_real_br_tag_not_a_newline() {
        let out = from_quip_html("<p>a<br>b</p>");
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("<br"), "HardBreak must export as <br>: {html}");
        assert!(!html.contains("a\nb"), "no literal newline leaked into the exported text: {html}");
    }

    /// #169, pinned as a pure unit test: the *extracted* Quip HTML parses to
    /// clean block structure, whereas the *raw JSON envelope* — what the pre-fix
    /// client mistakenly returned — imports garbled, with the JSON scaffolding
    /// (`response_metadata`, `next_cursor`, the `{"html":"` wrapper) leaking in
    /// as visible document text. This is the exact failure mode the
    /// `thread_html` JSON-parse fix removes upstream.
    ///
    /// NOTE: the raw envelope uses the REAL Quip escaping — JSON escapes `"`
    /// (to `\"`) and `\`, but NOT `<`/`>`, so the tags stay literal. That means
    /// html5ever still sees *some* markup (so the result is a garbled multi-node
    /// mess, not the "single escaped text node" the #169 brief imprecisely
    /// described) — but it is unmistakably corrupt: the JSON wrapper is now part
    /// of the document body.
    #[test]
    fn extracted_html_yields_clean_blocks_but_the_raw_json_envelope_is_garbled() {
        let inner_html = "<h1>Creating a New Social Media</h1><p>body</p>";
        // Exactly how Quip serializes it: literal `<`/`>`, escaped `"`.
        let raw_envelope = concat!(
            r#"{"html":"<h1>Creating a New Social Media</h1><p>body</p>","#,
            r#""response_metadata":{"next_cursor":""}}"#,
        );

        // The extracted HTML → clean block structure, no JSON scaffolding.
        let good = from_quip_html(inner_html);
        let good_txn = good.doc.transact();
        let good_frag = crate::document::get_content_fragment(&good_txn).expect("fragment");
        assert_eq!(good_frag.len(&good_txn), 2, "heading + paragraph are two real blocks");
        let good_html = crate::export::to_html(&good.doc);
        assert!(
            !good_html.contains("response_metadata") && !good_html.contains("next_cursor"),
            "no JSON scaffolding may appear in a clean import: {good_html}",
        );

        // The raw JSON envelope → garbled: the wrapper leaked into the body.
        let bad = from_quip_html(raw_envelope);
        let bad_html = crate::export::to_html(&bad.doc);
        assert!(
            bad_html.contains("response_metadata") || bad_html.contains("next_cursor"),
            "the raw JSON envelope must leak its wrapper into document text — the #169 garbling: {bad_html}",
        );
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

    // ─── person mentions (`<control>`) ────────────────────────────
    //
    // The fixtures below are markup from real staged `/2` thread bodies: the
    // whole class of bug here comes from the person shape and the document
    // shape being indistinguishable once a wrapper is dropped, which a
    // hand-written fixture is exactly the wrong tool for noticing.
    // `REAL_FOLDER_LINK` is the one derivation — see its own comment.

    /// A person mention. The `<control>` wrapper is the entire signal.
    const REAL_PERSON_MENTION: &str = concat!(
        "Assign tasks by mentioning someone: ",
        r#"<control data-remapped="true" id="SSfACAGTvYT">"#,
        r#"<a href="https://quip.com/XYJAEA0Sgev">Joel</a></control>."#,
    );

    /// The same folder link **with its real `<control>` wrapper**, verbatim
    /// from `SSfAAALs7fy`. Indistinguishable from [`REAL_PERSON_MENTION`] in
    /// the markup — same tag, same attributes, same URL shape — which is why
    /// only the worker's `/1/users/` answer can tell them apart.
    const REAL_CONTROL_FOLDER_LINK: &str = concat!(
        "When you're done, check out your folder: ",
        r#"<control data-remapped="true" id="SSfACA1I4lV">"#,
        r#"<a href="https://quip.com/JAdAOAxYGcQ">Family</a></control>"#,
    );

    /// A **derived** fixture: the same link with the wrapper stripped, kept
    /// because a bare anchor must still take the document path.
    ///
    /// Provenance, corrected — this is NOT a transcription. The corpus
    /// contains **no** bare `quip.com` anchors at all; every one is
    /// `<control>`-wrapped (see [`REAL_CONTROL_FOLDER_LINK`] for the real
    /// markup, and [`super::walk_control`] for the measurement). An earlier
    /// version of this comment claimed the bare form was verbatim, and that
    /// claim was the sole evidence for "wrapped ⇒ person".
    const REAL_FOLDER_LINK: &str = concat!(
        "When you're done, check out your folder: ",
        r#"<a href="https://quip.com/JAdAOAxYGcQ">Family</a>"#,
    );

    /// A `<control>`-wrapped link to a *section of a thread*, verbatim from
    /// the staged `KdFAAAxgYHm` body. The `#temp:C:…` fragment is what makes
    /// it decidably a document rather than a person.
    const REAL_CONTROL_SECTION_LINK: &str = concat!(
        r#"<control data-remapped="true" id="temp:C:KdF6a74070b4ab84f488b2ddcf7d">"#,
        r#"<a href="https://quip.com/81aBAkO87SsN#temp:C:KdF95ad5eac998f4086952fea453">"#,
        r#"Quip is being retired.</a></control>"#,
    );

    /// A Quip date: client-rendered, so the export carries an empty control.
    const REAL_EMPTY_CONTROL: &str =
        r#"Complete by <control data-remapped="true" id="SSfACAsTxeJ"></control>."#;

    /// A resolution map holding only matches — the common test shape.
    fn matched<const N: usize>(pairs: [(&str, &str); N]) -> HashMap<String, PersonOutcome> {
        pairs
            .into_iter()
            .map(|(quip, ogre)| (quip.to_string(), PersonOutcome::User(ogre.to_string())))
            .collect()
    }

    /// Every `Mention` element in `doc`, as `(user_id, display, pending)`.
    fn mention_leaves(doc: &Doc) -> Vec<(String, String, Option<String>)> {
        let txn = doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        let mut out = Vec::new();
        fn walk<T: ReadTxn>(txn: &T, el: &XmlElementRef, out: &mut Vec<(String, String, Option<String>)>) {
            for i in 0..el.len(txn) {
                let Some(XmlOut::Element(child)) = el.get(txn, i) else { continue };
                if child.tag().as_ref() == NodeType::Mention.tag_name() {
                    out.push((
                        child.get_attribute(txn, "user_id").unwrap_or_default(),
                        child.get_attribute(txn, "display").unwrap_or_default(),
                        child.get_attribute(txn, PENDING_QUIP_USER_ATTR),
                    ));
                }
                walk(txn, &child, out);
            }
        }
        for i in 0..frag.len(&txn) {
            if let Some(XmlOut::Element(el)) = frag.get(&txn, i) {
                walk(&txn, &el, &mut out);
            }
        }
        out
    }

    /// The bug this feature fixes, pinned at the walker: a `<control>`-wrapped
    /// anchor is a **person**, and must not become an intra-Quip document
    /// link (which renders as a "Missing document" chip).
    ///
    /// Mutation check: drop `"control"` from `allowed_tags()` and ammonia
    /// strips the wrapper while keeping its child, so this document parses
    /// byte-identically to `REAL_FOLDER_LINK` — `person_mentions` empties and
    /// `pending_links` gains an entry. Both assertions below go red.
    #[test]
    fn a_control_wrapped_anchor_is_a_person_mention_not_a_document_link() {
        let out = from_quip_html(REAL_PERSON_MENTION);

        assert_eq!(out.person_mentions.len(), 1, "one person mention");
        assert_eq!(out.person_mentions[0].quip_user_id, "XYJAEA0Sgev");
        assert_eq!(out.person_mentions[0].label, "Joel");
        assert!(
            out.pending_links.is_empty(),
            "a person is not a document: no pending doc link may be recorded — {:?}",
            out.pending_links.iter().map(|l| &l.target_quip_thread_id).collect::<Vec<_>>(),
        );

        // The leaf is a real `Mention`, still awaiting an identity.
        assert_eq!(
            mention_leaves(&out.doc),
            vec![(String::new(), "Joel".to_string(), Some("XYJAEA0Sgev".to_string()))],
        );
        assert_valid_tree(&out.doc);

        // The surrounding prose is untouched, and the chip did not swallow
        // the sentence-ending period that follows it.
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("Assign tasks by mentioning someone:"), "{html}");
        assert!(!html.contains("doc-mention"), "no DocMention may be emitted: {html}");
    }

    /// The contrast case, from the same source document: a **bare** anchor at
    /// an identically-shaped Quip URL keeps its existing document-link
    /// handling. Nothing about this test may change.
    #[test]
    fn a_bare_quip_anchor_is_still_a_document_link_not_a_person() {
        let out = from_quip_html(REAL_FOLDER_LINK);

        assert!(out.person_mentions.is_empty(), "a bare anchor is not a person mention");
        assert_eq!(out.pending_links.len(), 1);
        assert_eq!(out.pending_links[0].target_quip_thread_id, "JAdAOAxYGcQ");
        assert!(mention_leaves(&out.doc).is_empty(), "no person chip");

        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("doc-mention"), "the DocMention placeholder survives: {html}");
    }

    /// An empty `<control>` (a Quip date — the client renders it, so the
    /// export carries nothing) contributes nothing and must not disturb the
    /// text around it. `Complete by .` is what Quip itself hands us.
    #[test]
    fn an_empty_control_contributes_nothing_and_leaves_the_text_intact() {
        let out = from_quip_html(REAL_EMPTY_CONTROL);

        assert!(out.person_mentions.is_empty());
        assert!(out.pending_links.is_empty());
        assert!(mention_leaves(&out.doc).is_empty());

        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("Complete by ."), "surrounding text is intact: {html}");
    }

    /// All three shapes in one body — the way they actually arrive.
    #[test]
    fn the_three_control_shapes_coexist_in_one_document() {
        let html = format!("<p>{REAL_PERSON_MENTION}</p><p>{REAL_FOLDER_LINK}</p><p>{REAL_EMPTY_CONTROL}</p>");
        let out = from_quip_html(&html);
        assert_eq!(out.person_mentions.len(), 1, "only the wrapped anchor is a person");
        assert_eq!(out.pending_links.len(), 1, "only the bare anchor is a doc link");
        assert_valid_tree(&out.doc);
    }

    /// A matched person becomes a real OgreNotes mention: the pending Quip
    /// id is gone and the OgreNotes user id is the only identity left on the
    /// node. Nothing derived from the lookup (an email, above all) is stored.
    #[test]
    fn resolve_person_mentions_finishes_a_matched_chip() {
        let out = from_quip_html(REAL_PERSON_MENTION);
        let resolved = matched([("XYJAEA0Sgev", "ogre-user-7")]);
        assert_eq!(resolve_person_mentions(&out.doc, &resolved).degraded, 0, "nothing degraded");

        assert_eq!(
            mention_leaves(&out.doc),
            vec![("ogre-user-7".to_string(), "Joel".to_string(), None)],
            "user_id filled in, pending Quip id removed",
        );
        assert_valid_tree(&out.doc);

        let exported = crate::export::to_html(&out.doc);
        assert!(exported.contains("data-user-id=\"ogre-user-7\""), "{exported}");
        assert!(!exported.contains("XYJAEA0Sgev"), "the Quip id must not survive: {exported}");

        // At BYTE level, not just in the export. `render_html_attrs` is a
        // per-node-type allowlist, so an attribute the schema does not know
        // is invisible to every HTML-level assertion above while still being
        // written into the snapshot — and both pending attributes carry the
        // Quip person id. These are the assertions that make deleting either
        // `remove_attribute` on the matched path go red.
        //
        // Asserted on the attribute **values**, not the key names. A removed
        // attribute's *content* is garbage-collected out of the encoded
        // update, but its key string stays interned in the update's key
        // table for as long as the element lives — so `pending_quip_user`
        // itself is still greppable here, and always will be. That name is a
        // compile-time constant carrying nothing about anyone; the id and the
        // url are the payload, and they are what must not survive.
        let bytes = crate::snapshot::doc_to_bytes(&out.doc);
        let raw = String::from_utf8_lossy(&bytes);
        assert!(
            !raw.contains("XYJAEA0Sgev"),
            "the Quip person id must not reach the snapshot — it rides on BOTH \
             pending attributes, so either removal going missing shows up here",
        );
        assert!(
            !raw.contains("quip.com"),
            "the Quip url must not reach the snapshot (pending_quip_url not removed?)",
        );
    }

    /// No matching OgreNotes account: the chip degrades to the person's
    /// **name** as plain text — never a mention of nobody, and never a
    /// "Missing document" chip.
    #[test]
    fn resolve_person_mentions_degrades_an_unmatched_chip_to_the_persons_name() {
        let out = from_quip_html(REAL_PERSON_MENTION);
        assert_eq!(
            resolve_person_mentions(&out.doc, &HashMap::new()).degraded,
            1,
            "one degraded",
        );

        assert!(mention_leaves(&out.doc).is_empty(), "no empty-user_id chip may remain");
        assert_valid_tree(&out.doc);

        let html = crate::export::to_html(&out.doc);
        // Pinned exactly: the chip is gone, the sentence reads as prose, and
        // the `@` marks it as a person reference rather than a stray word.
        assert!(
            html.contains("Assign tasks by mentioning someone: @Joel."),
            "the degraded mention must read as ordinary prose: {html}",
        );
        assert!(!html.contains("doc-mention"), "never a missing-document chip: {html}");
        assert!(!html.contains("XYJAEA0Sgev"), "{html}");
    }

    /// `resolve_person_mentions` is total over the document, not over a
    /// caller-supplied list: an id the caller never asked about still gets
    /// decided, and unrelated keys in the map are inert.
    #[test]
    fn resolve_person_mentions_decides_every_chip_it_finds() {
        let out = from_quip_html(&format!(
            "<p>{REAL_PERSON_MENTION}</p><p>a <control><a href=\"https://quip.com/OTHERID\">Bea</a></control> b</p>",
        ));
        assert_eq!(out.person_mentions.len(), 2);
        let resolved = matched([("OTHERID", "ogre-bea"), ("NOBODY", "ogre-ghost")]);
        assert_eq!(
            resolve_person_mentions(&out.doc, &resolved).degraded,
            1,
            "Joel degrades, Bea resolves",
        );
        assert_eq!(
            mention_leaves(&out.doc),
            vec![("ogre-bea".to_string(), "Bea".to_string(), None)],
        );
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("@Joel") && html.contains("a ") && html.contains(" b"), "{html}");
    }

    /// A `<control>`-wrapped chip that Quip does not know as a person is a
    /// **document link**, not plain text.
    ///
    /// This is the majority case in the staged corpus (a folder chip and a
    /// thread chip against one real person), and degrading it to `@Family`
    /// text would destroy a back-patchable link — a regression against the
    /// pre-feature behavior, where the wrapper was simply stripped and
    /// `walk_anchor` produced a `DocMention`.
    ///
    /// The node must match what `walk_anchor` would have built, and the
    /// caller must be handed the pending-link record, or Phase 2b has
    /// nothing to back-patch.
    ///
    /// Mutation check: route `NotAPerson` to the plain-text branch (or map it
    /// to `NoAccount` in the worker) and every assertion below goes red — the
    /// `DocMention` disappears, `doc_links` empties, and the export shows
    /// `@Family`.
    #[test]
    fn a_control_wrapped_non_person_becomes_a_back_patchable_document_link() {
        let out = from_quip_html(REAL_CONTROL_FOLDER_LINK);
        // The walker cannot tell: it emits a provisional person mention.
        assert_eq!(out.person_mentions.len(), 1, "provisional, decided by the worker");
        assert!(out.pending_links.is_empty(), "the walker records nothing yet");

        let resolved =
            HashMap::from([("JAdAOAxYGcQ".to_string(), PersonOutcome::NotAPerson)]);
        let rewritten = resolve_person_mentions(&out.doc, &resolved);
        assert_eq!(rewritten.degraded, 0, "a document link is not a degraded mention");

        // The back-patch record Phase 2b needs.
        assert_eq!(rewritten.doc_links.len(), 1, "the pending link is handed to the caller");
        assert_eq!(rewritten.doc_links[0].target_quip_thread_id, "JAdAOAxYGcQ");
        assert_eq!(rewritten.doc_links[0].target_quip_section_id, None);

        assert!(mention_leaves(&out.doc).is_empty(), "no person chip remains");
        assert_valid_tree(&out.doc);

        // The node itself, and that it points at its own pending link.
        let txn = out.doc.transact();
        let frag = crate::document::get_content_fragment(&txn).expect("content fragment");
        let Some(XmlOut::Element(para)) = frag.get(&txn, 0) else { panic!("expected an element") };
        let mut found = false;
        for i in 0..para.len(&txn) {
            let Some(XmlOut::Element(el)) = para.get(&txn, i) else { continue };
            assert_eq!(NodeType::from_tag(el.tag().as_ref()), Some(NodeType::DocMention));
            found = true;
            assert_eq!(el.get_attribute(&txn, "doc_id").as_deref(), Some(""), "unresolved");
            assert_eq!(el.get_attribute(&txn, "title").as_deref(), Some("Family"));
            assert_eq!(
                el.get_attribute(&txn, "pending_quip_thread").as_deref(),
                Some("JAdAOAxYGcQ"),
            );
            assert_eq!(
                el.get_attribute(&txn, "url").as_deref(),
                Some("https://quip.com/JAdAOAxYGcQ"),
                "the same url `walk_anchor` would have stored",
            );
            assert_eq!(
                el.get_attribute(&txn, "blockId").as_deref(),
                Some(rewritten.doc_links[0].source_block_id.as_str()),
                "the placeholder's own blockId is the pending link's source",
            );
            assert!(el.get_attribute(&txn, PENDING_QUIP_USER_ATTR).is_none());
            assert!(el.get_attribute(&txn, PENDING_QUIP_URL_ATTR).is_none());
        }
        assert!(found, "a DocMention placeholder was emitted");
        drop(txn);

        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("doc-mention"), "the back-patchable chip survives: {html}");
        assert!(!html.contains("@Family"), "it must not degrade to text: {html}");
    }

    /// The contrast, on **identical** markup: a chip Quip confirms IS a
    /// person but that matches no OgreNotes account stays plain text.
    ///
    /// Rendering a real colleague as a missing *document* is the bug this
    /// whole feature exists to fix, so `NoAccount` and `NotAPerson` must
    /// never collapse into one another.
    #[test]
    fn a_person_with_no_ogrenotes_account_stays_plain_text_not_a_document_link() {
        let out = from_quip_html(REAL_PERSON_MENTION);
        let resolved = HashMap::from([("XYJAEA0Sgev".to_string(), PersonOutcome::NoAccount)]);
        let rewritten = resolve_person_mentions(&out.doc, &resolved);

        assert_eq!(rewritten.degraded, 1, "degraded to a name");
        assert!(rewritten.doc_links.is_empty(), "a person is never a document link");
        assert!(mention_leaves(&out.doc).is_empty(), "no empty-user_id chip may remain");
        assert_valid_tree(&out.doc);

        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("@Joel"), "the name survives as prose: {html}");
        assert!(!html.contains("doc-mention"), "never a missing-document chip: {html}");
        assert!(!html.contains("XYJAEA0Sgev"), "{html}");
    }

    /// An **empty-label** person mention (an avatar-only or deleted-user chip;
    /// `trim_spans` deliberately retains a person span with no text) must not
    /// disturb its siblings.
    ///
    /// This is the regression test for the index-shift corruption: the degrade
    /// branch removes the node and, with no label to show, inserts nothing —
    /// so an index-addressed rewrite loop leaves every later mention in the
    /// same parent off by one.
    ///
    /// Mutation check: restore the index-based rewrite (`PendingPerson.index`
    /// + `found.parent.get(&txn, found.index)`) and this test goes red on
    /// every assertion below — the trailing `" C"` is deleted and replaced by
    /// a duplicate `@Bob`, and a `<mention user_id="" pending_quip_user="…">`
    /// survives into the document.
    #[test]
    fn an_empty_label_person_mention_does_not_shift_its_siblings() {
        const BODY: &str = concat!(
            r#"<p>A <control><a href="https://quip.com/EMPTYONE"></a></control>"#,
            r#" B <control><a href="https://quip.com/SECOND">Bob</a></control> C</p>"#,
        );

        // Both unmatched.
        let out = from_quip_html(BODY);
        assert_eq!(out.person_mentions.len(), 2, "both chips are collected");
        assert_eq!(out.person_mentions[0].label, "", "the empty label is retained");
        assert_eq!(
            resolve_person_mentions(&out.doc, &HashMap::new()).degraded,
            2,
            "both decided",
        );
        assert!(mention_leaves(&out.doc).is_empty(), "no empty-user_id chip may remain");
        assert_valid_tree(&out.doc);

        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("@Bob"), "the labelled chip degrades to its name: {html}");
        assert_eq!(html.matches("@Bob").count(), 1, "and exactly once: {html}");
        assert!(html.contains(" C"), "adjacent text must not be destroyed: {html}");
        assert!(!html.contains("EMPTYONE") && !html.contains("SECOND"), "{html}");
        let bytes = crate::snapshot::doc_to_bytes(&out.doc);
        let raw = String::from_utf8_lossy(&bytes);
        assert!(
            !raw.contains("EMPTYONE") && !raw.contains("SECOND") && !raw.contains("quip.com"),
            "no Quip identifier may reach the snapshot",
        );
        // Here the whole element is deleted, so even the interned attribute
        // key goes with it — unlike the matched branch, where the element
        // survives and its removed keys stay in the update's key table. See
        // `resolve_person_mentions_finishes_a_matched_chip`.
        assert!(
            !raw.contains(PENDING_QUIP_USER_ATTR) && !raw.contains(PENDING_QUIP_URL_ATTR),
            "no transient attribute may reach the snapshot",
        );

        // The later chip *matched*: it must get the real user id, not have it
        // written onto whatever sat at a stale index.
        let out = from_quip_html(BODY);
        let resolved = matched([("SECOND", "ogre-bob")]);
        assert_eq!(
            resolve_person_mentions(&out.doc, &resolved).degraded,
            1,
            "only the empty one degrades",
        );
        assert_eq!(
            mention_leaves(&out.doc),
            vec![("ogre-bob".to_string(), "Bob".to_string(), None)],
            "the matched chip carries the OgreNotes id and no Quip id",
        );
        let html = crate::export::to_html(&out.doc);
        assert!(html.contains(" C"), "adjacent text must not be destroyed: {html}");
        assert!(!html.contains("SECOND"), "the Quip id must not survive: {html}");
    }

    /// A `<control>`-wrapped anchor carrying a **section fragment** is a
    /// document link, not a person: sections belong to threads, and a person
    /// has none. This is the one discriminator the staged corpus supplies.
    ///
    /// Mutation check: drop the fragment guard in `quip_person_from_url`
    /// and this becomes a person mention — `person_mentions` gains an entry
    /// and `pending_links` empties.
    #[test]
    fn a_control_wrapped_section_link_is_a_document_not_a_person() {
        let out = from_quip_html(REAL_CONTROL_SECTION_LINK);

        assert!(
            out.person_mentions.is_empty(),
            "a section anchor is a document link however it is wrapped: {:?}",
            out.person_mentions.iter().map(|m| &m.quip_user_id).collect::<Vec<_>>(),
        );
        assert_eq!(out.pending_links.len(), 1, "the back-patchable link survives");
        assert_eq!(out.pending_links[0].target_quip_thread_id, "81aBAkO87SsN");
        assert_eq!(
            out.pending_links[0].target_quip_section_id.as_deref(),
            Some("temp:C:KdF95ad5eac998f4086952fea453"),
        );
        assert!(mention_leaves(&out.doc).is_empty(), "no person chip");

        let html = crate::export::to_html(&out.doc);
        assert!(html.contains("doc-mention"), "the DocMention placeholder survives: {html}");
    }

    /// A person mention inside a table cell / list item is reached too — the
    /// rewrite walks the whole tree, not just top-level paragraphs.
    #[test]
    fn resolve_person_mentions_reaches_nested_chips() {
        let out = from_quip_html(&format!(
            "<table><tr><td><p>{REAL_PERSON_MENTION}</p></td></tr></table>",
        ));
        assert_eq!(out.person_mentions.len(), 1, "the nested mention is collected");
        assert_eq!(
            resolve_person_mentions(&out.doc, &HashMap::new()).degraded,
            1,
            "and it is decided",
        );
        assert!(mention_leaves(&out.doc).is_empty());
    }

    /// A `<control>` that wraps something other than a Quip anchor is
    /// transparent: its text survives rather than being dropped, matching
    /// how every other unknown wrapper in this walker behaves.
    #[test]
    fn a_control_without_a_quip_anchor_is_transparent() {
        for html in [
            "<p>x <control>plain text</control> y</p>",
            "<p>x <control><a href=\"https://elsewhere.example/p\">ext</a></control> y</p>",
        ] {
            let out = from_quip_html(html);
            assert!(out.person_mentions.is_empty(), "{html}");
            let exported = crate::export::to_html(&out.doc);
            assert!(exported.contains('x') && exported.contains('y'), "{html} -> {exported}");
        }
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
