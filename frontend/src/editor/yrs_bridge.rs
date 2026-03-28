use yrs::{
    Doc, ReadTxn, Transact, WriteTxn,
    Update,
    updates::decoder::Decode,
    types::xml::{Xml, XmlElementPrelim, XmlFragment, XmlOut, XmlTextPrelim},
    types::GetString,
};

use super::model::{Fragment, Node, NodeType};

/// Convert an editor document model to yrs Y.Doc state bytes.
///
/// **MVP limitation:** Inline marks (bold, italic, link, etc.) are NOT preserved
/// in the yrs representation. Only document structure and text content survive
/// the roundtrip. Mark support will be added when the collab sync is fully wired.
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
            if let Some(node) = read_xml_out(&txn, &child) {
                children.push(node);
            }
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
            // Insert text into the fragment.
            // MVP: marks are not stored in yrs formatting attributes.
            // Full mark support requires yrs Text::format() with proper Attrs,
            // which will be added when the collab sync is fully wired.
            let _text_ref = fragment.insert(txn, index, XmlTextPrelim::new(text));
            let _ = marks;
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
            // Formatting via marks would use text_ref.format() in a full implementation
            let _ = text_ref;
            let _ = marks;
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

fn read_xml_out<T: ReadTxn>(txn: &T, out: &XmlOut) -> Option<Node> {
    match out {
        XmlOut::Element(el) => {
            let tag = el.tag();
            let node_type = tag_to_node_type(&tag)?;

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
                    if let Some(node) = read_xml_out(txn, &child) {
                        children.push(node);
                    }
                }
            }

            Some(Node::element_with_attrs(
                node_type,
                attrs,
                Fragment::from(children),
            ))
        }
        XmlOut::Text(text) => {
            let content = text.get_string(txn);
            if content.is_empty() {
                return None;
            }
            // For MVP, text nodes from yrs don't carry mark formatting
            // (full mark support requires reading yrs text formatting attributes)
            Some(Node::text(&content))
        }
        _ => None,
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

// mark_type_to_attr will be needed when yrs text formatting is implemented.
// For now, marks are not preserved in the yrs bridge (MVP limitation).
#[allow(dead_code)]
fn mark_type_to_attr(mt: super::model::MarkType) -> &'static str {
    match mt {
        super::model::MarkType::Bold => "bold",
        super::model::MarkType::Italic => "italic",
        super::model::MarkType::Underline => "underline",
        super::model::MarkType::Strike => "strike",
        super::model::MarkType::Code => "code",
        super::model::MarkType::Link => "link",
    }
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
