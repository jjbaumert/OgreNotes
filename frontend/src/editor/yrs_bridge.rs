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

    read_doc_from_ydoc(&ydoc)
}

/// Read the editor document model from an existing yrs Doc reference.
/// Unlike `ydoc_bytes_to_doc`, this does not create a new Doc or decode bytes --
/// it reads directly from the provided Doc's current state.
pub fn read_doc_from_ydoc(ydoc: &Doc) -> Result<Node, BridgeError> {
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

// ─── Incremental sync: model → existing yrs Doc ────────────────

/// Apply the editor model to an existing yrs Doc, writing only the changes.
/// This preserves the Doc's client_id and produces minimal incremental updates
/// via `observe_update_v1`.
pub fn sync_model_to_ydoc(ydoc: &Doc, new_doc: &Node) {
    let mut txn = ydoc.transact_mut();
    let fragment = txn.get_or_insert_xml_fragment("content");

    let new_children = match new_doc {
        Node::Element { content, .. } => &content.children,
        Node::Text { .. } => return,
    };

    sync_children(&mut txn, SyncContainer::Fragment(&fragment), new_children);
}

/// Wrapper enum so sync_children can operate on both XmlFragmentRef and XmlElementRef.
enum SyncContainer<'a> {
    Fragment(&'a yrs::XmlFragmentRef),
    Element(&'a yrs::XmlElementRef),
}

impl<'a> SyncContainer<'a> {
    fn len<T: ReadTxn>(&self, txn: &T) -> u32 {
        match self {
            SyncContainer::Fragment(f) => f.len(txn),
            SyncContainer::Element(e) => e.len(txn),
        }
    }

    fn get<T: ReadTxn>(&self, txn: &T, index: u32) -> Option<XmlOut> {
        match self {
            SyncContainer::Fragment(f) => f.get(txn, index),
            SyncContainer::Element(e) => e.get(txn, index),
        }
    }

    fn remove(&self, txn: &mut yrs::TransactionMut<'_>, index: u32) {
        match self {
            SyncContainer::Fragment(f) => f.remove(txn, index),
            SyncContainer::Element(e) => e.remove(txn, index),
        }
    }

    fn remove_range(&self, txn: &mut yrs::TransactionMut<'_>, index: u32, len: u32) {
        match self {
            SyncContainer::Fragment(f) => f.remove_range(txn, index, len),
            SyncContainer::Element(e) => e.remove_range(txn, index, len),
        }
    }

    fn insert_node(&self, txn: &mut yrs::TransactionMut<'_>, index: u32, node: &Node) {
        match self {
            SyncContainer::Fragment(f) => write_node_to_fragment(txn, f, index, node),
            SyncContainer::Element(e) => write_node_to_element(txn, e, index, node),
        }
    }
}

/// Sync a list of model children into a yrs container.
/// Uses blockId-based matching to minimize yrs operations.
fn sync_children(
    txn: &mut yrs::TransactionMut<'_>,
    container: SyncContainer<'_>,
    new_children: &[Node],
) {
    // Read current yrs children info for matching
    let yrs_len = container.len(txn);
    let mut yrs_blocks: Vec<YrsBlockInfo> = Vec::with_capacity(yrs_len as usize);
    for i in 0..yrs_len {
        if let Some(child) = container.get(txn, i) {
            yrs_blocks.push(YrsBlockInfo::from_xml_out(txn, &child));
        }
    }

    // Build a map from blockId -> yrs index for fast lookup
    let mut block_id_map: HashMap<String, usize> = HashMap::new();
    for (i, info) in yrs_blocks.iter().enumerate() {
        if let Some(ref bid) = info.block_id {
            block_id_map.insert(bid.clone(), i);
        }
    }

    // Track which yrs blocks have been matched (to know which to delete)
    let mut matched = vec![false; yrs_blocks.len()];
    // Build the target sequence: each entry is either a matched yrs index or a new node to insert
    let mut target: Vec<SyncAction> = Vec::with_capacity(new_children.len());

    for new_child in new_children {
        let new_bid = new_child.block_id().map(|s| s.to_string());
        let new_tag = new_child.node_type().map(node_type_to_tag);

        // Try to match by blockId first
        let matched_idx = if let Some(ref bid) = new_bid {
            block_id_map.get(bid).copied().filter(|&idx| !matched[idx])
        } else {
            None
        };

        if let Some(idx) = matched_idx {
            matched[idx] = true;
            target.push(SyncAction::Reuse { yrs_idx: idx, node: new_child });
        } else {
            // No blockId match. For atomic leaf nodes (HorizontalRule, HardBreak,
            // Image) that carry no text content and are structurally unique, try
            // matching by type. For everything else (paragraphs, headings, lists,
            // etc.), treat as a new insert to avoid assigning the wrong CRDT
            // identity when multiple blocks of the same type exist.
            let is_leaf_node = matches!(
                new_child.node_type(),
                Some(NodeType::HorizontalRule | NodeType::HardBreak | NodeType::Image)
            );
            let type_match = if new_bid.is_none() && is_leaf_node {
                yrs_blocks.iter().enumerate().find(|(i, info)| {
                    !matched[*i]
                        && info.block_id.is_none()
                        && info.tag.as_deref() == new_tag
                }).map(|(i, _)| i)
            } else {
                None
            };

            if let Some(idx) = type_match {
                matched[idx] = true;
                target.push(SyncAction::Reuse { yrs_idx: idx, node: new_child });
            } else {
                target.push(SyncAction::Insert { node: new_child });
            }
        }
    }

    // Delete unmatched blocks (reverse order to keep indices stable)
    for i in (0..yrs_blocks.len()).rev() {
        if !matched[i] {
            container.remove(txn, i as u32);
        }
    }

    // Compute new positions for matched blocks after deletions.
    let mut old_to_new_pos: HashMap<usize, u32> = HashMap::new();
    let mut pos = 0u32;
    for (i, was_matched) in matched.iter().enumerate() {
        if *was_matched {
            old_to_new_pos.insert(i, pos);
            pos += 1;
        }
    }

    // Check if matched blocks are already in their target order
    let matched_in_target_order: Vec<usize> = target.iter().filter_map(|action| {
        if let SyncAction::Reuse { yrs_idx, .. } = action { Some(*yrs_idx) } else { None }
    }).collect();

    let already_ordered = matched_in_target_order.windows(2).all(|w| w[0] < w[1]);

    if already_ordered {
        // Fast path: matched blocks are in order. Insert new blocks and update changed ones.
        let mut insert_offset = 0u32;
        for (target_idx, action) in target.iter().enumerate() {
            match action {
                SyncAction::Insert { node } => {
                    container.insert_node(txn, target_idx as u32, node);
                    insert_offset += 1;
                }
                SyncAction::Reuse { yrs_idx, node } => {
                    let current_pos = old_to_new_pos[yrs_idx] + insert_offset;
                    sync_block_content(txn, &container, current_pos, node);
                }
            }
        }
    } else {
        // Slow path: blocks are reordered. Clear and rewrite everything.
        let remaining = container.len(txn);
        if remaining > 0 {
            container.remove_range(txn, 0, remaining);
        }
        for (i, action) in target.iter().enumerate() {
            let node = match action {
                SyncAction::Insert { node } => node,
                SyncAction::Reuse { node, .. } => node,
            };
            container.insert_node(txn, i as u32, node);
        }
    }
}

#[derive(Debug)]
enum SyncAction<'a> {
    Reuse { yrs_idx: usize, node: &'a Node },
    Insert { node: &'a Node },
}

/// Info about a yrs block for matching purposes.
#[derive(Debug)]
struct YrsBlockInfo {
    tag: Option<String>,
    block_id: Option<String>,
}

impl YrsBlockInfo {
    fn from_xml_out<T: ReadTxn>(txn: &T, out: &XmlOut) -> Self {
        match out {
            XmlOut::Element(el) => {
                let tag = Some(el.tag().to_string());
                let block_id = el.get_attribute(txn, "blockId");
                YrsBlockInfo { tag, block_id }
            }
            XmlOut::Text(_) => YrsBlockInfo { tag: None, block_id: None },
            _ => YrsBlockInfo { tag: None, block_id: None },
        }
    }
}

/// Update the content of an existing yrs block to match the model node.
fn sync_block_content(
    txn: &mut yrs::TransactionMut<'_>,
    container: &SyncContainer<'_>,
    pos: u32,
    model_node: &Node,
) {
    let Some(yrs_out) = container.get(txn, pos) else { return };
    let XmlOut::Element(ref el) = yrs_out else { return };

    let Node::Element { attrs: model_attrs, content: model_content, node_type: model_type, .. } = model_node else {
        return;
    };

    // Check if tag changed (e.g. paragraph -> heading). If so, replace entirely.
    let yrs_tag = el.tag().to_string();
    let model_tag = node_type_to_tag(*model_type);
    if yrs_tag != model_tag {
        container.remove(txn, pos);
        container.insert_node(txn, pos, model_node);
        return;
    }

    // Sync attributes
    let mut yrs_attrs: HashMap<String, String> = el.attributes(txn)
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    for (key, value) in model_attrs {
        if yrs_attrs.get(key) != Some(value) {
            el.insert_attribute(txn, key.as_str(), value.as_str());
        }
        yrs_attrs.remove(key);
    }
    for key in yrs_attrs.keys() {
        el.remove_attribute(txn, key);
    }

    // Container blocks (list, blockquote, list items): recurse into children
    let is_container = matches!(model_type,
        NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList |
        NodeType::Blockquote | NodeType::ListItem | NodeType::TaskItem
    );

    if is_container {
        sync_children(txn, SyncContainer::Element(el), &model_content.children);
    } else {
        // Leaf block (paragraph, heading, code_block, etc.)
        // Compare text content and marks; rewrite children if different.
        let yrs_text = collect_element_text(txn, el);
        let model_text = model_node.text_content();

        if yrs_text != model_text || !marks_match(txn, el, &model_content.children) {
            let child_len = el.len(txn);
            if child_len > 0 {
                el.remove_range(txn, 0, child_len);
            }
            for (i, child) in model_content.children.iter().enumerate() {
                write_node_to_element(txn, el, i as u32, child);
            }
        }
    }
}

/// Collect text content from a yrs XmlElement.
fn collect_element_text<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef) -> String {
    let mut result = String::new();
    let len = el.len(txn);
    for i in 0..len {
        if let Some(child) = el.get(txn, i) {
            collect_xml_out_text(txn, &child, &mut result);
        }
    }
    result
}

fn collect_xml_out_text<T: ReadTxn>(txn: &T, out: &XmlOut, buf: &mut String) {
    match out {
        XmlOut::Text(text) => {
            buf.push_str(&text.get_string(txn));
        }
        XmlOut::Element(el) => {
            let len = el.len(txn);
            for i in 0..len {
                if let Some(child) = el.get(txn, i) {
                    collect_xml_out_text(txn, &child, buf);
                }
            }
        }
        _ => {}
    }
}

/// Check if the marks of yrs children match the model children.
fn marks_match<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef, model_children: &[Node]) -> bool {
    use yrs::types::text::{Diff, YChange};

    let yrs_len = el.len(txn);
    let mut yrs_chunks: Vec<(String, Vec<Mark>)> = Vec::new();
    for i in 0..yrs_len {
        if let Some(XmlOut::Text(text)) = el.get(txn, i) {
            let diffs: Vec<Diff<YChange>> = text.diff(txn, YChange::identity);
            for diff in diffs {
                if let Out::Any(Any::String(s)) = &diff.insert {
                    let text_str: &str = s.as_ref();
                    if text_str.is_empty() { continue; }
                    let marks = diff.attributes.as_ref()
                        .map(|a| attrs_to_marks(a))
                        .unwrap_or_default();
                    yrs_chunks.push((text_str.to_string(), marks));
                }
            }
        } else {
            yrs_chunks.push(("".to_string(), vec![]));
        }
    }

    let mut model_chunks: Vec<(&str, &[Mark])> = Vec::new();
    for child in model_children {
        match child {
            Node::Text { text, marks } => {
                if !text.is_empty() {
                    model_chunks.push((text.as_str(), marks.as_slice()));
                }
            }
            Node::Element { .. } => {
                model_chunks.push(("", &[]));
            }
        }
    }

    if yrs_chunks.len() != model_chunks.len() {
        return false;
    }

    for (yrs_chunk, model_chunk) in yrs_chunks.iter().zip(model_chunks.iter()) {
        if yrs_chunk.0 != model_chunk.0 {
            return false;
        }
        if yrs_chunk.1.len() != model_chunk.1.len() {
            return false;
        }
        for (ym, mm) in yrs_chunk.1.iter().zip(model_chunk.1.iter()) {
            if ym.mark_type != mm.mark_type || ym.attrs != mm.attrs {
                return false;
            }
        }
    }

    true
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
    fn read_doc_from_ydoc_matches_bytes_roundtrip() {
        let doc = simple_doc();
        let bytes = doc_to_ydoc_bytes(&doc);

        // Build a ydoc from bytes the old way
        let ydoc = Doc::new();
        {
            let mut txn = ydoc.transact_mut();
            let update = Update::decode_v1(&bytes).unwrap();
            txn.apply_update(update).unwrap();
        }

        // read_doc_from_ydoc should produce the same result as ydoc_bytes_to_doc
        let from_ydoc = read_doc_from_ydoc(&ydoc).unwrap();
        let from_bytes = ydoc_bytes_to_doc(&bytes).unwrap();

        assert_eq!(from_ydoc.node_type(), from_bytes.node_type());
        assert_eq!(from_ydoc.child_count(), from_bytes.child_count());
        assert_eq!(from_ydoc.text_content(), from_bytes.text_content());
    }

    #[test]
    fn read_doc_from_ydoc_with_marks() {
        let link = Mark::new(MarkType::Link)
            .with_attr("href", "https://example.com");
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text("plain "),
                    Node::text_with_marks("bold", vec![Mark::new(MarkType::Bold)]),
                    Node::text_with_marks("link", vec![link]),
                ]),
            )]),
        );
        let bytes = doc_to_ydoc_bytes(&doc);

        let ydoc = Doc::new();
        {
            let mut txn = ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }

        let restored = read_doc_from_ydoc(&ydoc).unwrap();
        let para = restored.child(0).unwrap();
        assert_eq!(para.text_content(), "plain boldlink");
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

    // ─── sync_model_to_ydoc tests ──────────────────────────────

    /// Helper: create a ydoc from a model, then sync a new model to it,
    /// and verify the ydoc state matches the new model.
    fn assert_sync_roundtrip(initial: &Node, updated: &Node) {
        let ydoc = Doc::new();
        // Initialize with initial model
        sync_model_to_ydoc(&ydoc, initial);
        let read_back = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(read_back.text_content(), initial.text_content(),
            "initial sync failed");

        // Sync updated model
        sync_model_to_ydoc(&ydoc, updated);
        let read_back = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(read_back.text_content(), updated.text_content(),
            "updated sync failed");
        assert_eq!(read_back.child_count(), updated.child_count(),
            "child count mismatch after sync");
    }

    #[test]
    fn sync_initial_write() {
        let ydoc = Doc::new();
        let doc = simple_doc();
        sync_model_to_ydoc(&ydoc, &doc);
        let restored = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(restored.text_content(), "Hello world");
        assert_eq!(restored.child_count(), 1);
    }

    #[test]
    fn sync_modify_text() {
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let updated = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        );
        assert_sync_roundtrip(&initial, &updated);
    }

    #[test]
    fn sync_add_block() {
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("first")]),
            )]),
        );
        let updated = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b1".into())].into(),
                    Fragment::from(vec![Node::text("first")]),
                ),
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b2".into())].into(),
                    Fragment::from(vec![Node::text("second")]),
                ),
            ]),
        );
        assert_sync_roundtrip(&initial, &updated);
    }

    #[test]
    fn sync_remove_block() {
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b1".into())].into(),
                    Fragment::from(vec![Node::text("first")]),
                ),
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b2".into())].into(),
                    Fragment::from(vec![Node::text("second")]),
                ),
            ]),
        );
        let updated = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("first")]),
            )]),
        );
        assert_sync_roundtrip(&initial, &updated);
    }

    #[test]
    fn sync_change_marks() {
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("hello")]),
            )]),
        );
        let updated = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text_with_marks(
                    "hello",
                    vec![Mark::new(MarkType::Bold)],
                )]),
            )]),
        );

        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &initial);
        sync_model_to_ydoc(&ydoc, &updated);
        let restored = read_doc_from_ydoc(&ydoc).unwrap();
        let para = restored.child(0).unwrap();
        let text = para.child(0).unwrap();
        assert!(text.marks().iter().any(|m| m.mark_type == MarkType::Bold));
    }

    #[test]
    fn sync_change_block_type() {
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("title")]),
            )]),
        );
        // Same blockId but now a heading
        let updated = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Heading,
                [("blockId".into(), "b1".into()), ("level".into(), "1".into())].into(),
                Fragment::from(vec![Node::text("title")]),
            )]),
        );

        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &initial);
        sync_model_to_ydoc(&ydoc, &updated);
        let restored = read_doc_from_ydoc(&ydoc).unwrap();
        let block = restored.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::Heading));
        assert_eq!(block.attrs().get("level").unwrap(), "1");
        assert_eq!(block.text_content(), "title");
    }

    #[test]
    fn sync_nested_list() {
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("item 1")]),
                    )]),
                )]),
            )]),
        );
        let updated = Node::element_with_content(
            NodeType::Doc,
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
        );
        assert_sync_roundtrip(&initial, &updated);
    }

    #[test]
    fn sync_no_change_produces_no_update() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let ydoc = Doc::new();
        let doc = simple_doc();

        // Initial write
        sync_model_to_ydoc(&ydoc, &doc);

        // Register observer to count updates
        let update_count = Rc::new(RefCell::new(0u32));
        let count_ref = Rc::clone(&update_count);
        let _sub = ydoc.observe_update_v1(move |_txn, _event| {
            *count_ref.borrow_mut() += 1;
        }).unwrap();

        // Sync same doc again -- should produce no update
        sync_model_to_ydoc(&ydoc, &doc);
        assert_eq!(*update_count.borrow(), 0, "no-change sync should not fire observer");
    }

    #[test]
    fn sync_produces_valid_incremental_update() {
        // Create ydoc_a and write initial content
        let ydoc_a = Doc::new();
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        sync_model_to_ydoc(&ydoc_a, &initial);

        // Snapshot ydoc_a's state so ydoc_b shares the same CRDT history
        let pre_edit_state = {
            let txn = ydoc_a.transact();
            txn.encode_state_as_update_v1(&yrs::StateVector::default())
        };

        // Register observer to capture incremental updates from the edit
        let captured: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let cap_ref = Rc::clone(&captured);
        let _sub = ydoc_a.observe_update_v1(move |_txn, event| {
            cap_ref.borrow_mut().push(event.update.clone());
        }).unwrap();

        // Edit ydoc_a
        let updated = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        );
        sync_model_to_ydoc(&ydoc_a, &updated);

        let updates = captured.borrow();
        assert!(!updates.is_empty(), "should produce incremental updates");

        // Create ydoc_b from ydoc_a's pre-edit state, then apply incremental updates
        let ydoc_b = Doc::new();
        {
            let mut txn = ydoc_b.transact_mut();
            txn.apply_update(Update::decode_v1(&pre_edit_state).unwrap()).unwrap();
            for update_bytes in updates.iter() {
                txn.apply_update(Update::decode_v1(update_bytes).unwrap()).unwrap();
            }
        }

        // Both docs should converge to the same state
        let doc_a = read_doc_from_ydoc(&ydoc_a).unwrap();
        let doc_b = read_doc_from_ydoc(&ydoc_b).unwrap();
        assert_eq!(doc_a.text_content(), "Hello world");
        assert_eq!(doc_b.text_content(), "Hello world");
    }

    /// Mirrors the real CollabClient flow: initial state loaded via apply_update(bytes),
    /// then sync_model_to_ydoc called with a modified model.
    #[test]
    fn sync_after_apply_update_init() {
        // 1. Create initial content and serialize to bytes (like REST save does)
        let initial_model = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let initial_bytes = doc_to_ydoc_bytes(&initial_model);

        // 2. Load into a new Doc via apply_update (like CollabClient::new does)
        let ydoc = Doc::new();
        {
            let mut txn = ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap()).unwrap();
        }

        // 3. Register observer (like CollabClient::new does)
        let captured: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let cap_ref = Rc::clone(&captured);
        let _sub = ydoc.observe_update_v1(move |_txn, event| {
            cap_ref.borrow_mut().push(event.update.clone());
        }).unwrap();

        // 4. Verify no-change sync produces no updates
        sync_model_to_ydoc(&ydoc, &initial_model);
        assert_eq!(captured.borrow().len(), 0,
            "syncing unchanged model after apply_update init should produce no update");

        // 5. Edit the model (user adds text)
        let edited_model = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        );
        sync_model_to_ydoc(&ydoc, &edited_model);
        assert!(!captured.borrow().is_empty(),
            "syncing edited model should produce incremental updates");

        // 6. Verify the Doc has the new content
        let result = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(result.text_content(), "Hello world");

        // 7. Apply incremental updates to a second Doc (simulating server)
        let server_doc = Doc::new();
        {
            let mut txn = server_doc.transact_mut();
            // Server starts with same initial state
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap()).unwrap();
            // Apply incremental updates from client
            for update_bytes in captured.borrow().iter() {
                txn.apply_update(Update::decode_v1(update_bytes).unwrap()).unwrap();
            }
        }
        let server_result = read_doc_from_ydoc(&server_doc).unwrap();
        assert_eq!(server_result.text_content(), "Hello world",
            "server should converge to same state");
    }

    /// Test adding a new block after apply_update initialization.
    #[test]
    fn sync_add_block_after_apply_update_init() {
        let initial_model = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("First")]),
            )]),
        );
        let initial_bytes = doc_to_ydoc_bytes(&initial_model);

        let ydoc = Doc::new();
        {
            let mut txn = ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap()).unwrap();
        }

        let captured: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let cap_ref = Rc::clone(&captured);
        let _sub = ydoc.observe_update_v1(move |_txn, event| {
            cap_ref.borrow_mut().push(event.update.clone());
        }).unwrap();

        // Add a second paragraph
        let edited_model = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b1".into())].into(),
                    Fragment::from(vec![Node::text("First")]),
                ),
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b2".into())].into(),
                    Fragment::from(vec![Node::text("Second")]),
                ),
            ]),
        );
        sync_model_to_ydoc(&ydoc, &edited_model);

        assert!(!captured.borrow().is_empty(),
            "adding a block should produce updates");

        let result = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(result.child_count(), 2);
        assert_eq!(result.text_content(), "FirstSecond");
    }

    /// Verify that two Docs loaded from the same bytes produce equal Node trees.
    /// This matters because view.update_state skips re-render when doc == old_doc.
    #[test]
    fn two_docs_from_same_bytes_produce_equal_nodes() {
        let model = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Heading,
                    [("blockId".into(), "h1".into()), ("level".into(), "1".into())].into(),
                    Fragment::from(vec![Node::text("Title")]),
                ),
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "p1".into())].into(),
                    Fragment::from(vec![
                        Node::text("plain "),
                        Node::text_with_marks("bold", vec![Mark::new(MarkType::Bold)]),
                    ]),
                ),
            ]),
        );
        let bytes = doc_to_ydoc_bytes(&model);

        // Path 1: ydoc_bytes_to_doc (used by EditorComponent init)
        let doc_a = ydoc_bytes_to_doc(&bytes).unwrap();

        // Path 2: apply_update + read_doc_from_ydoc (used by CollabClient SyncStep2 callback)
        let ydoc = Doc::new();
        {
            let mut txn = ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }
        let doc_b = read_doc_from_ydoc(&ydoc).unwrap();

        assert_eq!(doc_a, doc_b, "both paths should produce identical Node trees");
    }

    /// Simulates a page refresh: snapshot + pending updates → full state → reload.
    /// Verifies no content duplication occurs.
    #[test]
    fn refresh_after_incremental_edits_no_duplication() {
        // 1. Original snapshot (like S3)
        let original = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let snapshot_bytes = doc_to_ydoc_bytes(&original);

        // 2. Client session: load snapshot, make edits, capture incremental updates
        let client_doc = Doc::new();
        {
            let mut txn = client_doc.transact_mut();
            txn.apply_update(Update::decode_v1(&snapshot_bytes).unwrap()).unwrap();
        }
        let captured: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let cap = Rc::clone(&captured);
        let _sub = client_doc.observe_update_v1(move |_txn, event| {
            cap.borrow_mut().push(event.update.clone());
        }).unwrap();

        // Make several edits (like typing + Enter)
        let edit1 = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        );
        sync_model_to_ydoc(&client_doc, &edit1);

        let edit2 = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b1".into())].into(),
                    Fragment::from(vec![Node::text("Hello world")]),
                ),
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b2".into())].into(),
                    Fragment::from(vec![Node::text("Second line")]),
                ),
            ]),
        );
        sync_model_to_ydoc(&client_doc, &edit2);

        // 3. Simulate REST get_content: snapshot + pending updates → full state bytes
        let server_doc = Doc::new();
        {
            let mut txn = server_doc.transact_mut();
            txn.apply_update(Update::decode_v1(&snapshot_bytes).unwrap()).unwrap();
            for update_bytes in captured.borrow().iter() {
                txn.apply_update(Update::decode_v1(update_bytes).unwrap()).unwrap();
            }
        }
        let refreshed_bytes = {
            let txn = server_doc.transact();
            txn.encode_state_as_update_v1(&yrs::StateVector::default())
        };

        // 4. Simulate page refresh: load from refreshed_bytes
        let refreshed_doc = ydoc_bytes_to_doc(&refreshed_bytes).unwrap();
        assert_eq!(refreshed_doc.child_count(), 2, "should have exactly 2 paragraphs");
        assert_eq!(refreshed_doc.child(0).unwrap().text_content(), "Hello world");
        assert_eq!(refreshed_doc.child(1).unwrap().text_content(), "Second line");

        // 5. Also check: sync_model_to_ydoc on the refreshed state produces no changes
        let ydoc2 = Doc::new();
        {
            let mut txn = ydoc2.transact_mut();
            txn.apply_update(Update::decode_v1(&refreshed_bytes).unwrap()).unwrap();
        }
        let captured2: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let cap2 = Rc::clone(&captured2);
        let _sub2 = ydoc2.observe_update_v1(move |_txn, event| {
            cap2.borrow_mut().push(event.update.clone());
        }).unwrap();

        sync_model_to_ydoc(&ydoc2, &refreshed_doc);
        assert_eq!(captured2.borrow().len(), 0,
            "syncing the refreshed doc against its own state should produce no updates");
    }

    use std::cell::RefCell;
    use std::rc::Rc;
}
