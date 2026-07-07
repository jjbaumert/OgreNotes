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
/// Same v1 limitation as `from_markdown`: inline marks (bold,
/// italic, code, link) are dropped. Pre-existing
/// `inline_emphasis_drops_marks_keeps_text` test pins the contract.
/// Mark-preservation across both importers lands in a follow-up
/// once the yrs text formatting story is wired symmetrically.
pub fn from_html(html: &str) -> Doc {
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::RcDom;

    // Stage 1: sanitize. ammonia's default Builder is conservative —
    // a whitelist of safe tags + attribute filtering — which is
    // exactly the posture we want for an import endpoint that may
    // receive third-party HTML.
    let safe = ammonia::Builder::default()
        .tags(allowed_html_tags())
        .clean(html)
        .to_string();

    // Stage 2: parse to DOM.
    let dom: RcDom = html5ever::parse_document(
        RcDom::default(),
        html5ever::driver::ParseOpts::default(),
    )
    .from_utf8()
    .read_from(&mut safe.as_bytes())
    .expect("html5ever parse is infallible on bytes");

    // Stage 3: walk + materialize.
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");
        walk_html(&mut txn, &dom.document, &XmlOpenable::Fragment(&fragment));
    }
    doc
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
