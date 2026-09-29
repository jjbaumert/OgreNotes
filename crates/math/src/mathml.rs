// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! The MathML tree the parser builds, and its serialization. Every text
//! node and attribute value is escaped on the way out; attribute *names*
//! are always compile-time constants.

pub(crate) type Attrs = Vec<(&'static str, String)>;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    /// A token element (`mi`, `mn`, `mo`, `mtext`) holding text.
    Leaf { tag: &'static str, attrs: Attrs, text: String },
    /// Any other element.
    El { tag: &'static str, attrs: Attrs, children: Vec<Node> },
}

impl Node {
    pub(crate) fn leaf(tag: &'static str, text: impl Into<String>) -> Node {
        Node::Leaf { tag, attrs: Vec::new(), text: text.into() }
    }

    pub(crate) fn el(tag: &'static str, children: Vec<Node>) -> Node {
        Node::El { tag, attrs: Vec::new(), children }
    }

    pub(crate) fn with(mut self, name: &'static str, value: impl Into<String>) -> Node {
        match &mut self {
            Node::Leaf { attrs, .. } | Node::El { attrs, .. } => attrs.push((name, value.into())),
        }
        self
    }

    pub(crate) fn mi(text: impl Into<String>) -> Node {
        Node::leaf("mi", text)
    }

    /// An upright identifier (multi-letter names, upright Greek, `\mathrm`).
    pub(crate) fn mi_upright(text: impl Into<String>) -> Node {
        Node::leaf("mi", text).with("mathvariant", "normal")
    }

    pub(crate) fn mn(text: impl Into<String>) -> Node {
        Node::leaf("mn", text)
    }

    pub(crate) fn mo(text: impl Into<String>) -> Node {
        Node::leaf("mo", text)
    }

    pub(crate) fn mtext(text: impl Into<String>) -> Node {
        Node::leaf("mtext", text)
    }

    pub(crate) fn mspace(width: &str) -> Node {
        Node::el("mspace", Vec::new()).with("width", width)
    }

    /// `children` as one node: the node itself when there is exactly one,
    /// otherwise an `mrow` (script and fraction slots take exactly one).
    pub(crate) fn row(mut children: Vec<Node>) -> Node {
        if children.len() == 1 {
            children.pop().expect("len checked")
        } else {
            Node::el("mrow", children)
        }
    }

    pub(crate) fn tag(&self) -> &'static str {
        match self {
            Node::Leaf { tag, .. } | Node::El { tag, .. } => tag,
        }
    }

    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Node::Leaf { text, .. } => Some(text),
            Node::El { .. } => None,
        }
    }

    /// Set (or replace) an attribute in place.
    pub(crate) fn set_attr(&mut self, name: &'static str, value: impl Into<String>) {
        let attrs = match self {
            Node::Leaf { attrs, .. } | Node::El { attrs, .. } => attrs,
        };
        let value = value.into();
        match attrs.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => attrs.push((name, value)),
        }
    }

    pub(crate) fn write(&self, out: &mut String) {
        match self {
            Node::Leaf { tag, attrs, text } => {
                open(out, tag, attrs);
                escape_into(out, text);
                close(out, tag);
            }
            Node::El { tag, attrs, children } => {
                if children.is_empty() {
                    out.push('<');
                    out.push_str(tag);
                    write_attrs(out, attrs);
                    out.push_str("/>");
                } else {
                    open(out, tag, attrs);
                    for c in children {
                        c.write(out);
                    }
                    close(out, tag);
                }
            }
        }
    }
}

fn open(out: &mut String, tag: &str, attrs: &Attrs) {
    out.push('<');
    out.push_str(tag);
    write_attrs(out, attrs);
    out.push('>');
}

fn close(out: &mut String, tag: &str) {
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}

fn write_attrs(out: &mut String, attrs: &Attrs) {
    for (name, value) in attrs {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        escape_into(out, value);
        out.push('"');
    }
}

/// XML-escape `s` into `out`. Control characters other than tab/newline
/// are illegal in XML 1.0 even as entities, so they are dropped.
pub(crate) fn escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: &Node) -> String {
        let mut out = String::new();
        n.write(&mut out);
        out
    }

    #[test]
    fn serializes_and_escapes() {
        let n = Node::el("mrow", vec![Node::mi("x"), Node::mo("<")]).with("title", "a\"b");
        assert_eq!(s(&n), r#"<mrow title="a&quot;b"><mi>x</mi><mo>&lt;</mo></mrow>"#);
        assert_eq!(s(&Node::mspace("1em")), r#"<mspace width="1em"/>"#);
        assert_eq!(s(&Node::mtext("a\u{1}b")), "<mtext>ab</mtext>");
    }

    #[test]
    fn row_of_one_is_the_node() {
        assert_eq!(Node::row(vec![Node::mi("x")]), Node::mi("x"));
        assert_eq!(Node::row(vec![]).tag(), "mrow");
    }
}
