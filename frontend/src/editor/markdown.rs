// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Markdown paste support: parse a markdown string into a document `Slice`.
//!
//! Used by the paste handler's plain-text fallback. CommonMark + GFM
//! (strikethrough, tasklists, tables) constructs map onto the editor's node
//! types. Plain prose with no markdown syntax parses into Paragraph nodes
//! identically to `clipboard::parse_from_text` for paste-flow purposes.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};
use std::collections::HashMap;

use super::model::{Fragment, Mark, MarkType, Node, NodeType, Slice};
use super::view::is_safe_url;

/// Classify a `Slice` (produced by HTML parsing) as "trivial" — meaning it
/// carries no formatting or break structure beyond paragraph wrapping. When
/// the clipboard's `text/html` payload is just plain text wrapped in `<p>`
/// tags (a common case when copying raw markdown source), we treat it as
/// trivial and re-parse the `text/plain` payload as markdown instead so that
/// syntax like `**bold**` is honored.
///
/// A slice is trivial iff:
/// - It is non-empty.
/// - Every top-level child is a Paragraph with zero marks.
/// - Every Paragraph's content is zero or more Text nodes with zero marks.
///
/// In particular, a top-level bare `HardBreak`, `HorizontalRule`, or any
/// structural/formatted element is non-trivial; and a Paragraph containing a
/// `HardBreak` is non-trivial too — switching to markdown parsing on plain
/// text would silently drop the break, since a single newline in CommonMark
/// is a soft break (collapsed to a space), not a hard break.
pub fn is_trivial_slice(slice: &Slice) -> bool {
    fn paragraph_child_is_trivial(n: &Node) -> bool {
        matches!(n, Node::Text { marks, .. } if marks.is_empty())
    }
    fn top_child_is_trivial(n: &Node) -> bool {
        match n {
            Node::Text { marks, .. } => marks.is_empty(),
            Node::Element { node_type, content, marks, .. } => {
                marks.is_empty()
                    && *node_type == NodeType::Paragraph
                    && content.children.iter().all(paragraph_child_is_trivial)
            }
        }
    }
    !slice.content.children.is_empty()
        && slice.content.children.iter().all(top_child_is_trivial)
}

/// Parse a markdown string into a document `Slice`.
pub fn parse_from_markdown(src: &str) -> Slice {
    if src.is_empty() {
        return Slice::empty();
    }
    // Normalize Windows-style line endings before pulldown-cmark sees the
    // input. pulldown-cmark generally tolerates `\r\n`, but our autolink
    // scanner indexes into the source by byte offset, so a stray `\r`
    // mid-paragraph could land in text runs unexpectedly.
    let normalized = super::clipboard::normalize_line_endings(src);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_GFM);

    let parser = Parser::new_ext(&normalized, opts);
    let mut builder = Builder::new();
    for event in parser {
        builder.handle(event);
    }
    let children = builder.finish();
    if children.is_empty() {
        return Slice::empty();
    }
    Slice::new(Fragment::from(children), 0, 0)
}

// ─── Builder ────────────────────────────────────────────────────

struct Frame {
    node_type: NodeType,
    attrs: HashMap<String, String>,
    children: Vec<Node>,
    /// Set when `TaskListMarker` fires inside an Item frame.
    task_checked: Option<bool>,
    /// Set on List frames when a child item is a TaskItem.
    has_task_child: bool,
    /// True if opened internally (e.g., wrap text that fell outside a textblock).
    auto: bool,
    /// True if this frame should be discarded on close, dropping its
    /// children. Used for unsupported constructs (footnotes, definition
    /// lists, HTML blocks) and for Images whose URL failed the safety check.
    discard: bool,
    /// True on a TableRow frame that lives inside a TableHead — its cells
    /// become TableHeader rather than TableCell.
    in_head: bool,
    /// Only meaningful on Table frames: per-column alignment from the source.
    column_alignments: Vec<Alignment>,
    /// Only meaningful on TableRow frames: next cell's column index.
    next_column: usize,
}

impl Frame {
    fn new(node_type: NodeType) -> Self {
        Self {
            node_type,
            attrs: HashMap::new(),
            children: Vec::new(),
            task_checked: None,
            has_task_child: false,
            auto: false,
            discard: false,
            in_head: false,
            column_alignments: Vec::new(),
            next_column: 0,
        }
    }
}

struct Builder {
    stack: Vec<Frame>,
    output: Vec<Node>,
    marks: Vec<Mark>,
    /// When present, we're inside a CodeBlock and all text goes into this buffer.
    code_buffer: Option<String>,
}

impl Builder {
    fn new() -> Self {
        Self {
            stack: Vec::new(),
            output: Vec::new(),
            marks: Vec::new(),
            code_buffer: None,
        }
    }

    fn finish(mut self) -> Vec<Node> {
        while !self.stack.is_empty() {
            self.close_top();
        }
        self.output
    }

    fn handle(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag_end) => self.end_tag(tag_end),
            Event::Text(s) => self.push_text(&s),
            Event::Code(s) => self.push_code(&s),
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.push_hard_break(),
            Event::Rule => self.push_hr(),
            Event::TaskListMarker(b) => self.mark_task(b),
            Event::InlineHtml(s) => self.handle_inline_html(&s),
            Event::Html(_)
            | Event::FootnoteReference(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {}
        }
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Emphasis => {
                self.marks.push(Mark::new(MarkType::Italic));
                return;
            }
            Tag::Strong => {
                self.marks.push(Mark::new(MarkType::Bold));
                return;
            }
            Tag::Strikethrough => {
                self.marks.push(Mark::new(MarkType::Strike));
                return;
            }
            Tag::Link { ref dest_url, ref title, link_type, .. } => {
                // Email autolinks arrive without the `mailto:` scheme; add it so
                // is_safe_url accepts them and so the rendered <a href> is clickable.
                let resolved = if link_type == LinkType::Email {
                    format!("mailto:{}", dest_url)
                } else {
                    dest_url.to_string()
                };
                let href = if is_safe_url(&resolved) { resolved } else { String::new() };
                // Always push a mark so End(Link) pops predictably; filter the
                // sentinel mark with empty href at emit time.
                let mut mark = Mark::new(MarkType::Link).with_attr("href", &href);
                if !title.is_empty() && !href.is_empty() {
                    mark = mark.with_attr("title", title);
                }
                self.marks.push(mark);
                return;
            }
            _ => {}
        }

        self.close_auto_frames();

        match tag {
            Tag::Paragraph => self.open(Frame::new(NodeType::Paragraph)),
            Tag::Heading { level, .. } => {
                let mut f = Frame::new(NodeType::Heading);
                f.attrs.insert("level".to_string(), heading_level_str(level).to_string());
                self.open(f);
            }
            Tag::BlockQuote(_) => self.open(Frame::new(NodeType::Blockquote)),
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let mut f = Frame::new(NodeType::CodeBlock);
                f.attrs.insert("language".to_string(), lang);
                self.open(f);
                self.code_buffer = Some(String::new());
            }
            Tag::List(None) => self.open(Frame::new(NodeType::BulletList)),
            Tag::List(Some(_)) => self.open(Frame::new(NodeType::OrderedList)),
            Tag::Item => self.open(Frame::new(NodeType::ListItem)),
            Tag::Table(alignments) => {
                let mut f = Frame::new(NodeType::Table);
                f.column_alignments = alignments;
                self.open(f);
            }
            Tag::TableHead => {
                // Rendered as a TableRow whose cells will be TableHeader.
                let mut f = Frame::new(NodeType::TableRow);
                f.in_head = true;
                self.open(f);
            }
            Tag::TableRow => self.open(Frame::new(NodeType::TableRow)),
            Tag::TableCell => {
                let is_head = self.stack.last().is_some_and(|f|
                    f.node_type == NodeType::TableRow && f.in_head);
                let nt = if is_head { NodeType::TableHeader } else { NodeType::TableCell };
                let align = self.lookup_column_alignment();
                // Advance the row's column counter for the next cell.
                if let Some(row) = self.stack.last_mut() {
                    if row.node_type == NodeType::TableRow {
                        row.next_column += 1;
                    }
                }
                let mut f = Frame::new(nt);
                if let Some(a) = align {
                    f.attrs.insert("align".to_string(), a.to_string());
                }
                self.open(f);
            }
            Tag::Image { ref dest_url, ref title, .. } => {
                let mut f = Frame::new(NodeType::Image);
                if is_safe_url(dest_url) {
                    f.attrs.insert("src".to_string(), dest_url.to_string());
                } else {
                    f.discard = true;
                }
                if !title.is_empty() {
                    f.attrs.insert("title".to_string(), title.to_string());
                }
                self.open(f);
            }
            // Unsupported constructs — open a discarded frame so the stack
            // stays balanced; close_top throws the children away.
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_)
            | Tag::HtmlBlock => {
                let mut f = Frame::new(NodeType::Paragraph);
                f.discard = true;
                self.open(f);
            }
            // Inline mark tags handled above.
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } => unreachable!(),
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.marks.pop();
                return;
            }
            _ => {}
        }

        // Close any leftover auto-frames that do not match this end tag.
        while let Some(top) = self.stack.last() {
            if top.auto && !tag_end_matches(top.node_type, &tag) {
                self.close_top();
            } else {
                break;
            }
        }

        if matches!(tag, TagEnd::CodeBlock) {
            if let Some(buf) = self.code_buffer.take() {
                if let Some(top) = self.stack.last_mut() {
                    if top.node_type == NodeType::CodeBlock && !buf.is_empty() {
                        top.children.push(Node::text(&buf));
                    }
                }
            }
        }

        if matches!(tag, TagEnd::Image)
            && self.stack.last().is_some_and(|f| f.node_type == NodeType::Image)
        {
            let frame = self.stack.pop().unwrap();
            if frame.discard {
                return;
            }
            let mut attrs = frame.attrs;
            let alt: String = frame.children.iter().map(|n| n.text_content()).collect();
            if !alt.is_empty() {
                attrs.insert("alt".to_string(), alt);
            }
            let img = Node::element_with_attrs(NodeType::Image, attrs, Fragment::empty());
            self.push_child(img);
            return;
        }

        self.close_top();
    }

    fn open(&mut self, frame: Frame) {
        self.stack.push(frame);
    }

    fn close_auto_frames(&mut self) {
        while let Some(top) = self.stack.last() {
            if top.auto {
                self.close_top();
            } else {
                break;
            }
        }
    }

    fn close_top(&mut self) {
        let Some(frame) = self.stack.pop() else { return };
        if frame.discard {
            return;
        }
        if let Some(node) = self.finalize_frame(frame) {
            self.push_child(node);
        }
    }

    fn finalize_frame(&mut self, frame: Frame) -> Option<Node> {
        match frame.node_type {
            NodeType::ListItem | NodeType::TaskItem => {
                let mut attrs = frame.attrs;
                let node_type = match frame.task_checked {
                    Some(checked) => {
                        attrs.insert("checked".to_string(), checked.to_string());
                        // Tell the enclosing list to promote itself to TaskList.
                        if let Some(parent) = self.stack.last_mut() {
                            if matches!(parent.node_type,
                                NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList)
                            {
                                parent.has_task_child = true;
                            }
                        }
                        NodeType::TaskItem
                    }
                    None => NodeType::ListItem,
                };
                Some(Node::element_with_attrs(
                    node_type,
                    attrs,
                    Fragment::from(frame.children),
                ))
            }
            NodeType::BulletList | NodeType::OrderedList if frame.has_task_child => {
                let children: Vec<Node> = frame
                    .children
                    .into_iter()
                    .map(|c| match c {
                        Node::Element {
                            node_type: NodeType::ListItem,
                            content,
                            ..
                        } => {
                            let mut a = HashMap::new();
                            a.insert("checked".to_string(), "false".to_string());
                            Node::element_with_attrs(NodeType::TaskItem, a, content)
                        }
                        other => other,
                    })
                    .collect();
                Some(Node::element_with_content(
                    NodeType::TaskList,
                    Fragment::from(children),
                ))
            }
            NodeType::TableCell | NodeType::TableHeader => {
                let all_inline = frame.children.iter().all(|n| {
                    matches!(n, Node::Text { .. })
                        || n.node_type().is_some_and(|t| t.is_inline())
                });
                let content = if all_inline && !frame.children.is_empty() {
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(frame.children),
                    )])
                } else {
                    Fragment::from(frame.children)
                };
                Some(Node::element_with_attrs(frame.node_type, frame.attrs, content))
            }
            // Drop a Paragraph that ended up empty — typically the wrapper
            // around an image whose URL was rejected. Empty Headings and
            // CodeBlocks are user-intentional (e.g., `# ` with no text) and
            // are preserved.
            NodeType::Paragraph if frame.children.is_empty() => None,
            _ => Some(Node::element_with_attrs(
                frame.node_type,
                frame.attrs,
                Fragment::from(frame.children),
            )),
        }
    }

    fn push_child(&mut self, node: Node) {
        if let Some(top) = self.stack.last_mut() {
            top.children.push(node);
        } else {
            self.output.push(node);
        }
    }

    fn ensure_textblock(&mut self) {
        let needs_wrap = self
            .stack
            .last()
            .map_or(true, |f| !f.node_type.is_textblock() && f.node_type != NodeType::Image);
        if needs_wrap {
            let mut f = Frame::new(NodeType::Paragraph);
            f.auto = true;
            self.stack.push(f);
        }
    }

    fn effective_marks(&self) -> Vec<Mark> {
        let mut marks: Vec<Mark> = self
            .marks
            .iter()
            .filter(|m| {
                m.mark_type != MarkType::Link
                    || m.attrs.get("href").is_some_and(|h| !h.is_empty())
            })
            .cloned()
            .collect();
        if marks.iter().any(|m| m.mark_type == MarkType::Code) {
            marks.retain(|m| m.mark_type == MarkType::Code);
        }
        marks
    }

    fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if let Some(buf) = self.code_buffer.as_mut() {
            buf.push_str(s);
            return;
        }
        // Inside an Image frame, text accumulates as the alt attribute —
        // no marks, no autolink scanning.
        if self.stack.last().is_some_and(|f| f.node_type == NodeType::Image) {
            self.stack.last_mut().unwrap().children.push(Node::text(s));
            return;
        }
        self.ensure_textblock();
        let marks = self.effective_marks();
        // Skip bare-URL autolinking when the run is already wrapped in a Link
        // (don't double-link) or in Code (URL chars are literal).
        let suppress_autolinks = marks.iter().any(|m|
            matches!(m.mark_type, MarkType::Link | MarkType::Code));
        if suppress_autolinks {
            self.emit_text(s, &marks);
        } else {
            self.emit_with_autolinks(s, &marks);
        }
    }

    /// Emit `s` as one or more text runs, splitting around bare URLs
    /// (`http://...` / `https://...`) and wrapping each URL in a Link mark
    /// alongside the caller-provided marks. Pulldown-cmark does not autolink
    /// bare URLs in prose, so this fills that gap.
    fn emit_with_autolinks(&mut self, s: &str, marks: &[Mark]) {
        let mut cursor = 0;
        while let Some((start, end)) = find_bare_url(s, cursor) {
            if start > cursor {
                self.emit_text(&s[cursor..start], marks);
            }
            let url = &s[start..end];
            let mut link_marks = marks.to_vec();
            link_marks.push(Mark::new(MarkType::Link).with_attr("href", url));
            self.emit_text(url, &link_marks);
            cursor = end;
        }
        if cursor < s.len() {
            self.emit_text(&s[cursor..], marks);
        }
    }

    fn emit_text(&mut self, s: &str, marks: &[Mark]) {
        if s.is_empty() {
            return;
        }
        let node = if marks.is_empty() {
            Node::text(s)
        } else {
            Node::text_with_marks(s, marks.to_vec())
        };
        self.push_child(node);
    }

    fn push_code(&mut self, s: &str) {
        if let Some(buf) = self.code_buffer.as_mut() {
            buf.push_str(s);
            return;
        }
        self.ensure_textblock();
        self.push_child(Node::text_with_marks(s, vec![Mark::new(MarkType::Code)]));
    }

    fn push_hard_break(&mut self) {
        if let Some(buf) = self.code_buffer.as_mut() {
            buf.push('\n');
            return;
        }
        self.ensure_textblock();
        self.push_child(Node::element(NodeType::HardBreak));
    }

    fn push_hr(&mut self) {
        self.close_auto_frames();
        self.push_child(Node::element(NodeType::HorizontalRule));
    }

    fn handle_inline_html(&mut self, raw: &str) {
        let Some(tag) = parse_html_tag(raw) else { return };
        let name = tag.name.to_ascii_lowercase();
        let mark_type = match name.as_str() {
            "b" | "strong" => Some(MarkType::Bold),
            "i" | "em" => Some(MarkType::Italic),
            "u" => Some(MarkType::Underline),
            "s" | "strike" | "del" => Some(MarkType::Strike),
            "code" | "kbd" | "tt" | "samp" => Some(MarkType::Code),
            "mark" => Some(MarkType::Highlight),
            "sub" => Some(MarkType::Subscript),
            "sup" => Some(MarkType::Superscript),
            _ => None,
        };
        if let Some(mt) = mark_type {
            if tag.is_close {
                // Remove the innermost matching mark. Non-LIFO removal is safe
                // here because emit_text re-reads the effective mark stack on
                // every text event.
                if let Some(idx) = self.marks.iter().rposition(|m| m.mark_type == mt) {
                    self.marks.remove(idx);
                }
            } else if !tag.self_close {
                self.marks.push(Mark::new(mt));
            }
            return;
        }
        match name.as_str() {
            "br" => {
                // <br>, <br/>, <br /> all emit a HardBreak; </br> ignored.
                if !tag.is_close {
                    self.push_hard_break();
                }
            }
            "a" => {
                if tag.is_close {
                    if let Some(idx) = self.marks.iter().rposition(|m| m.mark_type == MarkType::Link) {
                        self.marks.remove(idx);
                    }
                } else {
                    let href_raw = tag.attr("href").unwrap_or_default();
                    let href = if is_safe_url(href_raw) { href_raw.to_string() } else { String::new() };
                    let mut mark = Mark::new(MarkType::Link).with_attr("href", &href);
                    if let Some(t) = tag.attr("title") {
                        if !t.is_empty() && !href.is_empty() {
                            mark = mark.with_attr("title", t);
                        }
                    }
                    self.marks.push(mark);
                }
            }
            // Transparent containers (sub, sup, span, etc.) and unknown tags:
            // drop the tag itself; any text inside still emits.
            _ => {}
        }
    }

    /// Resolve the alignment for the cell about to be opened, using the
    /// enclosing Row's column counter to index into the Table frame's
    /// `column_alignments`. Returns `None` if unset or out of range.
    fn lookup_column_alignment(&self) -> Option<&'static str> {
        let row = self.stack.iter().rev().find(|f| f.node_type == NodeType::TableRow)?;
        let table = self.stack.iter().rev().find(|f| f.node_type == NodeType::Table)?;
        alignment_str(*table.column_alignments.get(row.next_column)?)
    }

    fn mark_task(&mut self, checked: bool) {
        if let Some(top) = self.stack.last_mut() {
            if matches!(top.node_type, NodeType::ListItem | NodeType::TaskItem) {
                top.task_checked = Some(checked);
            }
        }
    }
}

/// A parsed inline HTML tag, enough for the whitelist mapping in
/// `Builder::handle_inline_html`. Not a general-purpose HTML parser —
/// only handles the single-tag payloads pulldown-cmark emits as
/// `Event::InlineHtml`.
struct HtmlTag {
    name: String,
    is_close: bool,
    self_close: bool,
    attrs: Vec<(String, String)>,
}

impl HtmlTag {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
}

fn parse_html_tag(raw: &str) -> Option<HtmlTag> {
    let s = raw.trim();
    let s = s.strip_prefix('<')?.strip_suffix('>')?;
    let (is_close, rest) = match s.strip_prefix('/') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let mut rest = rest.trim_start();

    // Tag name: leading ASCII-alphanumeric run (HTML tag names).
    let name_end = rest.bytes().position(|b| !b.is_ascii_alphanumeric()).unwrap_or(rest.len());
    if name_end == 0 {
        return None;
    }
    let name = rest[..name_end].to_string();
    rest = rest[name_end..].trim_start();

    // Self-close trailing `/` (only on open tags; `</br/>` is malformed, ignore).
    let (attrs_part, self_close) = if !is_close && rest.ends_with('/') {
        (rest[..rest.len() - 1].trim_end(), true)
    } else {
        (rest.trim_end(), false)
    };

    let attrs = parse_html_attrs(attrs_part);
    Some(HtmlTag { name, is_close, self_close, attrs })
}

/// Parse a sequence of `name="value"` / `name='value'` / `name=value` / `name`
/// attribute pairs. Tolerant: malformed attrs are skipped.
fn parse_html_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' && bytes[i] != b'>' {
            i += 1;
        }
        if name_start == i {
            i += 1;
            continue;
        }
        let name = s[name_start..i].to_string();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            out.push((name, String::new()));
            continue;
        }
        i += 1; // skip '='
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            out.push((name, String::new()));
            break;
        }
        let value = if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            let v_start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            let v = &s[v_start..i];
            if i < bytes.len() {
                i += 1; // closing quote
            }
            v.to_string()
        } else {
            let v_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                i += 1;
            }
            s[v_start..i].to_string()
        };
        out.push((name, value));
    }
    out
}

/// Locate the next bare URL (`http://...` or `https://...`) in `s`, searching
/// from byte offset `from`. Returns `(start, end)` byte offsets.
///
/// Trailing sentence punctuation (`.,;:!?`) and unmatched closing brackets are
/// excluded from the URL, so `"see https://x.com."` yields a URL without the
/// final `.`. Matches only at a word boundary (not preceded by a letter/digit)
/// to avoid rewriting `xhttps://...` occurrences.
fn find_bare_url(s: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        let scheme_len = if bytes[i..].starts_with(b"https://") {
            8
        } else if bytes[i..].starts_with(b"http://") {
            7
        } else {
            i += 1;
            continue;
        };
        // Word boundary: previous byte must not be an identifier-like
        // character (alphanumeric, `_`, or `-`) to avoid rewriting things
        // like `xhttps://…` or `some_https://…` inside code-like prose.
        if i > 0 {
            let prev = bytes[i - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'-' {
                i += scheme_len;
                continue;
            }
        }
        // Consume until whitespace or a clearly-terminal character.
        let mut end = i + scheme_len;
        while end < bytes.len() {
            let c = bytes[end];
            if c.is_ascii_whitespace() || c == b'<' || c == b'>' || c == b'"' || c == b'\'' {
                break;
            }
            end += 1;
        }
        // Strip trailing punctuation that's typically sentence-level, and
        // balance parentheses so `(see https://x.com)` excludes the final `)`.
        while end > i + scheme_len {
            let c = bytes[end - 1];
            if matches!(c, b'.' | b',' | b';' | b':' | b'!' | b'?') {
                end -= 1;
                continue;
            }
            if c == b')' {
                let open = s[i..end].bytes().filter(|&b| b == b'(').count();
                let close = s[i..end].bytes().filter(|&b| b == b')').count();
                if close > open {
                    end -= 1;
                    continue;
                }
            }
            if c == b']' || c == b'}' {
                end -= 1;
                continue;
            }
            break;
        }
        if end > i + scheme_len {
            return Some((i, end));
        }
        i += scheme_len;
    }
    None
}

fn alignment_str(a: Alignment) -> Option<&'static str> {
    match a {
        Alignment::Left => Some("left"),
        Alignment::Center => Some("center"),
        Alignment::Right => Some("right"),
        Alignment::None => None,
    }
}

fn heading_level_str(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "1",
        HeadingLevel::H2 => "2",
        HeadingLevel::H3 => "3",
        HeadingLevel::H4 => "4",
        HeadingLevel::H5 => "5",
        HeadingLevel::H6 => "6",
    }
}

fn tag_end_matches(nt: NodeType, tag: &TagEnd) -> bool {
    match (nt, tag) {
        (NodeType::Paragraph, TagEnd::Paragraph) => true,
        (NodeType::Heading, TagEnd::Heading(_)) => true,
        (NodeType::Blockquote, TagEnd::BlockQuote(_)) => true,
        (NodeType::CodeBlock, TagEnd::CodeBlock) => true,
        (NodeType::BulletList, TagEnd::List(false)) => true,
        (NodeType::OrderedList, TagEnd::List(true)) => true,
        (NodeType::ListItem | NodeType::TaskItem, TagEnd::Item) => true,
        (NodeType::Table, TagEnd::Table) => true,
        (NodeType::TableRow, TagEnd::TableHead | TagEnd::TableRow) => true,
        (NodeType::TableCell | NodeType::TableHeader, TagEnd::TableCell) => true,
        (NodeType::Image, TagEnd::Image) => true,
        _ => false,
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Find the first node matching `pred` in `nodes`, walking recursively.
    fn find<'a>(nodes: &'a [Node], pred: &impl Fn(&Node) -> bool) -> Option<&'a Node> {
        for n in nodes {
            if pred(n) {
                return Some(n);
            }
            if let Node::Element { content, .. } = n {
                if let Some(found) = find(&content.children, pred) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn find_by_type(nodes: &[Node], nt: NodeType) -> Option<&Node> {
        find(nodes, &|n: &Node| n.node_type() == Some(nt))
    }

    fn text_with_mark(nodes: &[Node], mt: MarkType) -> Option<String> {
        let text_pred = |n: &Node| matches!(n, Node::Text { marks, .. } if marks.iter().any(|m| m.mark_type == mt));
        let n = find(nodes, &text_pred)?;
        if let Node::Text { text, .. } = n { Some(text.clone()) } else { None }
    }

    // ─── Block constructs ──────────────────────────────────────────

    #[test]
    fn md_plain_paragraph() {
        let slice = parse_from_markdown("hello world");
        assert_eq!(slice.content.children.len(), 1);
        let p = &slice.content.children[0];
        assert_eq!(p.node_type(), Some(NodeType::Paragraph));
        assert_eq!(p.text_content(), "hello world");
    }

    #[test]
    fn md_two_paragraphs() {
        let slice = parse_from_markdown("a\n\nb");
        assert_eq!(slice.content.children.len(), 2);
        assert_eq!(slice.content.children[0].node_type(), Some(NodeType::Paragraph));
        assert_eq!(slice.content.children[0].text_content(), "a");
        assert_eq!(slice.content.children[1].text_content(), "b");
    }

    #[test]
    fn md_soft_break_joins_lines() {
        let slice = parse_from_markdown("a\nb");
        assert_eq!(slice.content.children.len(), 1);
        assert_eq!(slice.content.children[0].text_content(), "a b");
    }

    #[test]
    fn md_heading_levels_1_through_6() {
        let slice = parse_from_markdown("# one\n\n## two\n\n### three\n\n#### four\n\n##### five\n\n###### six");
        assert_eq!(slice.content.children.len(), 6);
        for (i, expected) in ["1", "2", "3", "4", "5", "6"].iter().enumerate() {
            let h = &slice.content.children[i];
            assert_eq!(h.node_type(), Some(NodeType::Heading), "child {} not heading", i);
            assert_eq!(h.attrs().get("level").map(|s| s.as_str()), Some(*expected));
        }
        assert_eq!(slice.content.children[0].text_content(), "one");
        assert_eq!(slice.content.children[5].text_content(), "six");
    }

    #[test]
    fn md_bullet_list() {
        let slice = parse_from_markdown("- a\n- b");
        assert_eq!(slice.content.children.len(), 1);
        let list = &slice.content.children[0];
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.child_count(), 2);
        assert_eq!(list.child(0).unwrap().node_type(), Some(NodeType::ListItem));
        assert_eq!(list.child(0).unwrap().text_content(), "a");
        assert_eq!(list.child(1).unwrap().text_content(), "b");
    }

    #[test]
    fn md_ordered_list() {
        let slice = parse_from_markdown("1. a\n2. b");
        let list = &slice.content.children[0];
        assert_eq!(list.node_type(), Some(NodeType::OrderedList));
        assert_eq!(list.child_count(), 2);
        assert_eq!(list.child(0).unwrap().node_type(), Some(NodeType::ListItem));
    }

    #[test]
    fn md_nested_list() {
        let slice = parse_from_markdown("- a\n  - b\n- c");
        let outer = &slice.content.children[0];
        assert_eq!(outer.node_type(), Some(NodeType::BulletList));
        assert_eq!(outer.child_count(), 2);
        let first_item = outer.child(0).unwrap();
        assert_eq!(first_item.node_type(), Some(NodeType::ListItem));
        // First item holds a Paragraph ("a") and a nested BulletList
        let has_nested_list = first_item.child(0).iter().chain(first_item.child(1).iter())
            .any(|c| c.node_type() == Some(NodeType::BulletList));
        assert!(has_nested_list, "expected nested BulletList inside first item");
        let nested = first_item.child(1).unwrap();
        assert_eq!(nested.node_type(), Some(NodeType::BulletList));
        assert_eq!(nested.text_content(), "b");
        let third_item = outer.child(1).unwrap();
        assert_eq!(third_item.text_content(), "c");
    }

    #[test]
    fn md_task_list() {
        let slice = parse_from_markdown("- [ ] a\n- [x] b");
        let list = &slice.content.children[0];
        assert_eq!(list.node_type(), Some(NodeType::TaskList));
        assert_eq!(list.child_count(), 2);
        let first = list.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::TaskItem));
        assert_eq!(first.attrs().get("checked").map(|s| s.as_str()), Some("false"));
        let second = list.child(1).unwrap();
        assert_eq!(second.node_type(), Some(NodeType::TaskItem));
        assert_eq!(second.attrs().get("checked").map(|s| s.as_str()), Some("true"));
    }

    #[test]
    fn md_task_list_mixed_normalizes_bullet_items() {
        let slice = parse_from_markdown("- a\n- [ ] b");
        let list = &slice.content.children[0];
        assert_eq!(list.node_type(), Some(NodeType::TaskList));
        let first = list.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::TaskItem));
        assert_eq!(first.attrs().get("checked").map(|s| s.as_str()), Some("false"));
    }

    #[test]
    fn md_blockquote() {
        let slice = parse_from_markdown("> quoted");
        let bq = &slice.content.children[0];
        assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
        let para = bq.child(0).unwrap();
        assert_eq!(para.node_type(), Some(NodeType::Paragraph));
        assert_eq!(para.text_content(), "quoted");
    }

    #[test]
    fn md_code_block_fenced_with_language() {
        let slice = parse_from_markdown("```rust\nfn x() {}\n```");
        let cb = &slice.content.children[0];
        assert_eq!(cb.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(cb.attrs().get("language").map(|s| s.as_str()), Some("rust"));
        assert_eq!(cb.text_content(), "fn x() {}\n");
    }

    #[test]
    fn md_code_block_fenced_no_language() {
        let slice = parse_from_markdown("```\nx\n```");
        let cb = &slice.content.children[0];
        assert_eq!(cb.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(cb.attrs().get("language").map(|s| s.as_str()), Some(""));
        assert_eq!(cb.text_content(), "x\n");
    }

    #[test]
    fn md_code_block_indented() {
        let slice = parse_from_markdown("    code\n");
        let cb = &slice.content.children[0];
        assert_eq!(cb.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(cb.attrs().get("language").map(|s| s.as_str()), Some(""));
        assert_eq!(cb.text_content(), "code\n");
    }

    #[test]
    fn md_code_block_does_not_interpret_markdown_inside() {
        let slice = parse_from_markdown("```\n# not heading\n**not bold**\n```");
        let cb = &slice.content.children[0];
        assert_eq!(cb.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(cb.text_content(), "# not heading\n**not bold**\n");
        // No Heading or Bold-marked text anywhere in the output.
        assert!(find_by_type(&slice.content.children, NodeType::Heading).is_none());
        assert!(text_with_mark(&slice.content.children, MarkType::Bold).is_none());
    }

    #[test]
    fn md_horizontal_rule() {
        let slice = parse_from_markdown("---");
        assert!(find_by_type(&slice.content.children, NodeType::HorizontalRule).is_some());
    }

    #[test]
    fn md_image_safe_url() {
        let slice = parse_from_markdown("![alt text](https://example.com/x.png)");
        let img = find_by_type(&slice.content.children, NodeType::Image)
            .expect("expected Image node");
        assert_eq!(img.attrs().get("src").map(|s| s.as_str()), Some("https://example.com/x.png"));
        assert_eq!(img.attrs().get("alt").map(|s| s.as_str()), Some("alt text"));
    }

    #[test]
    fn md_image_javascript_url_dropped() {
        let slice = parse_from_markdown("![alt](javascript:alert(1))");
        assert!(find_by_type(&slice.content.children, NodeType::Image).is_none(),
            "Image with unsafe URL must not be emitted");
        // The paragraph that wrapped the image must also be suppressed when
        // it would otherwise be empty — no stray blank paragraph in output.
        assert!(slice.content.children.is_empty(),
            "unsafe image must not leave a stray empty paragraph: {:?}",
            slice.content.children);
    }

    #[test]
    fn md_unsafe_image_between_paragraphs_preserves_neighbors() {
        let slice = parse_from_markdown("before\n\n![](javascript:x)\n\nafter");
        // Two real paragraphs; the middle one (which held only the unsafe image) is dropped.
        assert_eq!(slice.content.children.len(), 2);
        assert_eq!(slice.content.children[0].text_content(), "before");
        assert_eq!(slice.content.children[1].text_content(), "after");
    }

    #[test]
    fn md_table_column_alignment() {
        let slice = parse_from_markdown(
            "| a | b | c | d |\n|:---|:---:|---:|---|\n| 1 | 2 | 3 | 4 |",
        );
        let table = find_by_type(&slice.content.children, NodeType::Table).unwrap();
        let header_row = table.child(0).unwrap();
        let expected = [Some("left"), Some("center"), Some("right"), None];
        for (i, want) in expected.iter().enumerate() {
            let cell = header_row.child(i).unwrap();
            assert_eq!(cell.node_type(), Some(NodeType::TableHeader));
            assert_eq!(
                cell.attrs().get("align").map(|s| s.as_str()),
                *want,
                "header cell {i}: expected align={:?}, got attrs {:?}",
                want,
                cell.attrs()
            );
        }
        let body_row = table.child(1).unwrap();
        for (i, want) in expected.iter().enumerate() {
            let cell = body_row.child(i).unwrap();
            assert_eq!(cell.node_type(), Some(NodeType::TableCell));
            assert_eq!(
                cell.attrs().get("align").map(|s| s.as_str()),
                *want,
                "body cell {i}: expected align={:?}",
                want,
            );
        }
    }

    #[test]
    fn md_table_alignment_markdown_to_html_roundtrip() {
        // Parse an aligned table from markdown, serialize back to HTML, and
        // assert that the alignment survives the full pipeline. A native
        // round-trip via the DOM parser isn't possible here (HTML import is
        // WASM-only), but this covers markdown-parse → HTML-emit, which is
        // the path clipboard copy takes.
        let slice = parse_from_markdown(
            "| a | b | c |\n|:---|:---:|---:|\n| 1 | 2 | 3 |",
        );
        let html = super::super::clipboard::serialize_to_html(&slice);
        assert!(html.contains("text-align: left"),
            "expected 'text-align: left' in HTML output: {html}");
        assert!(html.contains("text-align: center"),
            "expected 'text-align: center' in HTML output: {html}");
        assert!(html.contains("text-align: right"),
            "expected 'text-align: right' in HTML output: {html}");
    }

    #[test]
    fn md_table_alignment_shorter_than_columns() {
        // Malformed markdown: alignment row has fewer cells than the data
        // row. Cells beyond the alignments vec get no `align` attr rather
        // than panicking.
        let slice = parse_from_markdown(
            "| a | b | c |\n|:---|---:|\n| 1 | 2 | 3 |",
        );
        // Parser may reject this as invalid and fall back to paragraphs; we
        // only assert no panic and that no bogus `align` attr leaks.
        fn walk(n: &Node) {
            if let Node::Element { node_type, attrs, content, .. } = n {
                if matches!(node_type, NodeType::TableCell | NodeType::TableHeader) {
                    if let Some(a) = attrs.get("align") {
                        assert!(matches!(a.as_str(), "left" | "center" | "right"),
                            "unexpected align value: {a:?}");
                    }
                }
                for c in &content.children {
                    walk(c);
                }
            }
        }
        for c in &slice.content.children {
            walk(c);
        }
    }

    #[test]
    fn md_table_with_header() {
        let slice = parse_from_markdown("| a | b |\n|---|---|\n| 1 | 2 |");
        let table = find_by_type(&slice.content.children, NodeType::Table)
            .expect("expected Table");
        // First row = header
        let header_row = table.child(0).unwrap();
        assert_eq!(header_row.node_type(), Some(NodeType::TableRow));
        assert_eq!(header_row.child_count(), 2);
        assert_eq!(header_row.child(0).unwrap().node_type(), Some(NodeType::TableHeader));
        assert_eq!(header_row.child(0).unwrap().text_content(), "a");
        // Second row = body
        let body_row = table.child(1).unwrap();
        assert_eq!(body_row.node_type(), Some(NodeType::TableRow));
        assert_eq!(body_row.child(0).unwrap().node_type(), Some(NodeType::TableCell));
        assert_eq!(body_row.child(0).unwrap().text_content(), "1");
    }

    // ─── Inline marks ──────────────────────────────────────────────

    #[test]
    fn md_bold() {
        let slice = parse_from_markdown("**bold**");
        let text = text_with_mark(&slice.content.children, MarkType::Bold)
            .expect("expected text with Bold mark");
        assert_eq!(text, "bold");
    }

    #[test]
    fn md_italic() {
        let slice = parse_from_markdown("*italic*");
        let text = text_with_mark(&slice.content.children, MarkType::Italic)
            .expect("expected text with Italic mark");
        assert_eq!(text, "italic");
    }

    #[test]
    fn md_bold_italic_nested() {
        let slice = parse_from_markdown("***both***");
        // A single text run carries BOTH Bold and Italic marks.
        let both = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Bold)
            && marks.iter().any(|m| m.mark_type == MarkType::Italic)))
            .expect("expected text with both Bold and Italic marks");
        if let Node::Text { text, .. } = both {
            assert_eq!(text, "both");
        }
    }

    #[test]
    fn md_strikethrough() {
        let slice = parse_from_markdown("~~s~~");
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Strike).as_deref(), Some("s"));
    }

    #[test]
    fn md_inline_code() {
        let slice = parse_from_markdown("`code`");
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Code).as_deref(), Some("code"));
    }

    #[test]
    fn md_link_safe() {
        let slice = parse_from_markdown("[txt](https://example.com)");
        let link_text = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected text with Link mark");
        if let Node::Text { text, marks } = link_text {
            assert_eq!(text, "txt");
            let link = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(link.attrs.get("href").map(|s| s.as_str()), Some("https://example.com"));
        }
    }

    #[test]
    fn md_link_with_title() {
        let slice = parse_from_markdown(r#"[txt](https://x.com "my tip")"#);
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected Link mark");
        if let Node::Text { marks, .. } = link {
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://x.com"));
            assert_eq!(l.attrs.get("title").map(|s| s.as_str()), Some("my tip"));
        }
    }

    #[test]
    fn md_link_empty_title_not_emitted() {
        // `[txt](url "")` — title is empty; must NOT produce a title attr on
        // the mark, since that would serialize as `title=""` in HTML.
        let slice = parse_from_markdown(r#"[txt](https://x.com "")"#);
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected Link");
        if let Node::Text { marks, .. } = link {
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert!(l.attrs.get("title").is_none(),
                "empty title must not be stored as attr: {:?}", l.attrs);
        }
    }

    #[test]
    fn md_link_title_dropped_when_url_unsafe() {
        let slice = parse_from_markdown(r#"[txt](javascript:x "tip")"#);
        // No Link mark means no title leak either.
        assert!(text_with_mark(&slice.content.children, MarkType::Link).is_none());
        // Text still appears as plain content.
        let full = slice.content.children.iter().map(|n| n.text_content()).collect::<String>();
        assert!(full.contains("txt"));
    }

    #[test]
    fn md_link_javascript_url_strips_mark() {
        let slice = parse_from_markdown("[txt](javascript:alert(1))");
        // Text "txt" should still appear, but with NO Link mark.
        assert!(text_with_mark(&slice.content.children, MarkType::Link).is_none());
        let para = &slice.content.children[0];
        assert_eq!(para.text_content(), "txt");
    }

    #[test]
    fn md_hard_break() {
        let slice = parse_from_markdown("a  \nb");
        let para = &slice.content.children[0];
        assert_eq!(para.node_type(), Some(NodeType::Paragraph));
        // Paragraph must contain a HardBreak element between "a" and "b".
        let has_break = (0..para.child_count())
            .any(|i| para.child(i).unwrap().node_type() == Some(NodeType::HardBreak));
        assert!(has_break, "expected HardBreak inside paragraph");
        assert!(para.text_content().contains('a'));
        assert!(para.text_content().contains('b'));
    }

    #[test]
    fn md_mixed_inline() {
        let slice = parse_from_markdown("hello **bold** and *italic* and `code`");
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Bold).as_deref(), Some("bold"));
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Italic).as_deref(), Some("italic"));
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Code).as_deref(), Some("code"));
    }

    #[test]
    fn md_inline_html_bold() {
        let slice = parse_from_markdown("prefix <b>text</b> suffix");
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Bold).as_deref(), Some("text"));
        let full = slice.content.children[0].text_content();
        assert!(!full.contains('<'), "raw HTML must not appear: {full:?}");
        assert!(full.contains("prefix "));
        assert!(full.contains(" suffix"));
    }

    #[test]
    fn md_inline_html_strong_i_em_u_del_mark() {
        for (src, mt, txt) in [
            ("<strong>a</strong>", MarkType::Bold, "a"),
            ("<i>b</i>", MarkType::Italic, "b"),
            ("<em>c</em>", MarkType::Italic, "c"),
            ("<u>d</u>", MarkType::Underline, "d"),
            ("<del>e</del>", MarkType::Strike, "e"),
            ("<s>f</s>", MarkType::Strike, "f"),
            ("<mark>g</mark>", MarkType::Highlight, "g"),
            ("<sub>h</sub>", MarkType::Subscript, "h"),
            ("<sup>i</sup>", MarkType::Superscript, "i"),
        ] {
            let slice = parse_from_markdown(src);
            assert_eq!(text_with_mark(&slice.content.children, mt).as_deref(), Some(txt),
                "input {src:?} did not produce expected mark");
        }
    }

    #[test]
    fn md_inline_html_kbd_maps_to_code() {
        let slice = parse_from_markdown("press <kbd>Esc</kbd>");
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Code).as_deref(), Some("Esc"));
    }

    #[test]
    fn md_inline_html_br_variants_emit_hard_break() {
        for src in ["a<br>b", "a<br/>b", "a<br />b"] {
            let slice = parse_from_markdown(src);
            let para = &slice.content.children[0];
            let has_break = (0..para.child_count())
                .any(|i| para.child(i).unwrap().node_type() == Some(NodeType::HardBreak));
            assert!(has_break, "input {src:?}: expected HardBreak");
        }
    }

    #[test]
    fn md_inline_html_anchor_safe() {
        let slice = parse_from_markdown(r#"click <a href="https://x.com">here</a>"#);
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected Link mark");
        if let Node::Text { text, marks, .. } = link {
            assert_eq!(text, "here");
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://x.com"));
        }
    }

    #[test]
    fn md_inline_html_anchor_unsafe_url_strips_mark() {
        let slice = parse_from_markdown(r#"<a href="javascript:x">click</a>"#);
        assert!(text_with_mark(&slice.content.children, MarkType::Link).is_none());
        let full = slice.content.children[0].text_content();
        assert!(full.contains("click"));
    }

    #[test]
    fn md_inline_html_transparent_span() {
        let slice = parse_from_markdown("before <span>plain</span> after");
        let full = slice.content.children[0].text_content();
        assert!(!full.contains('<'));
        assert!(full.contains("plain"));
        assert!(text_with_mark(&slice.content.children, MarkType::Bold).is_none());
    }

    #[test]
    fn md_inline_html_unknown_tag_dropped() {
        let slice = parse_from_markdown("<foo>text</foo>");
        let full = slice.content.children[0].text_content();
        assert!(!full.contains('<'), "unknown tags must not leak angle brackets: {full:?}");
        assert!(full.contains("text"));
    }

    #[test]
    fn md_inline_html_nested_marks() {
        let slice = parse_from_markdown("<b>bold <i>both</i></b>");
        // "both" carries both Bold and Italic marks.
        let both = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, text, .. }
            if text == "both"
            && marks.iter().any(|m| m.mark_type == MarkType::Bold)
            && marks.iter().any(|m| m.mark_type == MarkType::Italic)))
            .expect("'both' must carry Bold + Italic");
        if let Node::Text { text, .. } = both {
            assert_eq!(text, "both");
        }
        // "bold " (outside <i>) carries only Bold.
        let outer = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, text, .. }
            if text == "bold "
            && marks.iter().any(|m| m.mark_type == MarkType::Bold)
            && !marks.iter().any(|m| m.mark_type == MarkType::Italic)))
            .expect("'bold ' must carry only Bold");
        let _ = outer;
    }

    #[test]
    fn md_inline_html_mismatched_close_tag() {
        // Malformed HTML: `<b>x</i>` — pulldown-cmark passes both through as
        // separate InlineHtml events. The close for a tag that was never
        // opened silently no-ops; the unclosed Bold persists until end of
        // paragraph. We just assert no panic and that text is preserved.
        let slice = parse_from_markdown("<b>x</i>");
        let full = slice.content.children[0].text_content();
        assert_eq!(full, "x");
        assert!(!full.contains('<'), "angle brackets must not leak: {full:?}");
    }

    #[test]
    fn md_inline_html_attr_single_quoted() {
        let slice = parse_from_markdown(r#"<a href='https://x.com'>link</a>"#);
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected Link from single-quoted href");
        if let Node::Text { marks, .. } = link {
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://x.com"));
        }
    }

    #[test]
    fn md_inline_html_attr_unquoted() {
        let slice = parse_from_markdown("<a href=https://x.com>link</a>");
        // Unquoted href should still parse if the URL has no whitespace.
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected Link from unquoted href");
        if let Node::Text { marks, .. } = link {
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://x.com"));
        }
    }

    #[test]
    fn md_inline_html_multiple_attrs() {
        let slice = parse_from_markdown(r#"<a href="https://x.com" title="my tip">link</a>"#);
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected Link");
        if let Node::Text { marks, .. } = link {
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://x.com"));
            assert_eq!(l.attrs.get("title").map(|s| s.as_str()), Some("my tip"));
        }
    }

    #[test]
    fn md_inline_html_uppercase_tag_and_attrs() {
        // Tag names and attribute names are ASCII-case-insensitive.
        let slice = parse_from_markdown(r#"<B>bold</B> and <A HREF="https://x.com">link</A>"#);
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Bold).as_deref(), Some("bold"));
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected Link from uppercase <A HREF>");
        if let Node::Text { marks, .. } = link {
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://x.com"));
        }
    }

    #[test]
    fn md_inline_html_sub_sup_stay_transparent() {
        let slice = parse_from_markdown("H<sub>2</sub>O and E=mc<sup>2</sup>");
        let full = slice.content.children[0].text_content();
        assert!(!full.contains('<'));
        assert!(full.contains("H2O"));
        assert!(full.contains("mc2"));
    }

    #[test]
    fn md_html_block_dropped() {
        // HTML blocks (script tags) are dropped entirely, not passed through.
        let slice = parse_from_markdown("<script>alert(1)</script>");
        let full_text = slice.content.children.iter()
            .map(|n| n.text_content())
            .collect::<String>();
        assert!(!full_text.contains("<script>"), "HTML block must not render: {full_text:?}");
        assert!(!full_text.contains("alert"));
    }

    // ─── Edge cases ────────────────────────────────────────────────

    #[test]
    fn md_empty_string_returns_empty_slice() {
        let slice = parse_from_markdown("");
        assert_eq!(slice.size(), 0);
        assert!(slice.content.children.is_empty());
    }

    #[test]
    fn md_only_whitespace_returns_empty_slice() {
        let slice = parse_from_markdown("   \n\n  ");
        assert!(slice.content.children.is_empty(),
            "whitespace-only input should produce no blocks");
    }

    #[test]
    fn md_autolink_bare_url() {
        let slice = parse_from_markdown("see https://example.com for more");
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected bare URL to become a Link");
        if let Node::Text { text, marks } = link {
            assert_eq!(text, "https://example.com");
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://example.com"));
        }
        // Surrounding plain text still present.
        let full = slice.content.children[0].text_content();
        assert!(full.starts_with("see "));
        assert!(full.ends_with(" for more"));
    }

    #[test]
    fn md_autolink_angle_form() {
        let slice = parse_from_markdown("<https://x.com>");
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Link).as_deref(),
            Some("https://x.com"));
    }

    #[test]
    fn md_autolink_http_scheme() {
        let slice = parse_from_markdown("check http://example.com now");
        let link_text = text_with_mark(&slice.content.children, MarkType::Link).unwrap();
        assert_eq!(link_text, "http://example.com");
    }

    #[test]
    fn md_autolink_multiple_urls_in_one_run() {
        let slice = parse_from_markdown("see https://a.com and https://b.com");
        // Walk all text nodes, collect the ones with Link marks.
        let mut hrefs: Vec<String> = Vec::new();
        fn walk(n: &Node, out: &mut Vec<String>) {
            if let Node::Text { marks, .. } = n {
                if let Some(link) = marks.iter().find(|m| m.mark_type == MarkType::Link) {
                    if let Some(h) = link.attrs.get("href") {
                        out.push(h.clone());
                    }
                }
            }
            if let Node::Element { content, .. } = n {
                for c in &content.children {
                    walk(c, out);
                }
            }
        }
        for c in &slice.content.children {
            walk(c, &mut hrefs);
        }
        assert_eq!(hrefs, vec!["https://a.com".to_string(), "https://b.com".to_string()]);
    }

    #[test]
    fn md_autolink_preserves_query_and_fragment() {
        let slice = parse_from_markdown("visit https://example.com/p?q=1&r=2#frag now");
        let link_text = text_with_mark(&slice.content.children, MarkType::Link).unwrap();
        assert_eq!(link_text, "https://example.com/p?q=1&r=2#frag");
    }

    #[test]
    fn md_autolink_not_linked_inside_code_block() {
        let slice = parse_from_markdown("```\nhttps://example.com\n```");
        let cb = &slice.content.children[0];
        assert_eq!(cb.node_type(), Some(NodeType::CodeBlock));
        assert!(text_with_mark(&slice.content.children, MarkType::Link).is_none(),
            "URL inside code block must not become a Link");
        assert!(cb.text_content().contains("https://example.com"));
    }

    #[test]
    fn md_autolink_with_utf8_prefix() {
        // Multi-byte UTF-8 before an ASCII URL: the scanner's byte offsets
        // must land on ASCII-only boundaries and not split a codepoint.
        let slice = parse_from_markdown("看看 https://example.com 谢谢");
        let link_text = text_with_mark(&slice.content.children, MarkType::Link).unwrap();
        assert_eq!(link_text, "https://example.com");
        let full = slice.content.children[0].text_content();
        assert!(full.starts_with("看看 "));
        assert!(full.ends_with(" 谢谢"));
    }

    #[test]
    fn md_autolink_respects_identifier_boundary() {
        // Underscore or hyphen immediately before the scheme is part of an
        // identifier, not a URL start — don't autolink.
        for src in ["foo_https://x.com", "bar-https://x.com"] {
            let slice = parse_from_markdown(src);
            assert!(
                text_with_mark(&slice.content.children, MarkType::Link).is_none(),
                "input {src:?} must not become a link"
            );
        }
    }

    #[test]
    fn md_autolink_strips_trailing_punctuation() {
        let slice = parse_from_markdown("see https://example.com.");
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected Link");
        if let Node::Text { text, marks, .. } = link {
            assert_eq!(text, "https://example.com");
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://example.com"));
        }
        // Trailing `.` preserved as plain text
        assert_eq!(slice.content.children[0].text_content(), "see https://example.com.");
    }

    #[test]
    fn md_autolink_balances_parens() {
        let slice = parse_from_markdown("(visit https://example.com)");
        let link_text = text_with_mark(&slice.content.children, MarkType::Link).unwrap();
        assert_eq!(link_text, "https://example.com");
    }

    #[test]
    fn md_autolink_skipped_inside_explicit_link() {
        // A markdown link with a URL as its text — the URL must NOT become a
        // nested autolink. Exactly one Link mark.
        let slice = parse_from_markdown("[go](https://target.com) and https://other.com");
        let mut link_count = 0;
        fn walk(n: &Node, count: &mut usize) {
            if let Node::Text { marks, .. } = n {
                if marks.iter().any(|m| m.mark_type == MarkType::Link) {
                    *count += 1;
                }
            }
            if let Node::Element { content, .. } = n {
                for c in &content.children {
                    walk(c, count);
                }
            }
        }
        for c in &slice.content.children {
            walk(c, &mut link_count);
        }
        assert_eq!(link_count, 2, "expected two link runs (explicit + autolink)");
    }

    #[test]
    fn md_autolink_skipped_inside_code_span() {
        let slice = parse_from_markdown("`https://example.com`");
        // Inline code with URL text stays just a Code mark; Link MUST NOT be added.
        assert!(text_with_mark(&slice.content.children, MarkType::Link).is_none());
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Code).as_deref(),
            Some("https://example.com"));
    }

    #[test]
    fn md_autolink_email() {
        let slice = parse_from_markdown("ping <bob@example.com>");
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected email autolink");
        if let Node::Text { marks, .. } = link {
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("mailto:bob@example.com"));
        }
    }

    #[test]
    fn is_trivial_slice_identifies_plain_paragraph_wrappers() {
        // `<p>text</p>` → Paragraph(Text) with no marks → trivial.
        let trivial = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("You can make text **bold**")]),
            )]),
            0, 0,
        );
        assert!(is_trivial_slice(&trivial));

        // Multiple plain paragraphs still trivial.
        let multi = Slice::new(
            Fragment::from(vec![
                Node::element_with_content(NodeType::Paragraph,
                    Fragment::from(vec![Node::text("**bold**")])),
                Node::element_with_content(NodeType::Paragraph,
                    Fragment::from(vec![Node::text("plain")])),
            ]),
            0, 0,
        );
        assert!(is_trivial_slice(&multi));
    }

    #[test]
    fn is_trivial_slice_rejects_hard_break_inside_paragraph() {
        // `<p>line 1<br>line 2</p>` — HTML with a real line break.
        // Switching to markdown on plain text "line 1\nline 2" would collapse
        // the break to a space (CommonMark soft break), losing information.
        // The predicate must refuse this slice so the HTML path is preserved.
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text("line 1"),
                    Node::element(NodeType::HardBreak),
                    Node::text("line 2"),
                ]),
            )]),
            0, 0,
        );
        assert!(!is_trivial_slice(&slice),
            "Paragraph containing a HardBreak must NOT be trivial");
    }

    #[test]
    fn is_trivial_slice_rejects_top_level_non_paragraph() {
        // Top-level HardBreak, HorizontalRule, or List — all non-trivial,
        // never match the "plain-text-in-paragraph-wrapper" shape.
        for nt in [NodeType::HardBreak, NodeType::HorizontalRule, NodeType::BulletList] {
            let slice = Slice::new(Fragment::from(vec![Node::element(nt)]), 0, 0);
            assert!(!is_trivial_slice(&slice),
                "top-level {nt:?} must NOT be trivial");
        }
    }

    #[test]
    fn is_trivial_slice_rejects_formatted_content() {
        // A bold mark makes it non-trivial — real HTML formatting present.
        let with_bold = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks("bold",
                    vec![Mark::new(MarkType::Bold)])]),
            )]),
            0, 0,
        );
        assert!(!is_trivial_slice(&with_bold));

        // A Heading is non-trivial (structural).
        let with_heading = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Heading,
                Fragment::from(vec![Node::text("Title")]),
            )]),
            0, 0,
        );
        assert!(!is_trivial_slice(&with_heading));

        // Empty slice is not trivial (nothing to replace with).
        assert!(!is_trivial_slice(&Slice::empty()));
    }

    #[test]
    fn md_sample_formatting_document_parses_correctly() {
        // The exact sample the user pasted — confirms the markdown path
        // produces the intended marks when invoked directly.
        let src = "You can make text **bold**, *italic*, or ***both***. You can also ~~strike it through~~ or mark it as `inline code`.\n\nCombine them: **bold with `code` inside** and *italic with a [link](https://example.com)*.\n\nHere's some text with a line break  \nthat continues on the next line.";
        let slice = parse_from_markdown(src);

        // Three paragraphs separated by blank lines.
        assert_eq!(slice.content.children.len(), 3, "expected 3 paragraphs");

        // All expected marks present somewhere in the output.
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Bold).as_deref(),
            Some("bold"), "first bold run");
        assert!(find(&slice.content.children, &|n| matches!(n, Node::Text { text, marks, .. }
            if text == "italic" && marks.iter().any(|m| m.mark_type == MarkType::Italic))).is_some(),
            "italic run missing");
        // "both" has Bold + Italic.
        assert!(find(&slice.content.children, &|n| matches!(n, Node::Text { text, marks, .. }
            if text == "both"
            && marks.iter().any(|m| m.mark_type == MarkType::Bold)
            && marks.iter().any(|m| m.mark_type == MarkType::Italic))).is_some(),
            "***both*** run missing Bold+Italic");
        assert!(find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Strike))).is_some(),
            "strike run missing");
        assert_eq!(text_with_mark(&slice.content.children, MarkType::Code).as_deref(),
            Some("inline code"), "inline code run");
        // Link inside italic
        assert!(find(&slice.content.children, &|n| matches!(n, Node::Text { text, marks, .. }
            if text == "link"
            && marks.iter().any(|m| m.mark_type == MarkType::Link)
            && marks.iter().any(|m| m.mark_type == MarkType::Italic))).is_some(),
            "link inside italic run missing");

        // Third paragraph has a HardBreak.
        let third = &slice.content.children[2];
        let has_hard_break = (0..third.child_count())
            .any(|i| third.child(i).unwrap().node_type() == Some(NodeType::HardBreak));
        assert!(has_hard_break, "expected HardBreak in third paragraph");
    }

    #[test]
    fn md_trailing_newline_single_paragraph() {
        let slice = parse_from_markdown("a\n");
        assert_eq!(slice.content.children.len(), 1);
        assert_eq!(slice.content.children[0].node_type(), Some(NodeType::Paragraph));
        assert_eq!(slice.content.children[0].text_content(), "a");
    }

    // ─── State-level integration tests ─────────────────────────────
    //
    // These mirror the paste-flow logic in `view.rs:721-886` (context detection,
    // `fit_slice_to_context`, block-level split vs inline replacement) so that
    // we verify markdown-derived slices land correctly in a live document.

    use super::super::clipboard::fit_slice_to_context;
    use super::super::selection::Selection;
    use super::super::state::{find_block_at, find_item_at, EditorState};

    fn paste_markdown(state: &EditorState, src: &str) -> EditorState {
        let slice = parse_from_markdown(src);
        if slice.content.children.is_empty() {
            return state.clone();
        }

        let pos = state.selection.from();
        let in_list = find_item_at(&state.doc, pos).is_some();
        let pasting_list = slice.content.children.iter().any(|n| matches!(
            n.node_type(),
            Some(NodeType::BulletList) | Some(NodeType::OrderedList) | Some(NodeType::TaskList)
        ));

        if in_list && pasting_list {
            let mut items = Vec::new();
            for node in &slice.content.children {
                if let Some(nt) = node.node_type() {
                    if matches!(nt, NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList) {
                        for j in 0..node.child_count() {
                            if let Some(item) = node.child(j) {
                                items.push(item.clone());
                            }
                        }
                    } else if matches!(nt, NodeType::ListItem | NodeType::TaskItem) {
                        items.push(node.clone());
                    }
                }
            }
            let item = find_item_at(&state.doc, pos).unwrap();
            let item_text = item.content.children.iter()
                .map(|c| c.text_content()).collect::<String>();
            let item_is_empty = item_text.trim().is_empty();
            let item_slice = Slice::new(Fragment::from(items), 0, 0);
            let txn = if item_is_empty {
                state.transaction()
                    .replace(item.offset, item.offset + item.node_size, item_slice).unwrap()
            } else {
                state.transaction()
                    .replace(item.offset, item.offset, item_slice).unwrap()
            };
            return state.apply(txn);
        }

        let has_blocks = slice.content.children.iter().any(|n| matches!(
            n.node_type(),
            Some(NodeType::Heading) | Some(NodeType::BulletList) | Some(NodeType::OrderedList)
                | Some(NodeType::TaskList) | Some(NodeType::Blockquote)
                | Some(NodeType::CodeBlock) | Some(NodeType::HorizontalRule)
                | Some(NodeType::Table) | Some(NodeType::Image)
        ));
        let parent_type = if has_blocks {
            NodeType::Doc
        } else {
            find_block_at(&state.doc, pos).map(|b| b.node_type).unwrap_or(NodeType::Doc)
        };
        let fitted = fit_slice_to_context(slice, parent_type);

        if has_blocks {
            if let Some(block) = find_block_at(&state.doc, pos) {
                let offset = pos.saturating_sub(block.content_start).min(block.content.size());
                let before_content = block.content.cut(0, offset);
                let after_content = block.content.cut(offset, block.content.size());
                let mut nodes = Vec::new();
                if !before_content.children.is_empty()
                    && before_content.children.iter().any(|n| !n.text_content().is_empty())
                {
                    nodes.push(Node::Element {
                        node_type: block.node_type,
                        attrs: block.attrs.clone(),
                        content: before_content,
                        marks: vec![],
                    });
                }
                nodes.extend(fitted.content.children);
                if !after_content.children.is_empty()
                    && after_content.children.iter().any(|n| !n.text_content().is_empty())
                {
                    nodes.push(Node::element_with_content(NodeType::Paragraph, after_content));
                }
                let block_slice = Slice::new(Fragment::from(nodes), 0, 0);
                let txn = state.transaction()
                    .replace(block.offset, block.offset + block.node_size, block_slice).unwrap();
                return state.apply(txn);
            }
        }

        let txn = state.transaction().replace_selection(fitted).unwrap();
        state.apply(txn)
    }

    fn empty_doc() -> EditorState {
        EditorState::empty()
    }

    fn doc_with_paragraph(text: &str, cursor: usize) -> EditorState {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text(text)]),
            )]),
        );
        EditorState {
            selection: Selection::cursor(cursor),
            ..EditorState::create_default(doc)
        }
    }

    #[test]
    fn paste_md_into_empty_paragraph_creates_heading() {
        let state = empty_doc();
        let new_state = paste_markdown(&state, "# hello");
        let first = new_state.doc.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Heading));
        assert_eq!(first.attrs().get("level").map(|s| s.as_str()), Some("1"));
        assert_eq!(first.text_content(), "hello");
    }

    #[test]
    fn paste_md_inline_into_middle_of_text() {
        // Doc "abcdef", cursor between "abc" and "def" (pos 4 = 1 + 3 chars)
        let state = doc_with_paragraph("abcdef", 4);
        let new_state = paste_markdown(&state, "**bold**");
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.node_type(), Some(NodeType::Paragraph));
        let full = para.text_content();
        assert_eq!(full, "abcbolddef");
        // The "bold" run must carry a Bold mark.
        let bold_text = text_with_mark(&[para.clone()], MarkType::Bold)
            .expect("expected Bold-marked text after paste");
        assert_eq!(bold_text, "bold");
    }

    #[test]
    fn paste_md_list_into_list_item() {
        // Doc: BulletList containing one item with "a", cursor inside that item
        let item_para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("a")]),
        );
        let item = Node::element_with_content(NodeType::ListItem, Fragment::from(vec![item_para]));
        let list = Node::element_with_content(NodeType::BulletList, Fragment::from(vec![item]));
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![list]));
        // Position inside the single item's paragraph text ("a" has 1 char).
        // Doc positions: 0=before list, 1=inside list before item, 2=inside item before para, 3=inside para before "a", 4=after "a".
        let state = EditorState {
            selection: Selection::cursor(4),
            ..EditorState::create_default(doc)
        };
        let new_state = paste_markdown(&state, "- x\n- y");
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        // Original "a" item + inserted "x" + "y" = at least 2 new items in addition to original.
        assert!(list.child_count() >= 3,
            "expected at least 3 items after paste, got {}", list.child_count());
        let texts: Vec<String> = (0..list.child_count())
            .map(|i| list.child(i).unwrap().text_content())
            .collect();
        assert!(texts.iter().any(|t| t == "x"), "missing 'x' item: {:?}", texts);
        assert!(texts.iter().any(|t| t == "y"), "missing 'y' item: {:?}", texts);
        assert!(texts.iter().any(|t| t == "a"), "original 'a' item lost: {:?}", texts);
    }

    /// Returns the (transaction, new_state) pair so callers can feed the
    /// transaction into HistoryPlugin and exercise undo.
    fn paste_markdown_txn(
        state: &EditorState,
        src: &str,
    ) -> Option<(super::super::state::Transaction, EditorState)> {
        let slice = parse_from_markdown(src);
        if slice.content.children.is_empty() {
            return None;
        }
        let pos = state.selection.from();
        let in_list = find_item_at(&state.doc, pos).is_some();
        let pasting_list = slice.content.children.iter().any(|n| matches!(
            n.node_type(),
            Some(NodeType::BulletList) | Some(NodeType::OrderedList) | Some(NodeType::TaskList)
        ));
        let txn = if in_list && pasting_list {
            let mut items = Vec::new();
            for node in &slice.content.children {
                if let Some(nt) = node.node_type() {
                    if matches!(nt, NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList) {
                        for j in 0..node.child_count() {
                            if let Some(item) = node.child(j) {
                                items.push(item.clone());
                            }
                        }
                    } else if matches!(nt, NodeType::ListItem | NodeType::TaskItem) {
                        items.push(node.clone());
                    }
                }
            }
            let item = find_item_at(&state.doc, pos).unwrap();
            let item_text = item.content.children.iter()
                .map(|c| c.text_content()).collect::<String>();
            let item_is_empty = item_text.trim().is_empty();
            let item_slice = Slice::new(Fragment::from(items), 0, 0);
            if item_is_empty {
                state.transaction().replace(item.offset, item.offset + item.node_size, item_slice).unwrap()
            } else {
                state.transaction().replace(item.offset, item.offset, item_slice).unwrap()
            }
        } else {
            let has_blocks = slice.content.children.iter().any(|n| matches!(
                n.node_type(),
                Some(NodeType::Heading) | Some(NodeType::BulletList) | Some(NodeType::OrderedList)
                    | Some(NodeType::TaskList) | Some(NodeType::Blockquote)
                    | Some(NodeType::CodeBlock) | Some(NodeType::HorizontalRule)
                    | Some(NodeType::Table) | Some(NodeType::Image)
            ));
            let parent_type = if has_blocks {
                NodeType::Doc
            } else {
                find_block_at(&state.doc, pos).map(|b| b.node_type).unwrap_or(NodeType::Doc)
            };
            let fitted = fit_slice_to_context(slice, parent_type);
            // Mirror view.rs: when has_blocks but no enclosing block is
            // found (cursor outside any block in a malformed doc), fall
            // through to replace_selection rather than panicking.
            let block_branch = has_blocks.then(|| find_block_at(&state.doc, pos)).flatten();
            if let Some(block) = block_branch {
                let offset = pos.saturating_sub(block.content_start).min(block.content.size());
                let before_content = block.content.cut(0, offset);
                let after_content = block.content.cut(offset, block.content.size());
                let mut nodes = Vec::new();
                if !before_content.children.is_empty()
                    && before_content.children.iter().any(|n| !n.text_content().is_empty())
                {
                    nodes.push(Node::Element {
                        node_type: block.node_type,
                        attrs: block.attrs.clone(),
                        content: before_content,
                        marks: vec![],
                    });
                }
                nodes.extend(fitted.content.children);
                if !after_content.children.is_empty()
                    && after_content.children.iter().any(|n| !n.text_content().is_empty())
                {
                    nodes.push(Node::element_with_content(NodeType::Paragraph, after_content));
                }
                let block_slice = Slice::new(Fragment::from(nodes), 0, 0);
                state.transaction()
                    .replace(block.offset, block.offset + block.node_size, block_slice).unwrap()
            } else {
                state.transaction().replace_selection(fitted).unwrap()
            }
        };
        let new_state = state.apply(txn.clone());
        Some((txn, new_state))
    }

    #[test]
    fn undo_after_markdown_paste_into_empty_doc() {
        use super::super::plugins::HistoryPlugin;
        let state = empty_doc();
        let original_doc = state.doc.clone();

        let (txn, new_state) = paste_markdown_txn(&state, "# heading\n\nparagraph").unwrap();
        assert_ne!(new_state.doc, original_doc, "paste should change the doc");

        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);
        assert!(history.can_undo(), "paste transaction should be recorded");

        let undo_txn = history.undo(&new_state).expect("undo should produce a transaction");
        let restored = new_state.apply(undo_txn);
        assert_eq!(restored.doc, original_doc,
            "undo after paste must restore the original doc");
    }

    #[test]
    fn undo_after_markdown_paste_inline() {
        use super::super::plugins::HistoryPlugin;
        let state = doc_with_paragraph("abcdef", 4);
        let original_doc = state.doc.clone();

        let (txn, new_state) = paste_markdown_txn(&state, "**bold**").unwrap();
        assert_ne!(new_state.doc, original_doc);

        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);
        let undo_txn = history.undo(&new_state).expect("undo must work for inline paste");
        let restored = new_state.apply(undo_txn);
        assert_eq!(restored.doc, original_doc,
            "inline paste undo must restore original text");
    }

    #[test]
    fn undo_after_markdown_paste_block_split() {
        use super::super::plugins::HistoryPlugin;
        // Paste a Heading into the middle of a paragraph; this is the
        // block-level split branch which builds a multi-node replacement.
        let state = doc_with_paragraph("before after", 7); // cursor between "before " and "after"
        let original_doc = state.doc.clone();

        let (txn, new_state) = paste_markdown_txn(&state, "# heading").unwrap();
        assert_ne!(new_state.doc, original_doc);

        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);
        let undo_txn = history.undo(&new_state).expect("undo must work for block-split paste");
        let restored = new_state.apply(undo_txn);
        assert_eq!(restored.doc, original_doc,
            "block-split paste undo must restore original doc");
    }

    #[test]
    fn undo_after_markdown_paste_table() {
        use super::super::plugins::HistoryPlugin;
        let state = empty_doc();
        let original_doc = state.doc.clone();

        let (txn, new_state) = paste_markdown_txn(
            &state, "| a | b |\n|:--|--:|\n| 1 | 2 |"
        ).unwrap();
        assert_ne!(new_state.doc, original_doc);

        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);
        let undo_txn = history.undo(&new_state).expect("undo must work for table paste");
        let restored = new_state.apply(undo_txn);
        assert_eq!(restored.doc, original_doc,
            "undo after table paste must restore original doc");
    }

    #[test]
    fn undo_after_markdown_paste_code_block() {
        use super::super::plugins::HistoryPlugin;
        let state = empty_doc();
        let original_doc = state.doc.clone();

        let (txn, new_state) = paste_markdown_txn(
            &state, "```rust\nfn main() {}\n```"
        ).unwrap();
        assert_ne!(new_state.doc, original_doc);

        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);
        let undo_txn = history.undo(&new_state).expect("undo must work for code-block paste");
        let restored = new_state.apply(undo_txn);
        assert_eq!(restored.doc, original_doc,
            "undo after code-block paste must restore original doc");
    }

    #[test]
    fn undo_after_markdown_paste_into_nonempty_list() {
        use super::super::plugins::HistoryPlugin;
        // BulletList with one non-empty item; paste a list into the middle.
        // Exercises the in_list && pasting_list branch with item_is_empty=false.
        let item_para = Node::element_with_content(
            NodeType::Paragraph, Fragment::from(vec![Node::text("first")]));
        let item = Node::element_with_content(NodeType::ListItem, Fragment::from(vec![item_para]));
        let list = Node::element_with_content(NodeType::BulletList, Fragment::from(vec![item]));
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![list]));
        let state = EditorState {
            selection: Selection::cursor(4),
            ..EditorState::create_default(doc.clone())
        };
        let original_doc = state.doc.clone();

        let (txn, new_state) = paste_markdown_txn(&state, "- two\n- three").unwrap();
        assert_ne!(new_state.doc, original_doc);

        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);
        let undo_txn = history.undo(&new_state)
            .expect("undo must work for list-into-nonempty-list paste");
        let restored = new_state.apply(undo_txn);
        assert_eq!(restored.doc, original_doc,
            "undo after list-into-list paste must restore original doc");
    }

    #[test]
    fn undo_redo_round_trip_after_markdown_paste() {
        use super::super::plugins::HistoryPlugin;
        let state = doc_with_paragraph("abcdef", 4);
        let original_doc = state.doc.clone();

        let (txn, new_state) = paste_markdown_txn(&state, "**bold**").unwrap();
        let after_paste_doc = new_state.doc.clone();

        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);

        let undo_txn = history.undo(&new_state).expect("undo");
        let after_undo = new_state.apply(undo_txn);
        assert_eq!(after_undo.doc, original_doc, "undo restores original");
        assert!(history.can_redo(), "redo must be available after undo");

        let redo_txn = history.redo(&after_undo).expect("redo");
        let after_redo = after_undo.apply(redo_txn);
        assert_eq!(after_redo.doc, after_paste_doc,
            "redo must restore the post-paste doc byte-for-byte");
    }

    #[test]
    fn paste_md_inline_fits_heading_context() {
        // Heading "Title" (5 chars), cursor at end (pos 6)
        let mut attrs = HashMap::new();
        attrs.insert("level".to_string(), "1".to_string());
        let heading = Node::element_with_attrs(
            NodeType::Heading, attrs,
            Fragment::from(vec![Node::text("Title")]),
        );
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![heading]));
        let state = EditorState {
            selection: Selection::cursor(6),
            ..EditorState::create_default(doc)
        };
        let new_state = paste_markdown(&state, "more");
        let h = new_state.doc.child(0).unwrap();
        assert_eq!(h.node_type(), Some(NodeType::Heading));
        assert_eq!(h.text_content(), "Titlemore");
    }

    // ─── Coverage gaps (added 2026-04-24) ──────────────────────────

    #[test]
    fn md_normalizes_crlf_line_endings() {
        // Windows-style line endings should produce the same structure as
        // Unix-style — a heading followed by a paragraph.
        let slice = parse_from_markdown("# Title\r\n\r\nBody text");
        assert_eq!(slice.content.children.len(), 2);
        let h = &slice.content.children[0];
        assert_eq!(h.node_type(), Some(NodeType::Heading));
        assert_eq!(h.text_content(), "Title");
        let p = &slice.content.children[1];
        assert_eq!(p.node_type(), Some(NodeType::Paragraph));
        assert_eq!(p.text_content(), "Body text");
        // No stray \r anywhere.
        fn walk_for_cr(n: &Node) -> bool {
            if n.text_content().contains('\r') { return true; }
            if let Node::Element { content, .. } = n {
                return content.children.iter().any(walk_for_cr);
            }
            false
        }
        assert!(!slice.content.children.iter().any(walk_for_cr),
            "no carriage returns may survive normalization");
    }

    #[test]
    fn md_setext_heading_level_1() {
        let slice = parse_from_markdown("Title\n=====\n");
        assert_eq!(slice.content.children.len(), 1);
        let h = &slice.content.children[0];
        assert_eq!(h.node_type(), Some(NodeType::Heading));
        assert_eq!(h.attrs().get("level").map(|s| s.as_str()), Some("1"));
        assert_eq!(h.text_content(), "Title");
    }

    #[test]
    fn md_setext_heading_level_2() {
        let slice = parse_from_markdown("Title\n-----\n");
        let h = &slice.content.children[0];
        assert_eq!(h.node_type(), Some(NodeType::Heading));
        assert_eq!(h.attrs().get("level").map(|s| s.as_str()), Some("2"));
        assert_eq!(h.text_content(), "Title");
    }

    #[test]
    fn md_loose_list_emits_paragraph_children() {
        // Loose list (blank line between items) — pulldown-cmark emits
        // explicit Start(Paragraph) events inside each Item, exercising the
        // non-tight branch where we don't synthesize a Paragraph.
        let slice = parse_from_markdown("- a\n\n- b");
        let list = &slice.content.children[0];
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.child_count(), 2);
        for i in 0..2 {
            let item = list.child(i).unwrap();
            assert_eq!(item.node_type(), Some(NodeType::ListItem));
            // Loose-list item must contain exactly one Paragraph child holding text.
            assert_eq!(item.child_count(), 1, "loose item {i} child count");
            let para = item.child(0).unwrap();
            assert_eq!(para.node_type(), Some(NodeType::Paragraph));
        }
        assert_eq!(list.child(0).unwrap().text_content(), "a");
        assert_eq!(list.child(1).unwrap().text_content(), "b");
    }

    #[test]
    fn md_reference_style_link() {
        let slice = parse_from_markdown("[txt][r]\n\n[r]: https://example.com");
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected reference link to resolve to Link mark");
        if let Node::Text { text, marks, .. } = link {
            assert_eq!(text, "txt");
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://example.com"));
        }
    }

    #[test]
    fn md_code_block_tilde_fence() {
        let slice = parse_from_markdown("~~~rust\nfn main() {}\n~~~");
        let cb = &slice.content.children[0];
        assert_eq!(cb.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(cb.attrs().get("language").map(|s| s.as_str()), Some("rust"));
        assert_eq!(cb.text_content(), "fn main() {}\n");
    }

    #[test]
    fn md_empty_heading() {
        // `# ` with nothing after the marker. Behaviour: produce a Heading
        // with empty content. (CommonMark allows empty ATX headings.)
        let slice = parse_from_markdown("# \n\nnext");
        // First child must be a Heading (possibly empty), second a Paragraph.
        let h = &slice.content.children[0];
        assert_eq!(h.node_type(), Some(NodeType::Heading));
        assert_eq!(h.text_content(), "");
        let p = &slice.content.children[1];
        assert_eq!(p.node_type(), Some(NodeType::Paragraph));
        assert_eq!(p.text_content(), "next");
    }

    #[test]
    fn md_inline_html_anchor_with_extra_attrs_uses_href() {
        let slice = parse_from_markdown(
            r#"<a href="https://x.com" target="_blank" rel="noopener">link</a>"#,
        );
        let link = find(&slice.content.children, &|n| matches!(n, Node::Text { marks, .. }
            if marks.iter().any(|m| m.mark_type == MarkType::Link)))
            .expect("expected Link mark");
        if let Node::Text { marks, .. } = link {
            let l = marks.iter().find(|m| m.mark_type == MarkType::Link).unwrap();
            assert_eq!(l.attrs.get("href").map(|s| s.as_str()), Some("https://x.com"));
            // target/rel are not part of our Link mark schema; must NOT leak.
            assert!(l.attrs.get("target").is_none());
            assert!(l.attrs.get("rel").is_none());
        }
    }

    #[test]
    fn md_image_inside_link() {
        // `[![alt](img.png)](https://example.com)` — Image nested inside a
        // markdown Link. Pulldown-cmark emits Link wrapping Image events.
        let slice = parse_from_markdown(
            "[![alt](https://example.com/img.png)](https://example.com)",
        );
        let img = find_by_type(&slice.content.children, NodeType::Image)
            .expect("Image must be emitted even when inside a Link");
        assert_eq!(img.attrs().get("src").map(|s| s.as_str()),
            Some("https://example.com/img.png"));
        assert_eq!(img.attrs().get("alt").map(|s| s.as_str()), Some("alt"));
    }

    #[test]
    fn md_image_alt_strips_emphasis_to_plain_text() {
        // `![*alt*](url)` — Emphasis events inside an Image are absorbed
        // into the alt text via text_content(); marks are not preserved on
        // the alt attribute.
        let slice = parse_from_markdown("![*alt*](https://example.com/x.png)");
        let img = find_by_type(&slice.content.children, NodeType::Image)
            .expect("expected Image");
        assert_eq!(img.attrs().get("alt").map(|s| s.as_str()), Some("alt"),
            "emphasis inside image alt must collapse to plain text");
    }

    #[test]
    fn md_ordered_list_start_number_dropped() {
        // Schema has no `start` attr on OrderedList; numbering always starts
        // at 1 in render. Confirm the source's start number doesn't leak.
        let slice = parse_from_markdown("5. foo\n6. bar");
        let list = &slice.content.children[0];
        assert_eq!(list.node_type(), Some(NodeType::OrderedList));
        assert!(list.attrs().get("start").is_none(),
            "OrderedList must not carry a start attr: {:?}", list.attrs());
        assert_eq!(list.child_count(), 2);
    }

    #[test]
    fn md_hard_break_at_paragraph_end_followed_by_new_paragraph() {
        // Trailing hard break in a paragraph immediately followed by a blank
        // line and a new paragraph. The hard break should NOT bleed into the
        // next paragraph; both paragraphs render independently.
        let slice = parse_from_markdown("text  \n\nnext");
        assert_eq!(slice.content.children.len(), 2,
            "expected exactly two paragraphs");
        let first = &slice.content.children[0];
        assert_eq!(first.node_type(), Some(NodeType::Paragraph));
        assert!(first.text_content().contains("text"));
        let second = &slice.content.children[1];
        assert_eq!(second.node_type(), Some(NodeType::Paragraph));
        assert_eq!(second.text_content(), "next");
        // The second paragraph must NOT contain a HardBreak — that would
        // mean the trailing break leaked across the boundary.
        let second_has_break = (0..second.child_count())
            .any(|i| second.child(i).unwrap().node_type() == Some(NodeType::HardBreak));
        assert!(!second_has_break, "HardBreak must not leak into following paragraph");
    }

    #[test]
    fn md_html_block_discard_keeps_neighboring_paragraphs() {
        // The discard-frame mechanism (now Frame.discard, formerly an
        // `_drop` attr) must drop only the HTML block, not its neighbors.
        let slice = parse_from_markdown("first\n\n<div>raw html</div>\n\nlast");
        let texts: Vec<String> = slice.content.children.iter()
            .map(|n| n.text_content()).collect();
        assert!(texts.iter().any(|t| t == "first"), "first paragraph survives");
        assert!(texts.iter().any(|t| t == "last"), "last paragraph survives");
        assert!(!texts.iter().any(|t| t.contains("<div>")),
            "raw HTML must not leak: {texts:?}");
    }

    #[test]
    fn parse_html_tag_recognizes_self_close_marker() {
        // Direct test of the HtmlTag parser — the self_close field must be
        // set for `<br/>` and `<br />` but not for plain `<br>`.
        let plain = parse_html_tag("<br>").expect("must parse");
        assert!(!plain.self_close);
        assert!(!plain.is_close);

        let slash = parse_html_tag("<br/>").expect("must parse");
        assert!(slash.self_close);
        assert!(!slash.is_close);

        let spaced = parse_html_tag("<br />").expect("must parse");
        assert!(spaced.self_close);
        assert!(!spaced.is_close);

        let close = parse_html_tag("</br>").expect("must parse");
        assert!(close.is_close);
        assert!(!close.self_close, "close tags are never self-closing");
    }

    #[test]
    fn parse_html_tag_attr_lookup_is_case_insensitive() {
        let tag = parse_html_tag(r#"<a HREF="https://x.com" Title="t">"#).unwrap();
        assert_eq!(tag.attr("href"), Some("https://x.com"));
        assert_eq!(tag.attr("HREF"), Some("https://x.com"));
        assert_eq!(tag.attr("title"), Some("t"));
        assert_eq!(tag.attr("missing"), None);
    }

    #[test]
    fn is_trivial_slice_accepts_whitespace_only_text_in_paragraphs() {
        // `<p>\n   text\n</p>` — Text node carries surrounding whitespace
        // but no marks. Still trivial: source likely emitted plain text and
        // the markdown re-parse will normalize whitespace per CommonMark.
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("\n   text\n")]),
            )]),
            0, 0,
        );
        assert!(is_trivial_slice(&slice));
    }
}
