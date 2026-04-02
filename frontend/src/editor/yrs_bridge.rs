use std::collections::HashMap;
use std::sync::Arc;

use yrs::{
    Any, Doc, Out, ReadTxn, Transact, WriteTxn,
    Update,
    updates::decoder::Decode,
    types::Attrs,
    types::text::{Diff, YChange},
    types::xml::{Xml, XmlElementPrelim, XmlFragment, XmlOut, XmlTextPrelim},
    types::{GetString, Text},
};

use super::model::{Fragment, Mark, MarkType, Node, NodeType};

/// Convert an editor document model to yrs Y.Doc state bytes.
/// Inline marks (bold, italic, link, etc.) are preserved as yrs formatting attributes.
pub fn doc_to_ydoc_bytes(doc: &Node) -> Vec<u8> {
    let ydoc = Doc::new();

    {
        let mut txn = ydoc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");

        if let Node::Element { content, .. } = doc {
            for (i, child) in content.children.iter().enumerate() {
                write_node_to_fragment(&mut txn, &fragment, i as u32, child);
            }
        }
    }

    let txn = ydoc.transact();
    txn.encode_state_as_update_v1(&yrs::StateVector::default())
}

/// Convert yrs Y.Doc state bytes to an editor document model.
pub fn ydoc_bytes_to_doc(bytes: &[u8]) -> Result<Node, BridgeError> {
    let ydoc = Doc::new();

    {
        let mut txn = ydoc.transact_mut();
        let update = Update::decode_v1(bytes)
            .map_err(|e| BridgeError(format!("decode error: {e}")))?;
        txn.apply_update(update)
            .map_err(|e| BridgeError(format!("apply error: {e}")))?;
    }

    let txn = ydoc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return Ok(Node::empty_doc());
    };

    let mut children = Vec::new();
    let len = fragment.len(&txn);

    for i in 0..len {
        if let Some(child) = fragment.get(&txn, i) {
            children.extend(read_xml_out(&txn, &child));
        }
    }

    if children.is_empty() {
        children.push(Node::element(NodeType::Paragraph));
    }

    Ok(Node::element_with_content(
        NodeType::Doc,
        Fragment::from(children),
    ))
}

#[derive(Debug, Clone)]
pub struct BridgeError(pub String);

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bridge error: {}", self.0)
    }
}

// ─── Write model to yrs ─────────────────────────────────────────

fn write_node_to_fragment(
    txn: &mut yrs::TransactionMut<'_>,
    fragment: &yrs::XmlFragmentRef,
    index: u32,
    node: &Node,
) {
    match node {
        Node::Text { text, marks } => {
            let text_ref = fragment.insert(txn, index, XmlTextPrelim::new(text));
            if !marks.is_empty() {
                let attrs = marks_to_attrs(marks);
                text_ref.format(txn, 0, text.len() as u32, attrs);
            }
        }
        Node::Element {
            node_type,
            attrs,
            content,
            ..
        } => {
            let tag = node_type_to_tag(*node_type);
            let el = fragment.insert(txn, index, XmlElementPrelim::empty(tag));

            // Set attributes
            for (key, value) in attrs {
                el.insert_attribute(txn, key.as_str(), value.as_str());
            }

            // Write children
            for (i, child) in content.children.iter().enumerate() {
                write_node_to_element(txn, &el, i as u32, child);
            }
        }
    }
}

fn write_node_to_element(
    txn: &mut yrs::TransactionMut<'_>,
    element: &yrs::XmlElementRef,
    index: u32,
    node: &Node,
) {
    match node {
        Node::Text { text, marks } => {
            let text_ref = element.insert(txn, index, XmlTextPrelim::new(text));
            if !marks.is_empty() {
                let attrs = marks_to_attrs(marks);
                text_ref.format(txn, 0, text.len() as u32, attrs);
            }
        }
        Node::Element {
            node_type,
            attrs,
            content,
            ..
        } => {
            let tag = node_type_to_tag(*node_type);
            let el = element.insert(txn, index, XmlElementPrelim::empty(tag));

            for (key, value) in attrs {
                el.insert_attribute(txn, key.as_str(), value.as_str());
            }

            for (i, child) in content.children.iter().enumerate() {
                write_node_to_element(txn, &el, i as u32, child);
            }
        }
    }
}

// ─── Read yrs to model ──────────────────────────────────────────

fn read_xml_out<T: ReadTxn>(txn: &T, out: &XmlOut) -> Vec<Node> {
    match out {
        XmlOut::Element(el) => {
            let Some(node_type) = tag_to_node_type(&el.tag()) else {
                return vec![];
            };

            // Read attributes
            let mut attrs = node_type.default_attrs();
            for (key, value) in el.attributes(txn) {
                attrs.insert(key.to_string(), value.to_string());
            }

            // Read children
            let mut children = Vec::new();
            let len = el.len(txn);
            for i in 0..len {
                if let Some(child) = el.get(txn, i) {
                    children.extend(read_xml_out(txn, &child));
                }
            }

            vec![Node::element_with_attrs(
                node_type,
                attrs,
                Fragment::from(children),
            )]
        }
        XmlOut::Text(text) => {
            // Use diff() to get formatted text chunks with their marks
            let diffs: Vec<Diff<YChange>> = text.diff(txn, YChange::identity);
            let mut nodes = Vec::new();
            for diff in diffs {
                if let Out::Any(Any::String(s)) = &diff.insert {
                    let text_str: &str = s.as_ref();
                    if text_str.is_empty() {
                        continue;
                    }
                    let marks = diff
                        .attributes
                        .as_ref()
                        .map(|a| attrs_to_marks(a))
                        .unwrap_or_default();
                    if marks.is_empty() {
                        nodes.push(Node::text(text_str));
                    } else {
                        nodes.push(Node::text_with_marks(text_str, marks));
                    }
                }
            }
            nodes
        }
        _ => vec![],
    }
}

// ─── Tag mapping ────────────────────────────────────────────────

fn node_type_to_tag(nt: NodeType) -> &'static str {
    match nt {
        NodeType::Doc => "doc",
        NodeType::Paragraph => "paragraph",
        NodeType::Heading => "heading",
        NodeType::BulletList => "bullet_list",
        NodeType::OrderedList => "ordered_list",
        NodeType::ListItem => "list_item",
        NodeType::TaskList => "task_list",
        NodeType::TaskItem => "task_item",
        NodeType::Blockquote => "blockquote",
        NodeType::CodeBlock => "code_block",
        NodeType::HorizontalRule => "horizontal_rule",
        NodeType::HardBreak => "hard_break",
        NodeType::Image => "image",
    }
}

fn tag_to_node_type(tag: &str) -> Option<NodeType> {
    match tag {
        "doc" => Some(NodeType::Doc),
        "paragraph" => Some(NodeType::Paragraph),
        "heading" => Some(NodeType::Heading),
        "bullet_list" => Some(NodeType::BulletList),
        "ordered_list" => Some(NodeType::OrderedList),
        "list_item" => Some(NodeType::ListItem),
        "task_list" => Some(NodeType::TaskList),
        "task_item" => Some(NodeType::TaskItem),
        "blockquote" => Some(NodeType::Blockquote),
        "code_block" => Some(NodeType::CodeBlock),
        "horizontal_rule" => Some(NodeType::HorizontalRule),
        "hard_break" => Some(NodeType::HardBreak),
        "image" => Some(NodeType::Image),
        _ => None,
    }
}

// ─── Mark ↔ yrs attribute conversion ────────────────────────────

fn mark_type_to_attr(mt: MarkType) -> &'static str {
    match mt {
        MarkType::Bold => "bold",
        MarkType::Italic => "italic",
        MarkType::Underline => "underline",
        MarkType::Strike => "strike",
        MarkType::Code => "code",
        MarkType::Link => "link",
        MarkType::TextColor => "textColor",
        MarkType::Highlight => "highlight",
    }
}

fn attr_to_mark_type(attr: &str) -> Option<MarkType> {
    match attr {
        "bold" => Some(MarkType::Bold),
        "italic" => Some(MarkType::Italic),
        "underline" => Some(MarkType::Underline),
        "strike" => Some(MarkType::Strike),
        "code" => Some(MarkType::Code),
        "link" => Some(MarkType::Link),
        "textColor" => Some(MarkType::TextColor),
        "highlight" => Some(MarkType::Highlight),
        _ => None,
    }
}

/// Convert editor marks to yrs formatting attributes.
fn marks_to_attrs(marks: &[Mark]) -> Attrs {
    let mut attrs = Attrs::new();
    for mark in marks {
        let key = mark_type_to_attr(mark.mark_type);
        let value = match mark.mark_type {
            // Marks with attrs (link href, text color, highlight color) → JSON string
            MarkType::Link | MarkType::TextColor | MarkType::Highlight => {
                let json = serde_json::to_string(&mark.attrs).unwrap_or_else(|_| "{}".to_string());
                Any::String(Arc::from(json.as_str()))
            }
            // Simple boolean marks
            _ => Any::Bool(true),
        };
        attrs.insert(Arc::from(key), value);
    }
    attrs
}

/// Convert yrs formatting attributes back to editor marks.
fn attrs_to_marks(attrs: &Attrs) -> Vec<Mark> {
    let mut marks = Vec::new();
    for (key, value) in attrs {
        let key_str: &str = key.as_ref();
        if let Some(mark_type) = attr_to_mark_type(key_str) {
            match mark_type {
                MarkType::Link | MarkType::TextColor | MarkType::Highlight => {
                    if let Any::String(json) = value {
                        let parsed_attrs: HashMap<String, String> =
                            serde_json::from_str(json.as_ref()).unwrap_or_default();
                        let mut mark = Mark::new(mark_type);
                        mark.attrs = parsed_attrs;
                        marks.push(mark);
                    }
                }
                _ => {
                    if *value != Any::Null {
                        marks.push(Mark::new(mark_type));
                    }
                }
            }
        }
    }
    super::model::normalize_marks(&mut marks);
    marks
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn simple_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        )
    }

    #[test]
    fn roundtrip_simple_doc() {
        let doc = simple_doc();
        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();

        assert_eq!(restored.node_type(), Some(NodeType::Doc));
        assert_eq!(restored.child_count(), 1);
        let para = restored.child(0).unwrap();
        assert_eq!(para.node_type(), Some(NodeType::Paragraph));
        assert_eq!(para.text_content(), "Hello world");
    }

    #[test]
    fn roundtrip_heading() {
        let mut attrs = HashMap::new();
        attrs.insert("level".to_string(), "2".to_string());
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Heading,
                attrs,
                Fragment::from(vec![Node::text("Title")]),
            )]),
        );

        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();

        let heading = restored.child(0).unwrap();
        assert_eq!(heading.node_type(), Some(NodeType::Heading));
        assert_eq!(heading.attrs().get("level").unwrap(), "2");
        assert_eq!(heading.text_content(), "Title");
    }

    #[test]
    fn roundtrip_nested_list() {
        let doc = Node::element_with_content(
            NodeType::Doc,
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
        );

        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();

        let list = restored.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        let item = list.child(0).unwrap();
        assert_eq!(item.node_type(), Some(NodeType::ListItem));
        assert_eq!(item.text_content(), "item");
    }

    #[test]
    fn roundtrip_empty_doc() {
        let doc = Node::empty_doc();
        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();

        assert_eq!(restored.node_type(), Some(NodeType::Doc));
        assert!(restored.child_count() >= 1);
    }

    #[test]
    fn roundtrip_hr_and_image() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("before")]),
                ),
                Node::element(NodeType::HorizontalRule),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("after")]),
                ),
            ]),
        );

        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();

        assert_eq!(restored.child_count(), 3);
        assert_eq!(
            restored.child(1).unwrap().node_type(),
            Some(NodeType::HorizontalRule)
        );
    }

    #[test]
    fn roundtrip_bold_text() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks(
                    "bold text",
                    vec![Mark::new(MarkType::Bold)],
                )]),
            )]),
        );
        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        let para = restored.child(0).unwrap();
        let text = para.child(0).unwrap();
        assert_eq!(text.text_content(), "bold text");
        assert!(text.marks().iter().any(|m| m.mark_type == MarkType::Bold));
    }

    #[test]
    fn roundtrip_multiple_marks() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks(
                    "bold italic",
                    vec![Mark::new(MarkType::Bold), Mark::new(MarkType::Italic)],
                )]),
            )]),
        );
        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        let para = restored.child(0).unwrap();
        let text = para.child(0).unwrap();
        assert!(text.marks().iter().any(|m| m.mark_type == MarkType::Bold));
        assert!(text.marks().iter().any(|m| m.mark_type == MarkType::Italic));
    }

    #[test]
    fn roundtrip_link_with_href() {
        let link = Mark::new(MarkType::Link)
            .with_attr("href", "https://example.com")
            .with_attr("target", "_blank");
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks("click here", vec![link])]),
            )]),
        );
        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        let para = restored.child(0).unwrap();
        let text = para.child(0).unwrap();
        assert_eq!(text.text_content(), "click here");
        let link_mark = text.marks().iter().find(|m| m.mark_type == MarkType::Link).unwrap();
        assert_eq!(link_mark.attrs.get("href").unwrap(), "https://example.com");
        assert_eq!(link_mark.attrs.get("target").unwrap(), "_blank");
    }

    #[test]
    fn roundtrip_mixed_marks_and_plain() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text("plain "),
                    Node::text_with_marks("bold", vec![Mark::new(MarkType::Bold)]),
                    Node::text(" end"),
                ]),
            )]),
        );
        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        let para = restored.child(0).unwrap();
        // Should have 3 text nodes: plain, bold, plain
        assert_eq!(para.text_content(), "plain bold end");
        // Find the bold part
        let mut found_bold = false;
        for i in 0..para.child_count() {
            let child = para.child(i).unwrap();
            if child.text_content().contains("bold") {
                assert!(child.marks().iter().any(|m| m.mark_type == MarkType::Bold));
                found_bold = true;
            }
        }
        assert!(found_bold, "Should have a bold text node");
    }

    #[test]
    fn invalid_bytes_returns_error() {
        let result = ydoc_bytes_to_doc(&[0xFF, 0xFE]);
        assert!(result.is_err());
    }

    #[test]
    fn tag_roundtrip() {
        let types = [
            NodeType::Paragraph,
            NodeType::Heading,
            NodeType::BulletList,
            NodeType::OrderedList,
            NodeType::ListItem,
            NodeType::TaskList,
            NodeType::TaskItem,
            NodeType::Blockquote,
            NodeType::CodeBlock,
            NodeType::HorizontalRule,
            NodeType::HardBreak,
            NodeType::Image,
        ];

        for nt in &types {
            let tag = node_type_to_tag(*nt);
            let back = tag_to_node_type(tag);
            assert_eq!(back, Some(*nt), "roundtrip failed for {tag}");
        }
    }
}
