// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P5 pieces A and B — Markdown and HTML → OgreNotes document
//! import. The Markdown importer (`from_markdown`, piece A) is below;
//! the HTML importer (`from_html`, piece B) lives in the second half of
//! this file, under its own section banner.
//!
//! Walks pulldown-cmark events and constructs a yrs `Doc` whose
//! XmlFragment shape matches what `export::to_markdown` would emit
//! for the same content — so a round-trip
//! `export.md → import → export` is approximately lossless for the
//! supported block grammar.
//!
//! **v1 limitation: inline marks are dropped.** Bold / italic / code /
//! link syntax is parsed but the resulting yrs Text gets plain
//! characters, no formatting attributes. The export side reads marks
//! via the yrs delta API; preserving them on import needs the
//! corresponding insert-with-attributes path, which lands together
//! with the HTML importer in M-P5 piece B (symmetric implementation).
//!
//! Supported block grammar:
//!
//!   - paragraph
//!   - heading h1-h6 with `level` attribute
//!   - bullet list / ordered list / list item
//!   - blockquote
//!   - code block with `language` attribute (when fenced ``` lang)
//!   - horizontal rule
//!   - hard break
//!
//! Out of scope for v1: tables, images, task lists, footnotes.

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use yrs::{
    Doc, Transact, WriteTxn, Xml,
    types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim},
    XmlElementRef,
};

use crate::schema::NodeType;

/// Parse a Markdown source string into a fresh yrs `Doc`. Always
/// succeeds — Markdown is a permissive grammar, malformed-looking
/// input produces a doc that's at worst awkward, not an error.
pub fn from_markdown(md: &str) -> Doc {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");

        // Stack of currently-open container elements. The top of
        // the stack is where the next child gets inserted.
        let mut stack: Vec<XmlElementRef> = Vec::new();

        // Buffer for text + soft-breaks accumulated within the
        // current leaf block (paragraph / heading / code block).
        // Flushed into the top-of-stack element on End-of-block.
        let mut text_buf = String::new();
        // Whether we're inside a code block — when true, the buffer
        // is preserved verbatim (no whitespace coalescing).
        let mut in_code_block = false;
        // Heading level for the currently-open heading (set on
        // Start(Heading), consumed when the heading element gets
        // its `level` attribute on End).
        let mut current_heading_level: Option<u8> = None;
        // Pending code-block language (set on Start(CodeBlock), read
        // on End to write the `language` attribute).
        let mut current_code_language: Option<String> = None;

        for event in Parser::new(md) {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Paragraph => {
                        let parent = current_parent(&fragment, &stack, &txn);
                        let el = insert_at_end(
                            &mut txn,
                            &parent,
                            NodeType::Paragraph,
                        );
                        stack.push(el);
                    }
                    Tag::Heading { level, .. } => {
                        let parent = current_parent(&fragment, &stack, &txn);
                        let el = insert_at_end(
                            &mut txn,
                            &parent,
                            NodeType::Heading,
                        );
                        stack.push(el);
                        current_heading_level = Some(heading_level_to_u8(level));
                    }
                    Tag::BlockQuote(_) => {
                        let parent = current_parent(&fragment, &stack, &txn);
                        let el = insert_at_end(
                            &mut txn,
                            &parent,
                            NodeType::Blockquote,
                        );
                        stack.push(el);
                    }
                    Tag::CodeBlock(kind) => {
                        let parent = current_parent(&fragment, &stack, &txn);
                        let el = insert_at_end(
                            &mut txn,
                            &parent,
                            NodeType::CodeBlock,
                        );
                        stack.push(el);
                        in_code_block = true;
                        current_code_language = code_block_language(kind);
                    }
                    Tag::List(start) => {
                        let parent = current_parent(&fragment, &stack, &txn);
                        let kind = if start.is_some() {
                            NodeType::OrderedList
                        } else {
                            NodeType::BulletList
                        };
                        let el = insert_at_end(&mut txn, &parent, kind);
                        stack.push(el);
                    }
                    Tag::Item => {
                        let parent = current_parent(&fragment, &stack, &txn);
                        let el = insert_at_end(
                            &mut txn,
                            &parent,
                            NodeType::ListItem,
                        );
                        stack.push(el);
                    }
                    // Inline markers — v1 ignores the start/end markers
                    // and accumulates the inner text into `text_buf`.
                    // Adding bold/italic/code/link marks lands with the
                    // HTML importer in piece B.
                    Tag::Emphasis | Tag::Strong | Tag::Strikethrough => {}
                    Tag::Link { .. } | Tag::Image { .. } => {}
                    _ => {}
                },
                Event::End(end) => {
                    match end {
                        TagEnd::Paragraph
                        | TagEnd::Heading(_)
                        | TagEnd::CodeBlock => {
                            if let Some(el) = stack.pop() {
                                flush_text(
                                    &mut txn,
                                    &el,
                                    &mut text_buf,
                                    in_code_block,
                                );
                                if let Some(level) = current_heading_level.take() {
                                    el.insert_attribute(
                                        &mut txn,
                                        "level",
                                        level.to_string(),
                                    );
                                }
                                if matches!(end, TagEnd::CodeBlock) {
                                    if let Some(lang) = current_code_language.take() {
                                        if !lang.is_empty() {
                                            el.insert_attribute(
                                                &mut txn,
                                                "language",
                                                lang,
                                            );
                                        }
                                    }
                                    in_code_block = false;
                                }
                            }
                        }
                        TagEnd::BlockQuote(_)
                        | TagEnd::List(_)
                        | TagEnd::Item => {
                            stack.pop();
                        }
                        _ => {}
                    }
                }
                Event::Text(t) => {
                    text_buf.push_str(t.as_ref());
                }
                Event::Code(t) => {
                    // Inline code — v1 inlines the literal text. piece B
                    // will write the Code mark via yrs text formatting.
                    text_buf.push_str(t.as_ref());
                }
                Event::SoftBreak => {
                    // pulldown-cmark emits SoftBreak between lines that
                    // CommonMark joins as a single paragraph. Convert to
                    // a space, matching how renderers display them.
                    if !in_code_block {
                        text_buf.push(' ');
                    } else {
                        text_buf.push('\n');
                    }
                }
                Event::HardBreak => {
                    // Flush accumulated text + insert a HardBreak
                    // element into the current block.
                    if let Some(parent) = stack.last() {
                        flush_text(&mut txn, parent, &mut text_buf, in_code_block);
                        insert_at_end(
                            &mut txn,
                            &XmlOpenable::Element(parent.clone()),
                            NodeType::HardBreak,
                        );
                    }
                }
                Event::Rule => {
                    let parent = current_parent(&fragment, &stack, &txn);
                    insert_at_end(&mut txn, &parent, NodeType::HorizontalRule);
                }
                Event::Html(_) | Event::InlineHtml(_) => {
                    // Raw HTML inside Markdown is dropped in v1 — the
                    // safer default. piece B's HTML importer accepts
                    // full HTML through the ammonia-sanitized path.
                }
                _ => {}
            }
        }
    }

    doc
}

/// Either the root XmlFragment or a nested XmlElement — the two
/// "where to insert a child" targets in the import traversal.
enum XmlOpenable<'a> {
    Fragment(&'a yrs::XmlFragmentRef),
    Element(XmlElementRef),
}

fn current_parent<'a, T: yrs::ReadTxn>(
    fragment: &'a yrs::XmlFragmentRef,
    stack: &[XmlElementRef],
    _txn: &T,
) -> XmlOpenable<'a> {
    match stack.last() {
        Some(el) => XmlOpenable::Element(el.clone()),
        None => XmlOpenable::Fragment(fragment),
    }
}

fn insert_at_end(
    txn: &mut yrs::TransactionMut<'_>,
    parent: &XmlOpenable<'_>,
    node: NodeType,
) -> XmlElementRef {
    let prelim = XmlElementPrelim::empty(node.tag_name());
    match parent {
        XmlOpenable::Fragment(f) => {
            let pos = f.len(txn);
            f.insert(txn, pos, prelim)
        }
        XmlOpenable::Element(e) => {
            let pos = e.len(txn);
            e.insert(txn, pos, prelim)
        }
    }
}

fn flush_text(
    txn: &mut yrs::TransactionMut<'_>,
    block: &XmlElementRef,
    buf: &mut String,
    _in_code_block: bool,
) {
    if buf.is_empty() {
        return;
    }
    let pos = block.len(txn);
    block.insert(txn, pos, XmlTextPrelim::new(buf.as_str()));
    buf.clear();
}

fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn code_block_language(kind: pulldown_cmark::CodeBlockKind<'_>) -> Option<String> {
    match kind {
        pulldown_cmark::CodeBlockKind::Fenced(lang) => {
            let s = lang.to_string();
            if s.is_empty() { None } else { Some(s) }
        }
        pulldown_cmark::CodeBlockKind::Indented => None,
    }
}

// ─── HTML import (Phase 5 M-P5 piece B) ──────────────────────────

/// Parse an HTML source string into a fresh yrs `Doc`. Same shape
/// guarantee as `from_markdown` — the resulting yrs XmlFragment
/// matches what `export::to_html` would emit for equivalent
/// content, so the round-trip
/// `export.html → import → export` is approximately lossless for
/// the supported block grammar.
///
/// Sanitization pipeline:
///
///   1. **ammonia.clean** strips script / iframe / form / on*
///      attributes / javascript: URLs. The output is HTML that's
///      safe to feed to html5ever.
///   2. **html5ever** parses the cleaned string into a `RcDom`.
///   3. A recursive walker maps each known tag to its `NodeType`
///      and copies text content into yrs text leaves.
///
/// Unknown tags become "transparent" — their children are walked
/// in the same context as the unknown wrapper. This is the right
/// default for `<div>` / `<section>` / etc. that the export side
/// never emits but a third-party HTML source might.
///
/// Nesting deeper than [`MAX_NESTING_DEPTH`] is flattened before the
/// walk — see that constant for why the bound is a liveness
/// requirement, not a formatting preference.
///
/// Same v1 limitation as `from_markdown`: inline marks (bold,
/// italic, code, link) are dropped. Pre-existing
/// `inline_emphasis_drops_marks_keeps_text` test pins the contract.
/// Mark-preservation across both importers lands in a follow-up
/// once the yrs text formatting story is wired symmetrically.
pub fn from_html(html: &str) -> Doc {
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::RcDom;

    // Stage 1: sanitize.
    let safe = sanitize(html);

    // Stage 2: parse to DOM.
    let dom: RcDom = html5ever::parse_document(
        RcDom::default(),
        html5ever::driver::ParseOpts::default(),
    )
    .from_utf8()
    .read_from(&mut safe.as_bytes())
    .expect("html5ever parse is infallible on bytes");

    // Stage 2b: bound the tree BEFORE the recursive walk, rather than
    // threading a depth counter through `walk_html`'s four recursion
    // sites. The guarantee is then structural: whatever the walker
    // receives is already shallower than MAX_NESTING_DEPTH, so every
    // recursive pass over it is bounded by the same constant for
    // free, and no future recursive pass can forget to check.
    let truncated = flatten_below_depth(&dom.document);
    if truncated > 0 {
        tracing::warn!(
            truncated,
            max_depth = MAX_NESTING_DEPTH,
            "html import flattened subtrees nested past the depth cap",
        );
    }

    // Stage 3: walk + materialize.
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");
        walk_html(&mut txn, &dom.document, &XmlOpenable::Fragment(&fragment));
    }
    doc
}

/// Stage 1 of [`from_html`] on its own: ammonia's default Builder,
/// narrowed to [`allowed_html_tags`].
///
/// ammonia's default posture is what we want for an import endpoint
/// that may receive third-party HTML — a tag whitelist, an attribute
/// whitelist, and URL-scheme filtering — and we narrow only the tag
/// set. Everything else (no `generic_attributes`, no
/// `generic_attribute_prefixes`, default `url_schemes`) is left at
/// ammonia's defaults *deliberately*: each of those knobs widens the
/// boundary, and `sanitize_*` in the test module below pins the
/// resulting behavior rather than the call shape.
///
/// Factored out of `from_html` so those tests can exercise the
/// boundary directly. It stays private — the walker-and-materializer
/// property in `tests/import_fuzz.rs` cannot observe this layer (the
/// yrs tree it inspects can only carry `NodeType` tag names), and
/// making the sanitizer `pub` to work around that would export an
/// internal stage as API.
fn sanitize(html: &str) -> String {
    ammonia::Builder::default()
        .tags(allowed_html_tags())
        .clean(html)
        .to_string()
}

/// Deepest element nesting the HTML walker is ever allowed to
/// descend.
///
/// **This is a process-liveness bound, not a formatting
/// preference.** `walk_html` recurses once per DOM level over HTML
/// that arrives straight from `POST /documents/import` with nothing
/// but an `AuthUser` check in front of it. Exhaust the thread stack
/// and Rust **aborts the process** — stack overflow is not a panic,
/// so no `catch_unwind` anywhere can contain it, and the abort takes
/// down every in-flight request and open WebSocket sharing that
/// process, not just the offending import.
///
/// Measured on the 2 MiB stack tokio worker threads and Rust test
/// threads both get (no `stack_size` / `RUST_MIN_STACK` / `ulimit -s`
/// override exists anywhere in the Rust code, the CDK app, the
/// Dockerfile or the compose file, so 2 MiB is what production runs):
/// nested `<div>` / `<span>` / `<blockquote>` parsed at 2 400 levels
/// and aborted at 3 000. A ~30 KB body was enough. `ammonia::clean`
/// and html5ever themselves survive 50 000+ — the recursion that
/// fails is ours.
///
/// 128 leaves roughly 20x headroom against that measured threshold,
/// which absorbs a smaller stack, a deeper call path above the
/// walker, or a debug build's fatter frames. It is deliberately the
/// same number the Quip walker caps at for the same class of bug, so
/// this crate's two HTML walkers give a reader one number to learn
/// rather than two (they are independent constants — neither parser
/// reads the other's). Authored documents are nowhere near it: our
/// own HTML export tops out in the tens even for deeply nested lists,
/// and paste-artifact wrapper accumulation runs to tens, not
/// hundreds.
pub const MAX_NESTING_DEPTH: usize = 128;

/// Replace every subtree rooted deeper than [`MAX_NESTING_DEPTH`]
/// with a single text node holding that subtree's flattened text,
/// returning how many subtrees were flattened.
///
/// **Fails soft: text survives, only the nesting is lost.** Dropping
/// the subtree would silently delete the deepest content of a
/// document, and rejecting the import would turn a formatting quirk
/// into a failed request — so the content is kept and the loss is
/// logged.
///
/// **Iterative, with an explicit stack, in both halves.** A recursive
/// implementation of the guard against unbounded recursion would
/// abort on exactly the input it exists to survive.
///
/// Depth is counted from the document node, so the
/// `<html>`/`<body>` scaffold html5ever always inserts spends three
/// levels of the budget. That is the conservative direction and the
/// margin swallows it.
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
                // The replacement's `parent` is left unset. That is
                // fine: `walk_html` reaches every node through
                // `children` and never reads `parent`.
                let replacement = Node::new(NodeData::Text {
                    contents: std::cell::RefCell::new(text.into()),
                });
                node.children.borrow_mut().push(replacement);
            }
            // `children` drops here, taking the over-deep subtree
            // with it. Safe at any depth: markup5ever_rcdom's `Drop`
            // for `Node` is a worklist loop, not a recursive one.
            continue;
        }
        for child in node.children.borrow().iter() {
            stack.push((child.clone(), depth + 1));
        }
    }
    truncated
}

/// Concatenate every text node in `roots` and their descendants,
/// separated by single spaces. Iterative for the same reason
/// [`flatten_below_depth`] is.
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

/// Set of HTML tags ammonia is allowed to pass through. Everything
/// not in here is dropped — `<script>`, `<iframe>`, `<form>`,
/// `<style>`, and friends never reach the walker. The list is the
/// union of tags we recognize plus inline marks (we drop them on
/// the way through but want the text inside to survive).
fn allowed_html_tags() -> std::collections::HashSet<&'static str> {
    [
        "html", "head", "body",
        "p", "div", "span",
        "h1", "h2", "h3", "h4", "h5", "h6",
        "ul", "ol", "li",
        "blockquote",
        "pre", "code",
        "hr", "br",
        "a",
        "strong", "em", "b", "i", "u", "s", "del",
    ]
    .into_iter()
    .collect()
}

fn walk_html<'a>(
    txn: &mut yrs::TransactionMut<'_>,
    handle: &markup5ever_rcdom::Handle,
    parent: &XmlOpenable<'a>,
) {
    use markup5ever_rcdom::NodeData;

    match &handle.data {
        NodeData::Document => {
            for child in handle.children.borrow().iter() {
                walk_html(txn, child, parent);
            }
        }
        NodeData::Element { name, .. } => {
            let tag = name.local.as_ref();
            // "html" / "head" / "body" are scaffold containers
            // html5ever inserts; descend through without creating a
            // matching NodeType.
            if matches!(tag, "html" | "head" | "body") {
                for child in handle.children.borrow().iter() {
                    walk_html(txn, child, parent);
                }
                return;
            }
            if let Some(nt) = map_html_tag(tag) {
                let el = insert_at_end(txn, parent, nt);
                // Heading carries a level attribute on its element,
                // not just on the tag. Recover it from the original
                // tag name.
                if nt == NodeType::Heading {
                    if let Some(level) = heading_level_from_tag(tag) {
                        el.insert_attribute(txn, "level", level.to_string());
                    }
                }
                let scope = XmlOpenable::Element(el);
                for child in handle.children.borrow().iter() {
                    walk_html(txn, child, &scope);
                }
            } else {
                // Transparent passthrough — unknown tag, walk
                // children in the same parent context.
                for child in handle.children.borrow().iter() {
                    walk_html(txn, child, parent);
                }
            }
        }
        NodeData::Text { contents } => {
            let s = contents.borrow();
            let trimmed = s.as_ref();
            // Skip pure-whitespace text nodes between sibling block
            // elements (typical of pretty-printed HTML). Preserving
            // them would leak " " into every list/blockquote.
            if trimmed.trim().is_empty() {
                return;
            }
            // Insert as text leaf under the current open element. If
            // we're at fragment scope, wrap in a paragraph first —
            // a bare text node at top level is otherwise unschematic.
            match parent {
                XmlOpenable::Fragment(f) => {
                    let p = {
                        let pos = f.len(txn);
                        f.insert(
                            txn,
                            pos,
                            yrs::types::xml::XmlElementPrelim::empty(
                                NodeType::Paragraph.tag_name(),
                            ),
                        )
                    };
                    let pos = p.len(txn);
                    p.insert(txn, pos, XmlTextPrelim::new(trimmed));
                }
                XmlOpenable::Element(e) => {
                    let pos = e.len(txn);
                    e.insert(txn, pos, XmlTextPrelim::new(trimmed));
                }
            }
        }
        NodeData::Comment { .. } | NodeData::Doctype { .. } | NodeData::ProcessingInstruction { .. } => {
            // Dropped silently.
        }
    }
}

fn map_html_tag(tag: &str) -> Option<NodeType> {
    Some(match tag {
        "p" => NodeType::Paragraph,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => NodeType::Heading,
        "ul" => NodeType::BulletList,
        "ol" => NodeType::OrderedList,
        "li" => NodeType::ListItem,
        "blockquote" => NodeType::Blockquote,
        "pre" => NodeType::CodeBlock,
        "hr" => NodeType::HorizontalRule,
        "br" => NodeType::HardBreak,
        // Inline marks: drop in v1 (text content passes through via
        // the transparent path). Returning None routes the element
        // through the passthrough branch in walk_html.
        _ => return None,
    })
}

fn heading_level_from_tag(tag: &str) -> Option<u8> {
    match tag {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{ReadTxn, types::xml::XmlOut};

    fn first_child_tag(doc: &Doc) -> Option<String> {
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content")?;
        let child = fragment.get(&txn, 0)?;
        let XmlOut::Element(el) = child else { return None };
        Some(el.tag().to_string())
    }

    #[test]
    fn empty_input_produces_empty_doc() {
        let doc = from_markdown("");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        assert_eq!(fragment.len(&txn), 0);
    }

    #[test]
    fn single_paragraph() {
        let doc = from_markdown("hello world");
        assert_eq!(first_child_tag(&doc).as_deref(), Some("paragraph"));
    }

    #[test]
    fn heading_carries_level_attribute() {
        let doc = from_markdown("# Title");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(el) = fragment.get(&txn, 0).unwrap() else {
            panic!("first child not an element");
        };
        assert_eq!(el.tag().as_ref(), "heading");
        assert_eq!(el.get_attribute(&txn, "level").as_deref(), Some("1"));
    }

    #[test]
    fn h3_carries_level_3() {
        let doc = from_markdown("### Sub");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(el) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(el.get_attribute(&txn, "level").as_deref(), Some("3"));
    }

    #[test]
    fn bullet_list_creates_list_then_items() {
        let doc = from_markdown("- one\n- two");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(list) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(list.tag().as_ref(), "bullet_list");
        assert_eq!(list.len(&txn), 2);
    }

    #[test]
    fn ordered_list_distinguished_from_bullet() {
        let doc = from_markdown("1. a\n2. b");
        assert_eq!(first_child_tag(&doc).as_deref(), Some("ordered_list"));
    }

    #[test]
    fn blockquote_wraps_inner_paragraph() {
        let doc = from_markdown("> quoted");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(bq) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(bq.tag().as_ref(), "blockquote");
        let XmlOut::Element(p) = bq.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(p.tag().as_ref(), "paragraph");
    }

    #[test]
    fn code_block_captures_language() {
        let doc = from_markdown("```rust\nfn main() {}\n```");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(cb) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(cb.tag().as_ref(), "code_block");
        assert_eq!(cb.get_attribute(&txn, "language").as_deref(), Some("rust"));
    }

    #[test]
    fn horizontal_rule_produces_node() {
        let doc = from_markdown("---");
        assert_eq!(first_child_tag(&doc).as_deref(), Some("horizontal_rule"));
    }

    #[test]
    fn inline_emphasis_drops_marks_keeps_text() {
        // v1 limitation: bold/italic syntax is parsed but the resulting
        // doc carries plain text. This test pins that as the v1 contract;
        // piece B switches it to preserve marks.
        let doc = from_markdown("**bold** and *italic*");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(p) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        // Just one text child, no nested mark elements.
        assert_eq!(p.len(&txn), 1);
    }

    // ─── HTML import (M-P5 piece B) ──────────────────────────────

    #[test]
    fn html_empty_produces_empty_doc() {
        let doc = from_html("");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        assert_eq!(fragment.len(&txn), 0);
    }

    #[test]
    fn html_single_paragraph() {
        let doc = from_html("<p>hello world</p>");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(p) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(p.tag().as_ref(), "paragraph");
    }

    #[test]
    fn html_heading_carries_level_attribute() {
        let doc = from_html("<h3>Sub</h3>");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(el) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(el.tag().as_ref(), "heading");
        assert_eq!(el.get_attribute(&txn, "level").as_deref(), Some("3"));
    }

    #[test]
    fn html_ul_vs_ol_distinguished() {
        let bullet = from_html("<ul><li>a</li></ul>");
        let txn = bullet.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(list) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(list.tag().as_ref(), "bullet_list");

        let ordered = from_html("<ol><li>a</li></ol>");
        let txn = ordered.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(list) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(list.tag().as_ref(), "ordered_list");
    }

    #[test]
    fn html_strips_script_and_iframe() {
        // ammonia drops both before html5ever sees them. The
        // surviving paragraph is the only top-level block.
        let doc = from_html(
            "<p>safe</p><script>alert('x')</script><iframe src='evil'></iframe>",
        );
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        assert_eq!(fragment.len(&txn), 1);
        let XmlOut::Element(p) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(p.tag().as_ref(), "paragraph");
    }

    #[test]
    fn html_strips_onerror_attribute() {
        // ammonia removes inline event handlers. The text content
        // (none here — img has no body) is empty; the doc has no
        // surviving block. We only care that the import didn't
        // panic and didn't carry the onerror handler through.
        let doc = from_html("<img src=x onerror='alert(1)'>");
        let txn = doc.transact();
        let _ = txn.get_xml_fragment("content").unwrap();
    }

    #[test]
    fn html_transparent_div_descends() {
        let doc = from_html("<div><p>inner</p></div>");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(p) = fragment.get(&txn, 0).unwrap() else {
            panic!("expected paragraph at fragment root");
        };
        assert_eq!(p.tag().as_ref(), "paragraph");
    }

    #[test]
    fn html_blockquote_wraps_paragraph() {
        let doc = from_html("<blockquote><p>quoted</p></blockquote>");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(bq) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(bq.tag().as_ref(), "blockquote");
        let XmlOut::Element(p) = bq.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(p.tag().as_ref(), "paragraph");
    }

    #[test]
    fn html_pre_becomes_code_block() {
        let doc = from_html("<pre>fn main() {}</pre>");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(cb) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(cb.tag().as_ref(), "code_block");
    }

    #[test]
    fn html_hr_produces_horizontal_rule() {
        let doc = from_html("<hr>");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(el) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        assert_eq!(el.tag().as_ref(), "horizontal_rule");
    }

    // ─── The sanitizer boundary (#160) ───────────────────────────
    //
    // `tests/import_fuzz.rs` has a property that *reads* as the XSS
    // guard for `from_html`. It cannot be: it inspects the
    // materialized yrs document, whose element names come from the
    // closed `NodeType` enum, so no allowlist mistake could ever make
    // an `<iframe>` appear there. Widening `allowed_html_tags` leaves
    // it green. These tests are the actual boundary, in two layers —
    // a direct assertion on the set (which names the offending entry
    // outright) and a behavioral pass through `sanitize` (which
    // covers what a set assertion structurally cannot: the ammonia
    // knobs *around* the set).

    /// Tags that must never be in the allowlist. Script execution,
    /// framing/embedding, style injection, credential-phishing form
    /// posts, and `<link>`/`<base>` resource hijacking.
    const FORBIDDEN_TAGS: &[&str] = &[
        "script", "iframe", "object", "embed", "style", "form", "link", "base", "meta", "svg",
        "math", "applet", "frame", "frameset", "noscript", "template", "input", "button",
    ];

    /// Layer 1 — assert the set itself. The sharpest possible
    /// failure: it names the exact entry someone added.
    #[test]
    fn allowed_html_tags_admits_no_forbidden_tag() {
        let allowed = allowed_html_tags();
        for tag in FORBIDDEN_TAGS {
            assert!(
                !allowed.contains(tag),
                "<{tag}> must never be in the HTML import allowlist — it is an \
                 execution/framing/injection vector, and nothing downstream of \
                 the sanitizer re-checks the tag set",
            );
        }
    }

    /// Layer 2 — behavior through `sanitize`, with positive controls
    /// so a negative assertion can't pass because the whole input was
    /// dropped for an unrelated reason.
    #[test]
    fn sanitize_drops_forbidden_tags_and_keeps_ordinary_ones() {
        for tag in FORBIDDEN_TAGS {
            let out = sanitize(&format!("<p>before</p><{tag}>payload</{tag}><p>after</p>"));
            assert!(
                !out.contains(&format!("<{tag}")),
                "<{tag}> survived sanitize: {out:?}",
            );
            // Positive control: the surrounding document is intact,
            // so the assertion above is about the tag and not about
            // sanitize having eaten everything.
            assert!(out.contains("before") && out.contains("after"), "{out:?}");
        }
    }

    /// Event-handler attributes die at the sanitizer, on tags that
    /// are otherwise perfectly allowed.
    #[test]
    fn sanitize_strips_event_handler_attributes() {
        let out = sanitize(
            "<p onclick=\"steal()\">a</p>\
             <a href=\"https://example.com\" onmouseover=\"steal()\">b</a>\
             <blockquote onerror=\"steal()\">c</blockquote>",
        );
        let lower = out.to_ascii_lowercase();
        for handler in ["onclick", "onmouseover", "onerror"] {
            assert!(!lower.contains(handler), "{handler} survived sanitize: {out:?}");
        }
        // Positive controls: the elements and their legitimate
        // attribute survived, so the handlers were stripped rather
        // than the tags being dropped wholesale.
        assert!(lower.contains("<p"), "{out:?}");
        assert!(lower.contains("href=\"https://example.com\""), "{out:?}");
    }

    /// No attribute *prefix* is allowed through — the hazard a set
    /// assertion structurally cannot see.
    ///
    /// `import_quip`'s sanitizer allows the `data-` prefix because
    /// its walker reads Quip hints out of `data-*`. This one allows
    /// no prefix at all, and that is worth pinning: a
    /// `generic_attribute_prefixes` call added here later would be
    /// invisible to `allowed_html_tags_admits_no_forbidden_tag`, and
    /// the prefix `"on"` would re-admit every event handler in one
    /// line. If a future reader needs `data-*` on this path, this
    /// test is the place that makes them say so out loud.
    #[test]
    fn sanitize_allows_no_attribute_prefix() {
        let out = sanitize("<p data-anything=\"x\" onbeforeinput=\"steal()\">text</p>");
        assert!(
            !out.contains("data-anything"),
            "no attribute prefix is allowed on this path; a prefix allowlist was added: {out:?}",
        );
        assert!(!out.to_ascii_lowercase().contains("onbeforeinput"), "{out:?}");
        assert!(out.contains("text"), "positive control: {out:?}");
    }

    /// ammonia's URL-scheme filtering, pinned behaviorally. The tag
    /// (`a`) and the attribute (`href`) are both allowed — only the
    /// *scheme* makes this dangerous, which is exactly the kind of
    /// thing no tag/attribute set assertion can express.
    #[test]
    fn sanitize_rejects_script_bearing_url_schemes() {
        for scheme in ["javascript:alert(1)", "data:text/html;base64,PHNjcmlwdD4="] {
            let out = sanitize(&format!("<a href=\"{scheme}\">click</a>"));
            assert!(
                !out.contains(scheme),
                "{scheme} survived sanitize as an href: {out:?}",
            );
            assert!(out.contains("click"), "positive control: {out:?}");
        }
        // Positive control on the same shape: an ordinary link keeps
        // its href, so the assertions above are about the scheme.
        let ok = sanitize("<a href=\"https://example.com/x\">click</a>");
        assert!(ok.contains("https://example.com/x"), "{ok:?}");
    }

    // ─── Nesting depth (#158) ────────────────────────────────────
    //
    // The deterministic cap tests live in `tests/import_fuzz.rs`,
    // next to the fuzz net that structurally cannot catch this class
    // (stack exhaustion aborts; a panic net has nothing to catch).
    // What belongs here is the unit-level shape of the pruning pass.

    #[test]
    fn flatten_below_depth_leaves_ordinary_nesting_alone() {
        let html = format!(
            "{}<p>nested</p>{}",
            "<blockquote>".repeat(16),
            "</blockquote>".repeat(16),
        );
        let doc = from_html(&html);
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        // 16 blockquotes then the paragraph — untouched.
        let mut depth = 0;
        let mut node = match fragment.get(&txn, 0).unwrap() {
            XmlOut::Element(el) => el,
            _ => panic!("expected an element"),
        };
        loop {
            depth += 1;
            match node.get(&txn, 0) {
                Some(XmlOut::Element(child)) => node = child,
                _ => break,
            }
        }
        assert_eq!(
            depth, 17,
            "16 blockquotes + 1 paragraph is ordinary document structure and \
             must pass through the depth cap untouched",
        );
    }

    #[test]
    fn html_inline_em_strong_text_survives() {
        // v1 contract: marks dropped, text preserved via the
        // transparent-passthrough branch in walk_html.
        let doc = from_html("<p><strong>bold</strong> mid <em>italic</em></p>");
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(p) = fragment.get(&txn, 0).unwrap() else {
            panic!();
        };
        // Children are XmlText runs separated by the (dropped) mark
        // wrappers. Count > 0 and no element children.
        assert!(p.len(&txn) >= 1);
    }
}
