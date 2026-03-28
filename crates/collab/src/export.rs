use yrs::{
    Doc, ReadTxn, Transact,
    types::xml::{Xml, XmlFragment, XmlOut},
    types::GetString,
};

use crate::schema::NodeType;

/// Export a yrs document to HTML.
pub fn to_html(doc: &Doc) -> String {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return String::new();
    };

    let mut html = String::new();
    render_fragment_html(&txn, &fragment, &mut html);
    html
}

/// Export a yrs document to Markdown.
pub fn to_markdown(doc: &Doc) -> String {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return String::new();
    };

    let mut md = String::new();
    render_fragment_markdown(&txn, &fragment, &mut md, 0);
    md
}

fn render_fragment_html<T: ReadTxn>(txn: &T, fragment: &yrs::XmlFragmentRef, out: &mut String) {
    let len = fragment.len(txn);
    for i in 0..len {
        if let Some(child) = fragment.get(txn, i) {
            render_node_html(txn, &child, out);
        }
    }
}

fn render_node_html<T: ReadTxn>(txn: &T, node: &XmlOut, out: &mut String) {
    match node {
        XmlOut::Element(el) => {
            let tag = el.tag();
            let Some(node_type) = NodeType::from_tag(&tag) else {
                return;
            };

            // Compute the correct HTML tag (headings need dynamic tags)
            let html_tag = resolve_html_tag(txn, el, node_type);
            let attrs = render_html_attrs(txn, el, node_type);

            if node_type.is_leaf() {
                out.push_str(&format!("<{html_tag}{attrs} />"));
                return;
            }

            out.push_str(&format!("<{html_tag}{attrs}>"));

            // Render children
            let len = el.len(txn);
            for i in 0..len {
                if let Some(child) = el.get(txn, i) {
                    render_node_html(txn, &child, out);
                }
            }

            out.push_str(&format!("</{html_tag}>"));
        }
        XmlOut::Text(text) => {
            let content = text.get_string(txn);
            out.push_str(&html_escape(&content));
        }
        _ => {}
    }
}

fn render_fragment_markdown<T: ReadTxn>(
    txn: &T,
    fragment: &yrs::XmlFragmentRef,
    out: &mut String,
    depth: usize,
) {
    let len = fragment.len(txn);
    for i in 0..len {
        if let Some(child) = fragment.get(txn, i) {
            render_node_markdown(txn, &child, out, depth);
        }
    }
}

fn render_node_markdown<T: ReadTxn>(txn: &T, node: &XmlOut, out: &mut String, depth: usize) {
    match node {
        XmlOut::Element(el) => {
            let tag = el.tag();
            let Some(node_type) = NodeType::from_tag(&tag) else {
                return;
            };

            match node_type {
                NodeType::Paragraph => {
                    render_children_markdown(txn, el, out, depth);
                    out.push_str("\n\n");
                }
                NodeType::Heading => {
                    let level = el
                        .get_attribute(txn, "level")
                        .and_then(|v| v.parse::<u8>().ok())
                        .unwrap_or(1)
                        .clamp(1, 6);
                    let prefix = "#".repeat(level as usize);
                    out.push_str(&format!("{prefix} "));
                    render_children_markdown(txn, el, out, depth);
                    out.push_str("\n\n");
                }
                NodeType::BulletList => {
                    render_list_items_markdown(txn, el, out, depth, "- ");
                }
                NodeType::OrderedList => {
                    render_list_items_markdown(txn, el, out, depth, "1. ");
                }
                NodeType::ListItem | NodeType::TaskItem => {
                    render_children_markdown(txn, el, out, depth);
                }
                NodeType::Blockquote => {
                    out.push_str("> ");
                    render_children_markdown(txn, el, out, depth);
                }
                NodeType::CodeBlock => {
                    let lang = el.get_attribute(txn, "language").unwrap_or_default();
                    out.push_str(&format!("```{lang}\n"));
                    render_children_markdown(txn, el, out, depth);
                    out.push_str("\n```\n\n");
                }
                NodeType::HorizontalRule => {
                    out.push_str("---\n\n");
                }
                NodeType::HardBreak => {
                    out.push_str("  \n");
                }
                NodeType::Image => {
                    let alt = el.get_attribute(txn, "alt").unwrap_or_default();
                    let src = el.get_attribute(txn, "src").unwrap_or_default();
                    if is_safe_url(&src) {
                        out.push_str(&format!("![{alt}]({src})"));
                    }
                }
                _ => {
                    render_children_markdown(txn, el, out, depth);
                }
            }
        }
        XmlOut::Text(text) => {
            let content = text.get_string(txn);
            out.push_str(&escape_markdown_text(&content));
        }
        _ => {}
    }
}

fn render_children_markdown<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    out: &mut String,
    depth: usize,
) {
    let len = el.len(txn);
    for i in 0..len {
        if let Some(child) = el.get(txn, i) {
            render_node_markdown(txn, &child, out, depth);
        }
    }
}

fn render_list_items_markdown<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    out: &mut String,
    depth: usize,
    prefix: &str,
) {
    let indent = "  ".repeat(depth);
    let len = el.len(txn);
    for i in 0..len {
        if let Some(child) = el.get(txn, i) {
            out.push_str(&format!("{indent}{prefix}"));
            render_node_markdown(txn, &child, out, depth + 1);
            out.push('\n');
        }
    }
    if depth == 0 {
        out.push('\n');
    }
}

/// Resolve the HTML tag for a node type, handling headings dynamically.
fn resolve_html_tag<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef, nt: NodeType) -> String {
    match nt {
        NodeType::Heading => {
            let level = el
                .get_attribute(txn, "level")
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or(1)
                .clamp(1, 6);
            format!("h{level}")
        }
        _ => node_type_to_html_tag(nt).to_string(),
    }
}

fn node_type_to_html_tag(nt: NodeType) -> &'static str {
    match nt {
        NodeType::Doc => "div",
        NodeType::Paragraph => "p",
        NodeType::Heading => "h1", // unreachable -- handled by resolve_html_tag
        NodeType::BulletList => "ul",
        NodeType::OrderedList => "ol",
        NodeType::ListItem => "li",
        NodeType::TaskList => "ul",
        NodeType::TaskItem => "li",
        NodeType::Blockquote => "blockquote",
        NodeType::CodeBlock => "pre",
        NodeType::HorizontalRule => "hr",
        NodeType::HardBreak => "br",
        NodeType::Image => "img",
    }
}

fn render_html_attrs<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    node_type: NodeType,
) -> String {
    let mut attrs = String::new();

    match node_type {
        NodeType::Heading => {
            // Tag is handled by resolve_html_tag; no extra attrs needed
        }
        NodeType::CodeBlock => {
            if let Some(lang) = el.get_attribute(txn, "language") {
                attrs.push_str(&format!(" class=\"language-{}\"", html_escape_attr(&lang)));
            }
        }
        NodeType::Image => {
            if let Some(src) = el.get_attribute(txn, "src") {
                if is_safe_url(&src) {
                    attrs.push_str(&format!(" src=\"{}\"", html_escape_attr(&src)));
                }
            }
            if let Some(alt) = el.get_attribute(txn, "alt") {
                attrs.push_str(&format!(" alt=\"{}\"", html_escape_attr(&alt)));
            }
            if let Some(title) = el.get_attribute(txn, "title") {
                attrs.push_str(&format!(" title=\"{}\"", html_escape_attr(&title)));
            }
        }
        NodeType::TaskList => {
            attrs.push_str(" data-type=\"taskList\"");
        }
        NodeType::TaskItem => {
            let checked = el
                .get_attribute(txn, "checked")
                .map(|v| v == "true")
                .unwrap_or(false);
            attrs.push_str(&format!(" data-type=\"taskItem\" data-checked=\"{checked}\""));
        }
        _ => {}
    }

    attrs
}

/// Check that a URL uses a safe protocol (blocks javascript: and data: URIs).
fn is_safe_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with('/')
        || lower.starts_with("data:image/") // allow data URIs for images only
}

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

/// Escape Markdown structural characters at line start to prevent
/// text content from being interpreted as Markdown structure.
fn escape_markdown_text(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for line in s.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#')
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("> ")
            || trimmed.starts_with("```")
            || trimmed.starts_with("---")
        {
            result.push('\\');
        }
        result.push_str(line);
        result.push('\n');
    }
    // Remove trailing newline added by the loop
    if result.ends_with('\n') && !s.ends_with('\n') {
        result.pop();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::OgreDoc;

    #[test]
    fn export_html_empty_doc() {
        let doc = OgreDoc::new();
        let html = to_html(doc.inner());
        assert!(html.contains("<p"));
    }

    #[test]
    fn export_markdown_empty_doc() {
        let doc = OgreDoc::new();
        let md = to_markdown(doc.inner());
        assert!(md.trim().is_empty() || md.contains('\n'));
    }

    #[test]
    fn html_escape_special_chars() {
        assert_eq!(
            html_escape("<script>alert('xss')</script>"),
            "&lt;script&gt;alert('xss')&lt;/script&gt;"
        );
    }

    #[test]
    fn html_escape_attr_quotes() {
        assert_eq!(
            html_escape_attr("value \"with\" quotes"),
            "value &quot;with&quot; quotes"
        );
    }

    #[test]
    fn safe_url_allows_http() {
        assert!(is_safe_url("https://example.com/img.png"));
        assert!(is_safe_url("http://example.com/img.png"));
        assert!(is_safe_url("/images/photo.jpg"));
        assert!(is_safe_url("data:image/png;base64,abc123"));
    }

    #[test]
    fn safe_url_blocks_javascript() {
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("JAVASCRIPT:alert(1)"));
        assert!(!is_safe_url(" javascript:alert(1)"));
    }

    #[test]
    fn safe_url_blocks_data_non_image() {
        assert!(!is_safe_url("data:text/html,<script>alert(1)</script>"));
    }

    #[test]
    fn markdown_escape_structural_chars() {
        assert_eq!(escape_markdown_text("# Not a heading"), "\\# Not a heading");
        assert_eq!(escape_markdown_text("- Not a list"), "\\- Not a list");
        assert_eq!(escape_markdown_text("Normal text"), "Normal text");
    }
}
