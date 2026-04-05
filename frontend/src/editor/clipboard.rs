use super::model::{Fragment, Mark, MarkType, Node, NodeType, Slice};

/// Serialize a document slice to HTML for clipboard copy.
pub fn serialize_to_html(slice: &Slice) -> String {
    let mut html = String::new();
    for child in &slice.content.children {
        render_node_html(child, &mut html);
    }
    html
}

/// Serialize a document slice to plain text for clipboard copy.
pub fn serialize_to_text(slice: &Slice) -> String {
    let mut text = String::new();
    for child in &slice.content.children {
        collect_text(child, &mut text);
    }
    text
}

/// Parse HTML string into a document Slice.
/// In WASM, uses the browser's DomParser for correct HTML handling.
/// In non-WASM tests, falls back to tag stripping.
pub fn parse_from_html(html: &str) -> Slice {
    #[cfg(target_arch = "wasm32")]
    {
        parse_from_html_dom(html)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let text = strip_tags(html);
        if text.is_empty() {
            return Slice::empty();
        }
        Slice::new(Fragment::from(vec![Node::text(&text)]), 0, 0)
    }
}

/// Parse HTML using the browser's DomParser (WASM only).
#[cfg(target_arch = "wasm32")]
fn parse_from_html_dom(html: &str) -> Slice {
    use wasm_bindgen::JsCast;

    let parser = match web_sys::DomParser::new() {
        Ok(p) => p,
        Err(_) => return Slice::empty(),
    };
    let doc = match parser.parse_from_string(html, web_sys::SupportedType::TextHtml) {
        Ok(d) => d,
        Err(_) => return Slice::empty(),
    };
    let Some(body) = doc.body() else {
        return Slice::empty();
    };

    let mut nodes = Vec::new();
    walk_dom_children(&body, &[], &mut nodes, false);

    if nodes.is_empty() {
        return Slice::empty();
    }
    Slice::new(Fragment::from(nodes), 0, 0)
}

/// Recursively walk DOM children and build model nodes.
#[cfg(target_arch = "wasm32")]
fn walk_dom_children(
    parent: &web_sys::Node,
    active_marks: &[Mark],
    out: &mut Vec<Node>,
    inline_context: bool,
) {
    use wasm_bindgen::JsCast;

    let children = parent.child_nodes();
    for i in 0..children.length() {
        let Some(child) = children.item(i) else { continue };

        match child.node_type() {
            web_sys::Node::TEXT_NODE => {
                convert_text_node(&child, active_marks, out, inline_context);
            }
            web_sys::Node::ELEMENT_NODE => {
                let Some(el) = child.dyn_ref::<web_sys::Element>() else { continue };
                convert_element_node(&child, el, active_marks, out, inline_context);
            }
            _ => {} // Skip comments, processing instructions, etc.
        }
    }
}

/// Convert a DOM text node into a model text node with active marks.
#[cfg(target_arch = "wasm32")]
fn convert_text_node(
    child: &web_sys::Node,
    active_marks: &[Mark],
    out: &mut Vec<Node>,
    inline_context: bool,
) {
    let text = child.text_content().unwrap_or_default();

    // Skip whitespace-only text nodes (common in web page HTML indentation)
    // but keep spaces inside inline context (they're meaningful between words)
    if text.is_empty() || (!inline_context && text.trim().is_empty()) {
        return;
    }

    let text = if inline_context { text } else { text.trim().to_string() };
    if text.is_empty() {
        return;
    }

    if active_marks.is_empty() {
        out.push(Node::text(&text));
    } else {
        out.push(Node::text_with_marks(&text, active_marks.to_vec()));
    }
}

/// Convert a DOM element node into model nodes, dispatching by tag.
#[cfg(target_arch = "wasm32")]
fn convert_element_node(
    child: &web_sys::Node,
    el: &web_sys::Element,
    active_marks: &[Mark],
    out: &mut Vec<Node>,
    inline_context: bool,
) {
    let tag = el.tag_name().to_lowercase();

    // Security: skip dangerous elements entirely
    if matches!(tag.as_str(), "script" | "style" | "iframe" | "object" | "embed" | "link" | "meta") {
        return;
    }

    // Inline formatting element → push mark and recurse
    if let Some(mark) = tag_to_mark(&tag, el) {
        let mut new_marks = active_marks.to_vec();
        new_marks.push(mark);
        walk_dom_children(child, &new_marks, out, inline_context);
        return;
    }

    // Known block element → convert to model node
    if let Some(node_type) = tag_to_block_type(&tag) {
        convert_block_element(child, el, &tag, node_type, out);
        return;
    }

    // Unknown block-level element (div, section, article, etc.)
    if is_block_level_tag(&tag) {
        convert_unknown_block(child, out);
        return;
    }

    // Unknown inline element (span, font, etc.): transparent, preserve marks
    walk_dom_children(child, active_marks, out, inline_context);
}

/// Walk a DOM node's children into a fresh Vec of model nodes.
#[cfg(target_arch = "wasm32")]
fn walk_block_children(child: &web_sys::Node, inline_context: bool) -> Vec<Node> {
    let mut nodes = Vec::new();
    walk_dom_children(child, &[], &mut nodes, inline_context);
    nodes
}

/// Convert a known block-type DOM element into the corresponding model node.
#[cfg(target_arch = "wasm32")]
fn convert_block_element(
    child: &web_sys::Node,
    el: &web_sys::Element,
    tag: &str,
    node_type: NodeType,
    out: &mut Vec<Node>,
) {
    match node_type {
        NodeType::Heading => {
            let level = match tag {
                "h1" => "1", "h2" => "2", "h3" => "3",
                "h4" => "4", "h5" => "5", "h6" => "6",
                _ => "1",
            };
            let mut attrs = std::collections::HashMap::new();
            attrs.insert("level".to_string(), level.to_string());
            let children = walk_block_children(child, true);
            out.push(Node::element_with_attrs(
                NodeType::Heading, attrs, Fragment::from(children),
            ));
        }
        NodeType::CodeBlock => out.push(convert_code_block(child, el)),
        NodeType::HorizontalRule => out.push(Node::element(NodeType::HorizontalRule)),
        NodeType::HardBreak => out.push(Node::element(NodeType::HardBreak)),
        NodeType::Image => { if let Some(n) = convert_image(el) { out.push(n); } }
        NodeType::ListItem => {
            out.push(convert_list_item(walk_block_children(child, false)));
        }
        NodeType::BulletList | NodeType::OrderedList => {
            let children = walk_block_children(child, false);
            out.push(Node::element_with_content(node_type, Fragment::from(children)));
        }
        NodeType::Table => {
            // Walk children; thead/tbody/tfoot are transparent (handled by is_block_level_tag)
            let children = walk_block_children(child, false);
            if !children.is_empty() {
                out.push(Node::element_with_content(NodeType::Table, Fragment::from(children)));
            }
        }
        NodeType::TableRow => {
            let children = walk_block_children(child, false);
            if !children.is_empty() {
                out.push(Node::element_with_content(NodeType::TableRow, Fragment::from(children)));
            }
        }
        NodeType::TableCell | NodeType::TableHeader => {
            let children = walk_block_children(child, false);
            // If all children are inline, wrap in Paragraph (same as list items)
            let children = if children.iter().all(|n| matches!(n, Node::Text { .. }) || n.is_leaf()) {
                vec![Node::element_with_content(NodeType::Paragraph, Fragment::from(children))]
            } else {
                children
            };
            let mut attrs = std::collections::HashMap::new();
            if let Some(cs) = el.get_attribute("colspan") {
                attrs.insert("colspan".into(), cs);
            }
            if let Some(rs) = el.get_attribute("rowspan") {
                attrs.insert("rowspan".into(), rs);
            }
            out.push(Node::element_with_attrs(node_type, attrs, Fragment::from(children)));
        }
        _ => {
            // Paragraph, Blockquote, etc.
            let children = walk_block_children(child, true);
            out.push(Node::element_with_content(node_type, Fragment::from(children)));
        }
    }
}

/// Parse a <pre> element into a CodeBlock node, extracting language from <code> child.
#[cfg(target_arch = "wasm32")]
fn convert_code_block(child: &web_sys::Node, el: &web_sys::Element) -> Node {
    use wasm_bindgen::JsCast;

    let code_el = el.query_selector("code").ok().flatten();
    let text_source = code_el.as_ref()
        .map(|c| c.dyn_ref::<web_sys::Node>().unwrap())
        .unwrap_or(child);

    let mut attrs = std::collections::HashMap::new();
    if let Some(code) = &code_el {
        if let Some(lang) = code.class_name().strip_prefix("language-") {
            attrs.insert("language".to_string(), lang.to_string());
        }
    }

    let text = text_source.text_content().unwrap_or_default();
    let content = if text.is_empty() {
        Fragment::empty()
    } else {
        Fragment::from(vec![Node::text(&text)])
    };
    Node::element_with_attrs(NodeType::CodeBlock, attrs, content)
}

/// Parse an <img> element into an Image node, validating the URL.
#[cfg(target_arch = "wasm32")]
fn convert_image(el: &web_sys::Element) -> Option<Node> {
    let src = el.get_attribute("src").unwrap_or_default();
    if !super::view::is_safe_url(&src) {
        return None;
    }
    let mut attrs = std::collections::HashMap::new();
    attrs.insert("src".to_string(), src);
    if let Some(alt) = el.get_attribute("alt") {
        attrs.insert("alt".to_string(), alt);
    }
    Some(Node::element_with_attrs(NodeType::Image, attrs, Fragment::empty()))
}

/// Convert a <li> element's children into a ListItem node.
/// If all children are inline, wraps them in a Paragraph first.
#[cfg(target_arch = "wasm32")]
fn convert_list_item(children: Vec<Node>) -> Node {
    let all_inline = children.iter().all(|n| matches!(n, Node::Text { .. }) || n.is_leaf());
    if all_inline {
        let para = Node::element_with_content(NodeType::Paragraph, Fragment::from(children));
        Node::element_with_content(NodeType::ListItem, Fragment::from(vec![para]))
    } else {
        Node::element_with_content(NodeType::ListItem, Fragment::from(children))
    }
}

/// Convert an unknown block-level element (div, section, etc.).
/// Wraps inline-only content in a Paragraph; passes through block children transparently.
#[cfg(target_arch = "wasm32")]
fn convert_unknown_block(child: &web_sys::Node, out: &mut Vec<Node>) {
    let children = walk_block_children(child, false);

    if children.is_empty() {
        return;
    }

    let all_inline = children.iter().all(|n| matches!(n, Node::Text { .. }) || n.is_leaf());
    if all_inline {
        out.push(Node::element_with_content(
            NodeType::Paragraph, Fragment::from(children),
        ));
    } else {
        out.extend(children);
    }
}

/// Map an HTML tag to a mark type.
#[cfg(target_arch = "wasm32")]
fn tag_to_mark(tag: &str, el: &web_sys::Element) -> Option<Mark> {
    match tag {
        "strong" | "b" => Some(Mark::new(MarkType::Bold)),
        "em" | "i" => Some(Mark::new(MarkType::Italic)),
        "u" => Some(Mark::new(MarkType::Underline)),
        "s" | "del" | "strike" => Some(Mark::new(MarkType::Strike)),
        "code" => Some(Mark::new(MarkType::Code)),
        "a" => {
            let href = el.get_attribute("href").unwrap_or_default();
            if super::view::is_safe_url(&href) {
                Some(Mark::new(MarkType::Link).with_attr("href", &href))
            } else {
                None
            }
        }
        "span" => {
            let style = el.get_attribute("style").unwrap_or_default();
            extract_css_color(&style, "color")
                .filter(|c| super::view::is_safe_color(c))
                .map(|c| Mark::new(MarkType::TextColor).with_attr("color", &c))
        }
        "mark" => {
            let style = el.get_attribute("style").unwrap_or_default();
            let color = extract_css_color(&style, "background")
                .filter(|c| super::view::is_safe_color(c))
                .unwrap_or_else(|| "#FFF176".to_string());
            Some(Mark::new(MarkType::Highlight).with_attr("color", &color))
        }
        _ => None,
    }
}

/// Extract a CSS property value from an inline style string.
/// e.g. `extract_css_color("color: #E53935; font-size: 14px", "color")` → Some("#E53935")
fn extract_css_color(style: &str, property: &str) -> Option<String> {
    for part in style.split(';') {
        let Some((key, value)) = part.split_once(':') else { continue };
        if key.trim() == property {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Check if an HTML tag is a block-level element that should create paragraph structure.
#[cfg(target_arch = "wasm32")]
fn is_block_level_tag(tag: &str) -> bool {
    matches!(
        tag,
        "div" | "section" | "article" | "header" | "footer"
            | "main" | "nav" | "aside" | "figure" | "figcaption"
            | "details" | "summary" | "address" | "center"
            | "thead" | "tbody" | "tfoot"
    )
}

/// Map an HTML tag to a block node type.
#[cfg(target_arch = "wasm32")]
fn tag_to_block_type(tag: &str) -> Option<NodeType> {
    match tag {
        "p" => Some(NodeType::Paragraph),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Some(NodeType::Heading),
        "ul" => Some(NodeType::BulletList),
        "ol" => Some(NodeType::OrderedList),
        "li" => Some(NodeType::ListItem),
        "blockquote" => Some(NodeType::Blockquote),
        "pre" => Some(NodeType::CodeBlock),
        "hr" => Some(NodeType::HorizontalRule),
        "br" => Some(NodeType::HardBreak),
        "img" => Some(NodeType::Image),
        "table" => Some(NodeType::Table),
        "tr" => Some(NodeType::TableRow),
        "td" => Some(NodeType::TableCell),
        "th" => Some(NodeType::TableHeader),
        _ => None,
    }
}

/// Parse plain text into a Slice.
/// Multi-line text is split into separate paragraphs.
pub fn parse_from_text(text: &str) -> Slice {
    if text.is_empty() {
        return Slice::empty();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() == 1 {
        // Single line: inline content
        Slice::new(Fragment::from(vec![Node::text(text)]), 0, 0)
    } else {
        // Multi-line: each line becomes a paragraph
        let paras: Vec<Node> = lines
            .iter()
            .map(|line| {
                if line.is_empty() {
                    Node::element(NodeType::Paragraph)
                } else {
                    Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text(line)]),
                    )
                }
            })
            .collect();
        Slice::new(Fragment::from(paras), 0, 0)
    }
}

// ─── Context Fitting ────────────────────────────────────────────

/// Adjust a parsed Slice so its content is valid within the target parent node type.
/// For example, pasting a Heading inside a ListItem converts it to a Paragraph.
pub fn fit_slice_to_context(slice: Slice, parent_type: NodeType) -> Slice {
    if slice.content.children.is_empty() {
        return slice;
    }

    let fitted: Vec<Node> = slice
        .content
        .children
        .into_iter()
        .flat_map(|node| fit_node(node, parent_type))
        .collect();

    if fitted.is_empty() {
        return Slice::empty();
    }
    Slice::new(Fragment::from(fitted), slice.open_start, slice.open_end)
}

/// Fit a single node into the target parent context.
fn fit_node(node: Node, parent_type: NodeType) -> Vec<Node> {
    match &node {
        Node::Text { .. } => {
            // Text nodes are valid inline content anywhere that accepts inline
            if parent_type.is_textblock() || parent_type == NodeType::CodeBlock {
                vec![node]
            } else {
                // Wrap in a paragraph for block contexts
                vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![node]),
                )]
            }
        }
        Node::Element { node_type, content, .. } => {
            match node_type {
                // Textblock (Paragraph, Heading, CodeBlock) pasted into a textblock context:
                // extract inline children directly so they merge into the target block
                // instead of creating invalid nested paragraphs.
                _ if node_type.is_textblock() && parent_type.is_textblock() => {
                    content.children.clone()
                }
                // Headings are not valid in list items — downgrade to paragraph
                NodeType::Heading if !is_valid_child(parent_type, *node_type) => {
                    vec![Node::element_with_content(
                        NodeType::Paragraph,
                        content.clone(),
                    )]
                }
                // Other block nodes that aren't valid — extract content as paragraphs
                nt if !nt.is_leaf() && !is_valid_child(parent_type, *node_type) => {
                    let text = node.text_content();
                    if text.is_empty() {
                        vec![]
                    } else {
                        vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text(&text)]),
                        )]
                    }
                }
                _ => vec![node],
            }
        }
    }
}

/// Check if a node type is valid as a child of the given parent type.
fn is_valid_child(parent_type: NodeType, child_type: NodeType) -> bool {
    match parent_type {
        NodeType::Doc => matches!(
            child_type,
            NodeType::Paragraph
                | NodeType::Heading
                | NodeType::BulletList
                | NodeType::OrderedList
                | NodeType::TaskList
                | NodeType::Blockquote
                | NodeType::CodeBlock
                | NodeType::HorizontalRule
                | NodeType::Image
                | NodeType::Table
        ),
        NodeType::ListItem | NodeType::TaskItem => matches!(
            child_type,
            NodeType::Paragraph
                | NodeType::BulletList
                | NodeType::OrderedList
                | NodeType::TaskList
                | NodeType::Blockquote
                | NodeType::CodeBlock
        ),
        NodeType::Blockquote => matches!(
            child_type,
            NodeType::Paragraph
                | NodeType::Heading
                | NodeType::BulletList
                | NodeType::OrderedList
                | NodeType::TaskList
                | NodeType::CodeBlock
                | NodeType::HorizontalRule
        ),
        // Textblocks (Paragraph, Heading, CodeBlock) only accept inline content
        _ if parent_type.is_textblock() => false,
        _ => true,
    }
}

// ─── HTML Rendering ─────────────────────────────────────────────

fn render_node_html(node: &Node, out: &mut String) {
    match node {
        Node::Text { text, marks } => {
            let escaped = html_escape(text);
            if marks.is_empty() {
                out.push_str(&escaped);
            } else {
                // Open marks (outermost first)
                for mark in marks {
                    out.push_str(&open_mark_tag(mark));
                }
                out.push_str(&escaped);
                // Close marks (innermost first)
                for mark in marks.iter().rev() {
                    out.push_str(&close_mark_tag(mark));
                }
            }
        }
        Node::Element {
            node_type,
            attrs,
            content,
            ..
        } => {
            let (open, close) = element_tags(*node_type, attrs);
            out.push_str(&open);
            for child in &content.children {
                render_node_html(child, out);
            }
            if let Some(close) = close {
                out.push_str(&close);
            }
        }
    }
}

fn open_mark_tag(mark: &Mark) -> String {
    match mark.mark_type {
        MarkType::Bold => "<strong>".to_string(),
        MarkType::Italic => "<em>".to_string(),
        MarkType::Underline => "<u>".to_string(),
        MarkType::Strike => "<s>".to_string(),
        MarkType::Code => "<code>".to_string(),
        MarkType::Link => {
            let href = mark
                .attrs
                .get("href")
                .map(|h| format!(" href=\"{}\"", html_escape_attr(h)))
                .unwrap_or_default();
            format!("<a{href}>")
        }
        MarkType::TextColor => {
            let color = mark.attrs.get("color").map(|c| html_escape_attr(c)).unwrap_or_default();
            format!("<span style=\"color: {color}\">")
        }
        MarkType::Highlight => {
            let color = mark.attrs.get("color").map(|c| html_escape_attr(c)).unwrap_or_default();
            format!("<mark style=\"background: {color}\">")
        }
    }
}

fn close_mark_tag(mark: &Mark) -> String {
    match mark.mark_type {
        MarkType::Bold => "</strong>".to_string(),
        MarkType::Italic => "</em>".to_string(),
        MarkType::Underline => "</u>".to_string(),
        MarkType::Strike => "</s>".to_string(),
        MarkType::Code => "</code>".to_string(),
        MarkType::Link => "</a>".to_string(),
        MarkType::TextColor => "</span>".to_string(),
        MarkType::Highlight => "</mark>".to_string(),
    }
}

fn element_tags(
    nt: NodeType,
    attrs: &std::collections::HashMap<String, String>,
) -> (String, Option<String>) {
    match nt {
        NodeType::Paragraph => ("<p>".into(), Some("</p>".into())),
        NodeType::Heading => {
            let level = attrs.get("level").map(|s| s.as_str()).unwrap_or("1");
            let tag = match level {
                "2" => "h2",
                "3" => "h3",
                _ => "h1",
            };
            (format!("<{tag}>"), Some(format!("</{tag}>")))
        }
        NodeType::BulletList => ("<ul>".into(), Some("</ul>".into())),
        NodeType::OrderedList => ("<ol>".into(), Some("</ol>".into())),
        NodeType::ListItem => ("<li>".into(), Some("</li>".into())),
        NodeType::TaskList => ("<ul data-type=\"taskList\">".into(), Some("</ul>".into())),
        NodeType::TaskItem => {
            let checked = attrs
                .get("checked")
                .map(|v| v == "true")
                .unwrap_or(false);
            (
                format!("<li data-type=\"taskItem\" data-checked=\"{checked}\">"),
                Some("</li>".into()),
            )
        }
        NodeType::Blockquote => ("<blockquote>".into(), Some("</blockquote>".into())),
        NodeType::CodeBlock => {
            let lang = attrs.get("language").filter(|l| !l.is_empty());
            let class = lang
                .map(|l| format!(" class=\"language-{}\"", html_escape_attr(l)))
                .unwrap_or_default();
            (
                format!("<pre><code{class}>"),
                Some("</code></pre>".into()),
            )
        }
        NodeType::HorizontalRule => ("<hr />".into(), None),
        NodeType::HardBreak => ("<br />".into(), None),
        NodeType::Image => {
            let src = attrs
                .get("src")
                .map(|s| format!(" src=\"{}\"", html_escape_attr(s)))
                .unwrap_or_default();
            let alt = attrs
                .get("alt")
                .map(|a| format!(" alt=\"{}\"", html_escape_attr(a)))
                .unwrap_or_default();
            (format!("<img{src}{alt} />"), None)
        }
        NodeType::Table => ("<table>".into(), Some("</table>".into())),
        NodeType::TableRow => ("<tr>".into(), Some("</tr>".into())),
        NodeType::TableCell => {
            let mut tag = "<td".to_string();
            if let Some(cs) = attrs.get("colspan").filter(|v| *v != "1") {
                tag.push_str(&format!(" colspan=\"{}\"", html_escape_attr(cs)));
            }
            if let Some(rs) = attrs.get("rowspan").filter(|v| *v != "1") {
                tag.push_str(&format!(" rowspan=\"{}\"", html_escape_attr(rs)));
            }
            tag.push('>');
            (tag, Some("</td>".into()))
        }
        NodeType::TableHeader => {
            let mut tag = "<th".to_string();
            if let Some(cs) = attrs.get("colspan").filter(|v| *v != "1") {
                tag.push_str(&format!(" colspan=\"{}\"", html_escape_attr(cs)));
            }
            if let Some(rs) = attrs.get("rowspan").filter(|v| *v != "1") {
                tag.push_str(&format!(" rowspan=\"{}\"", html_escape_attr(rs)));
            }
            tag.push('>');
            (tag, Some("</th>".into()))
        }
        NodeType::Doc => ("<div>".into(), Some("</div>".into())),
    }
}

fn collect_text(node: &Node, out: &mut String) {
    match node {
        Node::Text { text, .. } => out.push_str(text),
        Node::Element { content, node_type, .. } => {
            if *node_type == NodeType::HardBreak {
                out.push('\n');
                return;
            }
            let start_len = out.len();
            for child in &content.children {
                collect_text(child, out);
            }
            // Add double newline after block elements (paragraph separation)
            if node_type.is_block() && out.len() > start_len {
                out.push('\n');
            }
        }
    }
}

// ─── HTML Helpers ───────────────────────────────────────────────

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Strip HTML tags from a string, keeping only text content.
/// Handles `<` inside quoted attribute values to avoid garbled output.
fn strip_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_quote: Option<char> = None;

    for ch in html.chars() {
        if in_tag {
            if let Some(q) = in_quote {
                if ch == q {
                    in_quote = None;
                }
            } else if ch == '"' || ch == '\'' {
                in_quote = Some(ch);
            } else if ch == '>' {
                in_tag = false;
            }
        } else if ch == '<' {
            in_tag = true;
            in_quote = None;
        } else {
            result.push(ch);
        }
    }
    result
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn serialize_plain_text() {
        let slice = Slice::new(Fragment::from(vec![Node::text("Hello")]), 0, 0);
        assert_eq!(serialize_to_html(&slice), "Hello");
        assert_eq!(serialize_to_text(&slice), "Hello");
    }

    #[test]
    fn serialize_bold_text() {
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks(
                "Bold",
                vec![Mark::new(MarkType::Bold)],
            )]),
            0,
            0,
        );
        assert_eq!(serialize_to_html(&slice), "<strong>Bold</strong>");
    }

    #[test]
    fn serialize_bold_italic() {
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks(
                "Both",
                vec![Mark::new(MarkType::Bold), Mark::new(MarkType::Italic)],
            )]),
            0,
            0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("<strong>"));
        assert!(html.contains("<em>"));
        assert!(html.contains("Both"));
    }

    #[test]
    fn serialize_link() {
        let link = Mark::new(MarkType::Link).with_attr("href", "https://example.com");
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks("Click", vec![link])]),
            0,
            0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("href=\"https://example.com\""));
        assert!(html.contains("Click"));
    }

    #[test]
    fn serialize_paragraph() {
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello")]),
            )]),
            0,
            0,
        );
        assert_eq!(serialize_to_html(&slice), "<p>Hello</p>");
    }

    #[test]
    fn serialize_heading() {
        let mut attrs = HashMap::new();
        attrs.insert("level".to_string(), "2".to_string());
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Heading,
                attrs,
                Fragment::from(vec![Node::text("Title")]),
            )]),
            0,
            0,
        );
        assert_eq!(serialize_to_html(&slice), "<h2>Title</h2>");
    }

    #[test]
    fn serialize_code_block() {
        let mut attrs = HashMap::new();
        attrs.insert("language".to_string(), "rust".to_string());
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::CodeBlock,
                attrs,
                Fragment::from(vec![Node::text("fn main() {}")]),
            )]),
            0,
            0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("<pre><code class=\"language-rust\">"));
        assert!(html.contains("fn main() {}"));
    }

    #[test]
    fn serialize_hr() {
        let slice = Slice::new(
            Fragment::from(vec![Node::element(NodeType::HorizontalRule)]),
            0,
            0,
        );
        assert_eq!(serialize_to_html(&slice), "<hr />");
    }

    #[test]
    fn serialize_escapes_html() {
        let slice = Slice::new(
            Fragment::from(vec![Node::text("<script>alert('xss')</script>")]),
            0,
            0,
        );
        let html = serialize_to_html(&slice);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn parse_plain_text() {
        let slice = parse_from_text("Hello world");
        assert_eq!(slice.content.child(0).unwrap().text_content(), "Hello world");
    }

    #[test]
    fn parse_html_strips_tags() {
        let slice = parse_from_html("<p>Hello <strong>world</strong></p>");
        assert_eq!(
            slice.content.child(0).unwrap().text_content(),
            "Hello world"
        );
    }

    #[test]
    fn parse_empty_returns_empty_slice() {
        assert_eq!(parse_from_text("").size(), 0);
        assert_eq!(parse_from_html("").size(), 0);
    }

    #[test]
    fn strip_tags_basic() {
        assert_eq!(strip_tags("<p>Hello</p>"), "Hello");
        assert_eq!(strip_tags("<b>bold</b> text"), "bold text");
        assert_eq!(strip_tags("no tags"), "no tags");
        assert_eq!(strip_tags("<img src=\"x\" />"), "");
    }

    #[test]
    fn to_text_adds_newlines_for_blocks() {
        let slice = Slice::new(
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hello")]),
                ),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("World")]),
                ),
            ]),
            0,
            0,
        );
        let text = serialize_to_text(&slice);
        assert!(text.contains("Hello\n"));
        assert!(text.contains("World"));
    }

    #[test]
    fn strip_tags_with_attribute_angle_brackets() {
        // < inside a quoted attribute should not break the parser
        assert_eq!(strip_tags("<img alt=\"a < b\" />"), "");
        assert_eq!(strip_tags("<div title=\"x>y\">text</div>"), "text");
    }

    #[test]
    fn code_block_language_escaped_in_html() {
        let mut attrs = HashMap::new();
        attrs.insert("language".to_string(), "rust\" onload=\"alert(1)".to_string());
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::CodeBlock,
                attrs,
                Fragment::from(vec![Node::text("code")]),
            )]),
            0,
            0,
        );
        let html = serialize_to_html(&slice);
        // The " in the language value should be escaped to &quot;
        // preventing attribute injection (the raw " can't close the class attr)
        assert!(html.contains("&quot;"));
        // The class attribute value should contain the escaped version
        assert!(html.contains("language-rust&quot;"));
    }

    // ── fit_slice_to_context ──

    #[test]
    fn fit_heading_in_list_item_becomes_paragraph() {
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("level".to_string(), "1".to_string());
        let heading = Node::element_with_attrs(
            NodeType::Heading,
            attrs,
            Fragment::from(vec![Node::text("Title")]),
        );
        let slice = Slice::new(Fragment::from(vec![heading]), 0, 0);
        let fitted = fit_slice_to_context(slice, NodeType::ListItem);

        assert_eq!(fitted.content.children.len(), 1);
        let node = &fitted.content.children[0];
        assert_eq!(node.node_type(), Some(NodeType::Paragraph));
        assert_eq!(node.text_content(), "Title");
    }

    #[test]
    fn fit_paragraph_in_doc_unchanged() {
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("Hello")]),
        );
        let slice = Slice::new(Fragment::from(vec![para]), 0, 0);
        let fitted = fit_slice_to_context(slice, NodeType::Doc);

        assert_eq!(fitted.content.children.len(), 1);
        assert_eq!(
            fitted.content.children[0].node_type(),
            Some(NodeType::Paragraph)
        );
    }

    // ── parse_from_text multiline ──

    #[test]
    fn parse_text_multiline_creates_paragraphs() {
        let slice = parse_from_text("Hello\nWorld");
        assert_eq!(slice.content.children.len(), 2);
        assert_eq!(
            slice.content.children[0].node_type(),
            Some(NodeType::Paragraph)
        );
        assert_eq!(slice.content.children[0].text_content(), "Hello");
        assert_eq!(slice.content.children[1].text_content(), "World");
    }

    #[test]
    fn parse_text_single_line_is_inline() {
        let slice = parse_from_text("Hello");
        assert_eq!(slice.content.children.len(), 1);
        // Single line: inline text, no paragraph wrapper
        assert!(matches!(slice.content.children[0], Node::Text { .. }));
    }

    #[test]
    fn parse_text_empty_line_creates_empty_paragraph() {
        let slice = parse_from_text("Hello\n\nWorld");
        assert_eq!(slice.content.children.len(), 3);
        assert_eq!(slice.content.children[1].text_content(), "");
        assert_eq!(slice.content.children[1].node_type(), Some(NodeType::Paragraph));
    }

    // ── serialize_to_html: all mark types ──

    #[test]
    fn serialize_underline() {
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks("u", vec![Mark::new(MarkType::Underline)])]),
            0, 0,
        );
        assert_eq!(serialize_to_html(&slice), "<u>u</u>");
    }

    #[test]
    fn serialize_strike() {
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks("del", vec![Mark::new(MarkType::Strike)])]),
            0, 0,
        );
        assert_eq!(serialize_to_html(&slice), "<s>del</s>");
    }

    #[test]
    fn serialize_code_mark() {
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks("x", vec![Mark::new(MarkType::Code)])]),
            0, 0,
        );
        assert_eq!(serialize_to_html(&slice), "<code>x</code>");
    }

    #[test]
    fn serialize_text_color() {
        let mark = Mark::new(MarkType::TextColor).with_attr("color", "#E53935");
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks("red", vec![mark])]),
            0, 0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("style=\"color: #E53935\""));
        assert!(html.contains("red"));
        assert!(html.contains("</span>"));
    }

    #[test]
    fn serialize_highlight() {
        let mark = Mark::new(MarkType::Highlight).with_attr("color", "#FFF176");
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks("hi", vec![mark])]),
            0, 0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("style=\"background: #FFF176\""));
        assert!(html.contains("</mark>"));
    }

    #[test]
    fn serialize_mixed_marks_and_plain() {
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text("plain "),
                    Node::text_with_marks("bold", vec![Mark::new(MarkType::Bold)]),
                    Node::text(" end"),
                ]),
            )]),
            0, 0,
        );
        assert_eq!(
            serialize_to_html(&slice),
            "<p>plain <strong>bold</strong> end</p>"
        );
    }

    // ── serialize_to_html: element types ──

    #[test]
    fn serialize_blockquote() {
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("quoted")]),
                )]),
            )]),
            0, 0,
        );
        assert_eq!(serialize_to_html(&slice), "<blockquote><p>quoted</p></blockquote>");
    }

    #[test]
    fn serialize_task_list() {
        let mut item_attrs = HashMap::new();
        item_attrs.insert("checked".to_string(), "true".to_string());
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::TaskList,
                Fragment::from(vec![Node::element_with_attrs(
                    NodeType::TaskItem,
                    item_attrs,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("done")]),
                    )]),
                )]),
            )]),
            0, 0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("data-type=\"taskList\""));
        assert!(html.contains("data-checked=\"true\""));
        assert!(html.contains("done"));
    }

    #[test]
    fn serialize_image() {
        let mut attrs = HashMap::new();
        attrs.insert("src".to_string(), "photo.png".to_string());
        attrs.insert("alt".to_string(), "A photo".to_string());
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Image, attrs, Fragment::empty(),
            )]),
            0, 0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("src=\"photo.png\""));
        assert!(html.contains("alt=\"A photo\""));
        assert!(html.contains("/>"));
    }

    #[test]
    fn serialize_hard_break() {
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text("line1"),
                    Node::element(NodeType::HardBreak),
                    Node::text("line2"),
                ]),
            )]),
            0, 0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("line1<br />line2"));
    }

    #[test]
    fn serialize_nested_list() {
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("item 1")]),
                        )]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("item 2")]),
                        )]),
                    ),
                ]),
            )]),
            0, 0,
        );
        assert_eq!(
            serialize_to_html(&slice),
            "<ul><li><p>item 1</p></li><li><p>item 2</p></li></ul>"
        );
    }

    #[test]
    fn serialize_heading_level_3() {
        let mut attrs = HashMap::new();
        attrs.insert("level".to_string(), "3".to_string());
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Heading, attrs,
                Fragment::from(vec![Node::text("Small")]),
            )]),
            0, 0,
        );
        assert_eq!(serialize_to_html(&slice), "<h3>Small</h3>");
    }

    #[test]
    fn serialize_empty_paragraph() {
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph, Fragment::empty(),
            )]),
            0, 0,
        );
        assert_eq!(serialize_to_html(&slice), "<p></p>");
    }

    // ── serialize_to_text ──

    #[test]
    fn to_text_hard_break_is_newline() {
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text("a"),
                    Node::element(NodeType::HardBreak),
                    Node::text("b"),
                ]),
            )]),
            0, 0,
        );
        let text = serialize_to_text(&slice);
        assert!(text.contains("a\nb"));
    }

    #[test]
    fn to_text_nested_list() {
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("item")]),
                    )]),
                )]),
            )]),
            0, 0,
        );
        let text = serialize_to_text(&slice);
        assert!(text.contains("item"));
    }

    #[test]
    fn to_text_empty_paragraph() {
        let slice = Slice::new(
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("before")]),
                ),
                Node::element_with_content(NodeType::Paragraph, Fragment::empty()),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("after")]),
                ),
            ]),
            0, 0,
        );
        let text = serialize_to_text(&slice);
        assert!(text.contains("before"));
        assert!(text.contains("after"));
    }

    // ── extract_css_color ──

    #[test]
    fn extract_css_color_basic() {
        let color = extract_css_color("color: #E53935; font-size: 14px", "color");
        assert_eq!(color, Some("#E53935".to_string()));
    }

    #[test]
    fn extract_css_color_background() {
        let color = extract_css_color("background: yellow", "background");
        assert_eq!(color, Some("yellow".to_string()));
    }

    #[test]
    fn extract_css_color_no_match() {
        assert_eq!(extract_css_color("font-size: 14px", "color"), None);
        assert_eq!(extract_css_color("", "color"), None);
    }

    #[test]
    fn extract_css_color_prefix_not_confused() {
        // "background-color" should NOT match when searching for "background"
        let result = extract_css_color("background-color: red", "background");
        assert_eq!(result, None);
    }

    #[test]
    fn extract_css_color_with_spaces() {
        let color = extract_css_color("  color :  rgb(255, 0, 0)  ;", "color");
        assert_eq!(color, Some("rgb(255, 0, 0)".to_string()));
    }

    // ── Security: html_escape_attr ──

    #[test]
    fn link_href_with_quotes_escaped() {
        let link = Mark::new(MarkType::Link).with_attr("href", "x\" onclick=\"alert(1)");
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks("click", vec![link])]),
            0, 0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("&quot;"), "quotes in href must be escaped");
        // The escaped output should NOT contain a raw onclick= attribute breakout.
        // The " is escaped so onclick stays inside the href value, not as its own attribute.
        assert!(!html.contains("\" onclick="), "attribute injection must not break out of quotes");
    }

    #[test]
    fn image_src_with_quotes_escaped() {
        let mut attrs = HashMap::new();
        attrs.insert("src".to_string(), "x\" onerror=\"alert(1)".to_string());
        let slice = Slice::new(
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Image, attrs, Fragment::empty(),
            )]),
            0, 0,
        );
        let html = serialize_to_html(&slice);
        assert!(html.contains("&quot;"), "quotes in src must be escaped");
        assert!(!html.contains("\" onerror="), "attribute injection must not break out of quotes");
    }

    #[test]
    fn serialize_link_javascript_url() {
        // javascript: URLs should be rendered as-is by serialize (the view layer
        // is responsible for sanitization), but let's document the behavior.
        let link = Mark::new(MarkType::Link).with_attr("href", "javascript:alert(1)");
        let slice = Slice::new(
            Fragment::from(vec![Node::text_with_marks("click", vec![link])]),
            0, 0,
        );
        let html = serialize_to_html(&slice);
        // The href value is present (serialize doesn't filter URLs)
        assert!(html.contains("href="));
    }

    // ── fit_slice_to_context: additional cases ──

    #[test]
    fn fit_text_in_doc_wraps_in_paragraph() {
        let slice = Slice::new(Fragment::from(vec![Node::text("loose text")]), 0, 0);
        let fitted = fit_slice_to_context(slice, NodeType::Doc);
        assert_eq!(fitted.content.children.len(), 1);
        assert_eq!(fitted.content.children[0].node_type(), Some(NodeType::Paragraph));
        assert_eq!(fitted.content.children[0].text_content(), "loose text");
    }

    #[test]
    fn fit_empty_slice_returns_empty() {
        let slice = Slice::empty();
        let fitted = fit_slice_to_context(slice, NodeType::Doc);
        assert_eq!(fitted.content.children.len(), 0);
    }

    #[test]
    fn fit_mixed_valid_and_invalid_in_list_item() {
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("valid")]),
        );
        let mut attrs = HashMap::new();
        attrs.insert("level".to_string(), "1".to_string());
        let heading = Node::element_with_attrs(
            NodeType::Heading, attrs,
            Fragment::from(vec![Node::text("invalid heading")]),
        );
        let slice = Slice::new(Fragment::from(vec![para, heading]), 0, 0);
        let fitted = fit_slice_to_context(slice, NodeType::ListItem);

        assert_eq!(fitted.content.children.len(), 2);
        // Paragraph stays as-is
        assert_eq!(fitted.content.children[0].node_type(), Some(NodeType::Paragraph));
        // Heading downgraded to paragraph
        assert_eq!(fitted.content.children[1].node_type(), Some(NodeType::Paragraph));
        assert_eq!(fitted.content.children[1].text_content(), "invalid heading");
    }

    #[test]
    fn fit_list_in_doc_unchanged() {
        let list = Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("item")]),
                )]),
            )]),
        );
        let slice = Slice::new(Fragment::from(vec![list]), 0, 0);
        let fitted = fit_slice_to_context(slice, NodeType::Doc);
        assert_eq!(fitted.content.children[0].node_type(), Some(NodeType::BulletList));
    }

    #[test]
    fn fit_blockquote_valid_in_list_item() {
        let bq = Node::element_with_content(
            NodeType::Blockquote,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("quoted")]),
            )]),
        );
        let slice = Slice::new(Fragment::from(vec![bq]), 0, 0);
        let fitted = fit_slice_to_context(slice, NodeType::ListItem);
        assert_eq!(fitted.content.children[0].node_type(), Some(NodeType::Blockquote));
    }

    // ── fit_slice_to_context: paragraph into paragraph (inline paste) ──

    #[test]
    fn fit_paragraph_into_paragraph_extracts_inline_content() {
        // Pasting a Paragraph("23") into a Paragraph context should extract
        // the inline children ("23" text node), not keep the wrapper Paragraph.
        // This prevents nested <p> inside <p> which breaks contenteditable.
        let pasted = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("23")]),
        );
        let slice = Slice::new(Fragment::from(vec![pasted]), 0, 0);
        let fitted = fit_slice_to_context(slice, NodeType::Paragraph);

        // Should produce a text node, not a paragraph
        assert_eq!(fitted.content.children.len(), 1);
        assert!(matches!(fitted.content.children[0], Node::Text { .. }),
            "should extract inline text, got: {:?}", fitted.content.children[0]);
        assert_eq!(fitted.content.children[0].text_content(), "23");
    }

    #[test]
    fn fit_paragraph_with_marks_into_paragraph_preserves_marks() {
        // Pasting bold text from another paragraph should preserve the marks.
        let pasted = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![
                Node::text("plain "),
                Node::text_with_marks("bold", vec![Mark::new(MarkType::Bold)]),
            ]),
        );
        let slice = Slice::new(Fragment::from(vec![pasted]), 0, 0);
        let fitted = fit_slice_to_context(slice, NodeType::Paragraph);

        // Should have 2 inline nodes (plain text + bold text), not a Paragraph wrapper
        assert_eq!(fitted.content.children.len(), 2);
        assert!(matches!(fitted.content.children[0], Node::Text { .. }));
        assert_eq!(fitted.content.children[0].text_content(), "plain ");
        assert_eq!(fitted.content.children[1].text_content(), "bold");
        assert!(fitted.content.children[1].marks().iter().any(|m| m.mark_type == MarkType::Bold));
    }

    // ── strip_tags edge cases ──

    #[test]
    fn strip_tags_nested() {
        assert_eq!(strip_tags("<div><p><b>deep</b></p></div>"), "deep");
    }

    #[test]
    fn strip_tags_unclosed() {
        assert_eq!(strip_tags("<p>unclosed"), "unclosed");
    }

    #[test]
    fn strip_tags_self_closing() {
        assert_eq!(strip_tags("a<br/>b"), "ab");
        assert_eq!(strip_tags("a<hr />b"), "ab");
    }
}
