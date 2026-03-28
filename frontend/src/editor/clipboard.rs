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
/// For MVP, this handles basic inline HTML tags.
pub fn parse_from_html(html: &str) -> Slice {
    // Simplified HTML parsing for MVP:
    // Strip all tags and return plain text.
    // A full parser would use html5ever, but for the MVP
    // we just extract text content.
    let text = strip_tags(html);
    if text.is_empty() {
        return Slice::empty();
    }
    let content = Fragment::from(vec![Node::text(&text)]);
    Slice::new(content, 0, 0)
}

/// Parse plain text into a Slice.
pub fn parse_from_text(text: &str) -> Slice {
    if text.is_empty() {
        return Slice::empty();
    }
    let content = Fragment::from(vec![Node::text(text)]);
    Slice::new(content, 0, 0)
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
}
