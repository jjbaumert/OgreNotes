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
                write_node(&mut txn, &fragment, i as u32, child);
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

    let doc = Node::element_with_content(NodeType::Doc, Fragment::from(children));
    Ok(super::model::normalize_doc(&doc))
}

#[derive(Debug, Clone)]
pub struct BridgeError(pub String);

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bridge error: {}", self.0)
    }
}

/// An action to take for each model child during sync.
#[derive(Debug)]
enum SyncAction<'a> {
    /// Reuse an existing yrs block at `yrs_idx`, updating its content.
    Reuse { yrs_idx: usize, node: &'a Node },
    /// Insert a new yrs block from this model node.
    Insert { node: &'a Node },
}

impl<'a> SyncAction<'a> {
    fn node(&self) -> &'a Node {
        match self {
            SyncAction::Reuse { node, .. } | SyncAction::Insert { node } => node,
        }
    }
}

/// Snapshot of a yrs block's identity for matching purposes.
#[derive(Debug)]
struct YrsBlockInfo {
    tag: Option<String>,
    block_id: Option<String>,
}

impl YrsBlockInfo {
    fn from_xml_out<T: ReadTxn>(txn: &T, out: &XmlOut) -> Self {
        match out {
            XmlOut::Element(el) => YrsBlockInfo {
                tag: Some(el.tag().to_string()),
                block_id: el.get_attribute(txn, "blockId"),
            },
            _ => YrsBlockInfo { tag: None, block_id: None },
        }
    }
}

// ─── Write model to yrs ─────────────────────────────────────────

fn write_node<C: XmlFragment>(
    txn: &mut yrs::TransactionMut<'_>,
    container: &C,
    index: u32,
    node: &Node,
) {
    match node {
        Node::Text { text, marks } => {
            let text_ref = container.insert(txn, index, XmlTextPrelim::new(text));
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
            let el = container.insert(txn, index, XmlElementPrelim::empty(tag));

            for (key, value) in attrs {
                el.insert_attribute(txn, key.as_str(), value.as_str());
            }

            for (i, child) in content.children.iter().enumerate() {
                write_node(txn, &el, i as u32, child);
            }
        }
    }
}

// ─── Incremental sync: model → existing yrs Doc ────────────────

/// Apply the editor model to an existing yrs Doc, writing only the changes.
/// This preserves the Doc's client_id and produces minimal incremental updates
/// via `observe_update_v1`.
pub fn sync_model_to_ydoc(ydoc: &Doc, new_doc: &Node) {
    let normalized = super::model::normalize_doc(new_doc);
    let mut txn = ydoc.transact_mut();
    let fragment = txn.get_or_insert_xml_fragment("content");

    let new_children = match &normalized {
        Node::Element { content, .. } => &content.children,
        Node::Text { .. } => return,
    };

    sync_children(&mut txn, &fragment, new_children);
}

/// Sync a list of model children into a yrs container.
/// Uses blockId-based matching to minimize yrs operations.
fn sync_children<C: XmlFragment>(
    txn: &mut yrs::TransactionMut<'_>,
    container: &C,
    new_children: &[Node],
) {
    let (actions, matched) = match_children(txn, container, new_children);
    remove_unmatched(txn, container, &matched);
    apply_actions(txn, container, &actions, &matched);
}

/// Match model children to existing yrs blocks by blockId (or tag for leaf atoms).
/// Returns a list of SyncActions and a bitmask of which yrs blocks were matched.
fn match_children<'a, C: XmlFragment>(
    txn: &yrs::TransactionMut<'_>,
    container: &C,
    new_children: &'a [Node],
) -> (Vec<SyncAction<'a>>, Vec<bool>) {
    let yrs_len = container.len(txn);
    let mut yrs_blocks: Vec<YrsBlockInfo> = Vec::with_capacity(yrs_len as usize);
    for i in 0..yrs_len {
        if let Some(child) = container.get(txn, i) {
            yrs_blocks.push(YrsBlockInfo::from_xml_out(txn, &child));
        }
    }

    let mut block_id_map: HashMap<String, usize> = HashMap::new();
    for (i, info) in yrs_blocks.iter().enumerate() {
        if let Some(ref bid) = info.block_id {
            block_id_map.insert(bid.clone(), i);
        }
    }

    let mut matched = vec![false; yrs_blocks.len()];
    let mut actions: Vec<SyncAction> = Vec::with_capacity(new_children.len());

    for new_child in new_children {
        if let Some(idx) = find_match(new_child, &block_id_map, &yrs_blocks, &matched) {
            matched[idx] = true;
            actions.push(SyncAction::Reuse { yrs_idx: idx, node: new_child });
        } else {
            actions.push(SyncAction::Insert { node: new_child });
        }
    }

    (actions, matched)
}

/// Try to find a matching yrs block for a model node.
/// Matches by blockId first, then falls back to tag-matching for leaf atoms
/// (HorizontalRule, HardBreak, Image) that have no blockId.
fn find_match(
    node: &Node,
    block_id_map: &HashMap<String, usize>,
    yrs_blocks: &[YrsBlockInfo],
    matched: &[bool],
) -> Option<usize> {
    let bid = node.block_id().map(|s| s.to_string());
    let tag = node.node_type().map(node_type_to_tag);

    // Try blockId match first
    if let Some(ref bid) = bid {
        if let Some(&idx) = block_id_map.get(bid) {
            if !matched[idx] {
                return Some(idx);
            }
        }
    }

    // For atomic leaf nodes without a blockId, try matching by tag.
    // Non-leaf blocks (paragraphs, headings, lists) are always inserted fresh
    // to avoid assigning the wrong CRDT identity when multiple blocks share a type.
    let is_leaf = matches!(
        node.node_type(),
        Some(NodeType::HorizontalRule | NodeType::HardBreak | NodeType::Image)
    );
    if bid.is_none() && is_leaf {
        return yrs_blocks.iter().enumerate().find(|(i, info)| {
            !matched[*i] && info.block_id.is_none() && info.tag.as_deref() == tag
        }).map(|(i, _)| i);
    }

    None
}

/// Remove yrs blocks that weren't matched to any model node.
/// Iterates in reverse to keep indices stable during removal.
fn remove_unmatched<C: XmlFragment>(
    txn: &mut yrs::TransactionMut<'_>,
    container: &C,
    matched: &[bool],
) {
    for i in (0..matched.len()).rev() {
        if !matched[i] {
            container.remove(txn, i as u32);
        }
    }
}

/// Write the matched/new actions to the yrs container.
/// Uses a fast path when matched blocks are already in their target order,
/// falls back to clearing and rewriting everything when blocks were reordered.
fn apply_actions<C: XmlFragment>(
    txn: &mut yrs::TransactionMut<'_>,
    container: &C,
    actions: &[SyncAction],
    matched: &[bool],
) {
    // Compute where each matched block ended up after deletions
    let mut old_to_new_pos: HashMap<usize, u32> = HashMap::new();
    let mut pos = 0u32;
    for (i, was_matched) in matched.iter().enumerate() {
        if *was_matched {
            old_to_new_pos.insert(i, pos);
            pos += 1;
        }
    }

    let reused_indices: Vec<usize> = actions.iter().filter_map(|a| {
        if let SyncAction::Reuse { yrs_idx, .. } = a { Some(*yrs_idx) } else { None }
    }).collect();

    let already_ordered = reused_indices.windows(2).all(|w| w[0] < w[1]);

    if already_ordered {
        // Fast path: matched blocks are in order. Insert new blocks and update changed ones.
        let mut insert_offset = 0u32;
        for (target_idx, action) in actions.iter().enumerate() {
            match action {
                SyncAction::Insert { node } => {
                    write_node(txn, container, target_idx as u32, node);
                    insert_offset += 1;
                }
                SyncAction::Reuse { yrs_idx, node } => {
                    let current_pos = old_to_new_pos[yrs_idx] + insert_offset;
                    sync_block_content(txn, container, current_pos, node);
                }
            }
        }
    } else {
        // Slow path: blocks are reordered. Clear and rewrite everything.
        let remaining = container.len(txn);
        if remaining > 0 {
            container.remove_range(txn, 0, remaining);
        }
        for (i, action) in actions.iter().enumerate() {
            write_node(txn, container, i as u32, action.node());
        }
    }
}

/// Update the content of an existing yrs block to match the model node.
fn sync_block_content<C: XmlFragment>(
    txn: &mut yrs::TransactionMut<'_>,
    container: &C,
    pos: u32,
    model_node: &Node,
) {
    let Some(yrs_out) = container.get(txn, pos) else { return };
    let XmlOut::Element(ref el) = yrs_out else { return };

    let Node::Element { attrs: model_attrs, content: model_content, node_type: model_type, .. } = model_node else {
        return;
    };

    // Tag changed (e.g. paragraph -> heading) → replace entirely
    if el.tag().as_ref() != node_type_to_tag(*model_type) {
        container.remove(txn, pos);
        write_node(txn, container, pos, model_node);
        return;
    }

    sync_attrs(txn, el, model_attrs);

    // Container blocks recurse into children; leaf blocks compare text + marks
    let is_container = matches!(model_type,
        NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList |
        NodeType::Blockquote | NodeType::ListItem | NodeType::TaskItem
    );

    if is_container {
        sync_children(txn, el, &model_content.children);
    } else if !text_and_marks_match(txn, el, model_node, &model_content.children) {
        replace_children(txn, el, &model_content.children);
    }
}

/// Sync element attributes: add/update those in the model, remove stale ones.
fn sync_attrs(
    txn: &mut yrs::TransactionMut<'_>,
    el: &yrs::XmlElementRef,
    model_attrs: &HashMap<String, String>,
) {
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
}

/// Check if a leaf block's text content and marks match the model.
fn text_and_marks_match(
    txn: &yrs::TransactionMut<'_>,
    el: &yrs::XmlElementRef,
    model_node: &Node,
    model_children: &[Node],
) -> bool {
    collect_element_text(txn, el) == model_node.text_content()
        && marks_match(txn, el, model_children)
}

/// Replace all children of a yrs element with the given model nodes.
fn replace_children(
    txn: &mut yrs::TransactionMut<'_>,
    el: &yrs::XmlElementRef,
    children: &[Node],
) {
    let len = el.len(txn);
    if len > 0 {
        el.remove_range(txn, 0, len);
    }
    for (i, child) in children.iter().enumerate() {
        write_node(txn, el, i as u32, child);
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
    // Collect yrs formatted chunks
    let mut yrs_chunks: Vec<(String, Vec<Mark>)> = Vec::new();
    for i in 0..el.len(txn) {
        if let Some(XmlOut::Text(text)) = el.get(txn, i) {
            for diff in text.diff(txn, YChange::identity) {
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
            yrs_chunks.push((String::new(), vec![]));
        }
    }

    // Collect model chunks and compare in one pass
    let model_chunks: Vec<(&str, &[Mark])> = model_children.iter().filter_map(|child| {
        match child {
            Node::Text { text, marks } if !text.is_empty() => Some((text.as_str(), marks.as_slice())),
            Node::Element { .. } => Some(("", [].as_slice())),
            _ => None,
        }
    }).collect();

    yrs_chunks.len() == model_chunks.len()
        && yrs_chunks.iter().zip(model_chunks.iter()).all(|(yrs, model)| {
            yrs.0 == model.0 && yrs.1 == model.1
        })
}

// ─── Read yrs to model ──────────────────────────────────────────

fn read_xml_out<T: ReadTxn>(txn: &T, out: &XmlOut) -> Vec<Node> {
    match out {
        XmlOut::Element(el) => read_element(txn, el),
        XmlOut::Text(text) => read_text_diffs(txn, text),
        _ => vec![],
    }
}

/// Read a yrs XmlElement into an editor Node, recursing into children.
fn read_element<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef) -> Vec<Node> {
    let Some(node_type) = tag_to_node_type(&el.tag()) else {
        return vec![];
    };

    let mut attrs = node_type.default_attrs();
    for (key, value) in el.attributes(txn) {
        attrs.insert(key.to_string(), value.to_string());
    }

    let mut children = Vec::new();
    for i in 0..el.len(txn) {
        if let Some(child) = el.get(txn, i) {
            children.extend(read_xml_out(txn, &child));
        }
    }

    vec![Node::element_with_attrs(node_type, attrs, Fragment::from(children))]
}

/// Read a yrs XmlText into editor text Nodes, preserving formatting marks.
fn read_text_diffs<T: ReadTxn>(txn: &T, text: &yrs::XmlTextRef) -> Vec<Node> {
    let mut nodes = Vec::new();
    for diff in text.diff(txn, YChange::identity) {
        if let Out::Any(Any::String(s)) = &diff.insert {
            let text_str: &str = s.as_ref();
            if text_str.is_empty() {
                continue;
            }
            let marks = diff.attributes.as_ref()
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
        NodeType::Table => "table",
        NodeType::TableRow => "table_row",
        NodeType::TableCell => "table_cell",
        NodeType::TableHeader => "table_header",
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
        "table" => Some(NodeType::Table),
        "table_row" => Some(NodeType::TableRow),
        "table_cell" => Some(NodeType::TableCell),
        "table_header" => Some(NodeType::TableHeader),
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
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

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

    // ─── Multi-client simulation helpers ──────────────────────────

    struct ClientPair {
        doc_a: Doc,
        doc_b: Doc,
        updates_a: Rc<RefCell<Vec<Vec<u8>>>>,
        updates_b: Rc<RefCell<Vec<Vec<u8>>>>,
        _sub_a: yrs::Subscription,
        _sub_b: yrs::Subscription,
    }

    fn make_client_pair(initial: &Node) -> ClientPair {
        let bytes = doc_to_ydoc_bytes(initial);

        let doc_a = Doc::with_client_id(1);
        {
            let mut txn = doc_a.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }

        let doc_b = Doc::with_client_id(2);
        {
            let mut txn = doc_b.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }

        let updates_a = Rc::new(RefCell::new(Vec::new()));
        let updates_b = Rc::new(RefCell::new(Vec::new()));

        let cap_a = Rc::clone(&updates_a);
        let _sub_a = doc_a.observe_update_v1(move |_, event| {
            cap_a.borrow_mut().push(event.update.clone());
        }).unwrap();

        let cap_b = Rc::clone(&updates_b);
        let _sub_b = doc_b.observe_update_v1(move |_, event| {
            cap_b.borrow_mut().push(event.update.clone());
        }).unwrap();

        ClientPair { doc_a, doc_b, updates_a, updates_b, _sub_a, _sub_b }
    }

    fn exchange_updates(pair: &ClientPair) {
        let a_updates: Vec<Vec<u8>> = pair.updates_a.borrow().clone();
        let b_updates: Vec<Vec<u8>> = pair.updates_b.borrow().clone();

        {
            let mut txn = pair.doc_b.transact_mut();
            for u in &a_updates {
                txn.apply_update(Update::decode_v1(u).unwrap()).unwrap();
            }
        }
        {
            let mut txn = pair.doc_a.transact_mut();
            for u in &b_updates {
                txn.apply_update(Update::decode_v1(u).unwrap()).unwrap();
            }
        }
    }

    fn assert_convergence(pair: &ClientPair) -> (Node, Node) {
        let a = read_doc_from_ydoc(&pair.doc_a).unwrap();
        let b = read_doc_from_ydoc(&pair.doc_b).unwrap();
        assert_eq!(a, b, "CRDT convergence failed: docs differ after update exchange");
        (a, b)
    }

    fn make_para(block_id: &str, text: &str) -> Node {
        Node::element_with_attrs(
            NodeType::Paragraph,
            [("blockId".into(), block_id.into())].into(),
            Fragment::from(vec![Node::text(text)]),
        )
    }

    fn make_doc(children: Vec<Node>) -> Node {
        Node::element_with_content(NodeType::Doc, Fragment::from(children))
    }

    // ─── Single-user edge case tests ───────────────────────────────

    #[test]
    fn sync_rapid_sequential_edits() {
        let ydoc = Doc::new();
        let initial = make_doc(vec![make_para("b1", "")]);
        sync_model_to_ydoc(&ydoc, &initial);

        let update_count = Rc::new(RefCell::new(0u32));
        let count_ref = Rc::clone(&update_count);
        let _sub = ydoc.observe_update_v1(move |_, _| {
            *count_ref.borrow_mut() += 1;
        }).unwrap();

        for i in 1..=10 {
            let text: String = (b'A'..b'A' + i as u8).map(|c| c as char).collect();
            let model = make_doc(vec![make_para("b1", &text)]);
            sync_model_to_ydoc(&ydoc, &model);
        }

        let result = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(result.child(0).unwrap().text_content(), "ABCDEFGHIJ");
        assert_eq!(*update_count.borrow(), 10, "each sync should fire exactly 1 update");
    }

    #[test]
    fn sync_block_reorder_slow_path() {
        let ydoc = Doc::new();
        let initial = make_doc(vec![
            make_para("b1", "first"),
            make_para("b2", "second"),
            make_para("b3", "third"),
        ]);
        sync_model_to_ydoc(&ydoc, &initial);

        // Reorder: [b3, b1, b2]
        let reordered = make_doc(vec![
            make_para("b3", "third"),
            make_para("b1", "first"),
            make_para("b2", "second"),
        ]);
        sync_model_to_ydoc(&ydoc, &reordered);

        let result = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(result.child_count(), 3);
        assert_eq!(result.child(0).unwrap().text_content(), "third");
        assert_eq!(result.child(1).unwrap().text_content(), "first");
        assert_eq!(result.child(2).unwrap().text_content(), "second");
    }

    #[test]
    fn roundtrip_unicode_and_emoji() {
        let text = "Hello \u{1F30D}\u{1F44B}\u{1F3FD} caf\u{00E9} \u{00FC}\u{00F6}\u{00E4} \u{4F60}\u{597D} \u{0410}\u{043B}\u{0438}\u{0441}\u{0430}";
        let doc = make_doc(vec![make_para("b1", text)]);
        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        assert_eq!(restored.child(0).unwrap().text_content(), text);

        // Also test incremental sync with emoji appended
        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &doc);
        let updated = make_doc(vec![make_para("b1", &format!("{text}\u{1F680}"))]);
        sync_model_to_ydoc(&ydoc, &updated);
        let result = read_doc_from_ydoc(&ydoc).unwrap();
        assert!(result.child(0).unwrap().text_content().ends_with("\u{1F680}"));
    }

    #[test]
    fn roundtrip_deeply_nested_structure() {
        let doc = make_doc(vec![
            Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::BulletList,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::BulletList,
                            Fragment::from(vec![Node::element_with_content(
                                NodeType::ListItem,
                                Fragment::from(vec![Node::element_with_content(
                                    NodeType::Paragraph,
                                    Fragment::from(vec![Node::text("deep")]),
                                )]),
                            )]),
                        )]),
                    )]),
                )]),
            ),
        ]);

        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        assert_eq!(restored.text_content(), "deep");

        // Modify innermost text via sync
        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &doc);
        // Rebuild with changed text
        let updated = make_doc(vec![
            Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::BulletList,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::BulletList,
                            Fragment::from(vec![Node::element_with_content(
                                NodeType::ListItem,
                                Fragment::from(vec![Node::element_with_content(
                                    NodeType::Paragraph,
                                    Fragment::from(vec![Node::text("deeper")]),
                                )]),
                            )]),
                        )]),
                    )]),
                )]),
            ),
        ]);
        sync_model_to_ydoc(&ydoc, &updated);
        let result = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(result.text_content(), "deeper");
    }

    #[test]
    fn sync_very_long_text() {
        let long_text = "a".repeat(100_000);
        let doc = make_doc(vec![make_para("b1", &long_text)]);
        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        assert_eq!(restored.child(0).unwrap().text_content().len(), 100_000);

        // Sync with one char changed in the middle
        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &doc);
        let mut modified = long_text.clone();
        modified.replace_range(50_000..50_001, "X");
        let updated = make_doc(vec![make_para("b1", &modified)]);
        sync_model_to_ydoc(&ydoc, &updated);
        let result = read_doc_from_ydoc(&ydoc).unwrap();
        let result_text = result.child(0).unwrap().text_content();
        assert_eq!(result_text.len(), 100_000);
        assert_eq!(&result_text[49_999..50_001], "aX");
    }

    #[test]
    fn roundtrip_empty_blocks() {
        let doc = make_doc(vec![
            Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "p1".into())].into(),
                Fragment::from(vec![]),
            ),
            Node::element_with_attrs(
                NodeType::Heading,
                [("blockId".into(), "h1".into()), ("level".into(), "1".into())].into(),
                Fragment::from(vec![]),
            ),
            Node::element_with_attrs(
                NodeType::CodeBlock,
                [("blockId".into(), "c1".into()), ("language".into(), "".into())].into(),
                Fragment::from(vec![]),
            ),
        ]);

        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        assert_eq!(restored.child_count(), 3);
        assert_eq!(restored.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
        assert_eq!(restored.child(1).unwrap().node_type(), Some(NodeType::Heading));
        assert_eq!(restored.child(2).unwrap().node_type(), Some(NodeType::CodeBlock));
        assert_eq!(restored.text_content(), "");
    }

    #[test]
    fn roundtrip_many_marks_on_same_text() {
        let marks = vec![
            Mark::new(MarkType::Bold),
            Mark::new(MarkType::Italic),
            Mark::new(MarkType::Underline),
            Mark::new(MarkType::Strike),
            Mark::new(MarkType::TextColor).with_attr("color", "#ff0000"),
            Mark::new(MarkType::Highlight).with_attr("color", "yellow"),
        ];
        let doc = make_doc(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text_with_marks("styled", marks.clone())]),
        )]);

        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        let text_node = restored.child(0).unwrap().child(0).unwrap();
        let restored_marks = text_node.marks();
        assert_eq!(restored_marks.len(), 6, "all 6 marks should survive roundtrip");
        assert!(restored_marks.iter().any(|m| m.mark_type == MarkType::Bold));
        assert!(restored_marks.iter().any(|m| m.mark_type == MarkType::Italic));
        assert!(restored_marks.iter().any(|m| m.mark_type == MarkType::Underline));
        assert!(restored_marks.iter().any(|m| m.mark_type == MarkType::Strike));
        assert!(restored_marks.iter().any(|m| m.mark_type == MarkType::TextColor
            && m.attrs.get("color").map(|s| s.as_str()) == Some("#ff0000")));
        assert!(restored_marks.iter().any(|m| m.mark_type == MarkType::Highlight
            && m.attrs.get("color").map(|s| s.as_str()) == Some("yellow")));
    }

    #[test]
    fn roundtrip_code_mark_exclusion() {
        let doc = make_doc(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![
                Node::text("before "),
                Node::text_with_marks("code", vec![Mark::new(MarkType::Code)]),
                Node::text(" after"),
            ]),
        )]);

        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        let para = restored.child(0).unwrap();
        assert_eq!(para.text_content(), "before code after");

        // Find the code-marked text node
        let mut found_code = false;
        for i in 0..para.child_count() {
            let child = para.child(i).unwrap();
            if child.text_content().contains("code") && !child.text_content().contains(" ") {
                assert_eq!(child.marks().len(), 1);
                assert_eq!(child.marks()[0].mark_type, MarkType::Code);
                found_code = true;
            }
        }
        assert!(found_code, "should have a code-marked text node");
    }

    // ─── Two-user concurrent edit tests ────────────────────────────

    #[test]
    fn concurrent_edit_different_blocks() {
        let initial = make_doc(vec![
            make_para("b1", "Hello"),
            make_para("b2", "World"),
        ]);
        let pair = make_client_pair(&initial);

        // A edits b1
        let model_a = make_doc(vec![
            make_para("b1", "Hello Alice"),
            make_para("b2", "World"),
        ]);
        sync_model_to_ydoc(&pair.doc_a, &model_a);

        // B edits b2
        let model_b = make_doc(vec![
            make_para("b1", "Hello"),
            make_para("b2", "World Bob"),
        ]);
        sync_model_to_ydoc(&pair.doc_b, &model_b);

        exchange_updates(&pair);
        let (result, _) = assert_convergence(&pair);

        // Both edits should be preserved since they target different blocks
        assert_eq!(result.child(0).unwrap().text_content(), "Hello Alice");
        assert_eq!(result.child(1).unwrap().text_content(), "World Bob");
    }

    #[test]
    fn concurrent_edit_same_block_text() {
        // Both clients rewrite the same paragraph's text.
        // sync_model_to_ydoc does a full children rewrite when text differs,
        // so this is an XML-level conflict, not char-level merge.
        let initial = make_doc(vec![make_para("b1", "Hello world")]);
        let pair = make_client_pair(&initial);

        let model_a = make_doc(vec![make_para("b1", "Hello beautiful world")]);
        sync_model_to_ydoc(&pair.doc_a, &model_a);

        let model_b = make_doc(vec![make_para("b1", "Hello world!")]);
        sync_model_to_ydoc(&pair.doc_b, &model_b);

        exchange_updates(&pair);
        let (result, _) = assert_convergence(&pair);

        // Primary assertion: convergence (A==B). Content depends on yrs conflict resolution.
        let text = result.child(0).unwrap().text_content();
        assert!(!text.is_empty(), "merged text should not be empty: got '{text}'");
    }

    #[test]
    fn concurrent_insert_same_position() {
        let initial = make_doc(vec![make_para("b1", "AB")]);
        let pair = make_client_pair(&initial);

        let model_a = make_doc(vec![make_para("b1", "AXB")]);
        sync_model_to_ydoc(&pair.doc_a, &model_a);

        let model_b = make_doc(vec![make_para("b1", "AYB")]);
        sync_model_to_ydoc(&pair.doc_b, &model_b);

        exchange_updates(&pair);
        let (result, _) = assert_convergence(&pair);

        let text = result.child(0).unwrap().text_content();
        assert!(!text.is_empty(), "merged text should not be empty: got '{text}'");
    }

    #[test]
    fn concurrent_delete_vs_edit() {
        let initial = make_doc(vec![
            make_para("b1", "Keep"),
            make_para("b2", "Delete me"),
        ]);
        let pair = make_client_pair(&initial);

        // A removes b2
        let model_a = make_doc(vec![make_para("b1", "Keep")]);
        sync_model_to_ydoc(&pair.doc_a, &model_a);

        // B edits b2
        let model_b = make_doc(vec![
            make_para("b1", "Keep"),
            make_para("b2", "Delete me edited"),
        ]);
        sync_model_to_ydoc(&pair.doc_b, &model_b);

        exchange_updates(&pair);
        let (result, _) = assert_convergence(&pair);

        // Convergence is the primary assertion.
        // The exact outcome depends on yrs conflict resolution for concurrent
        // remove vs modify of the same XML element.
        assert!(result.child_count() >= 1, "should have at least the kept block");
        assert_eq!(result.child(0).unwrap().text_content(), "Keep");
    }

    #[test]
    fn concurrent_type_change_vs_text_edit() {
        let initial = make_doc(vec![make_para("b1", "Hello")]);
        let pair = make_client_pair(&initial);

        // A converts paragraph -> heading
        let model_a = make_doc(vec![Node::element_with_attrs(
            NodeType::Heading,
            [("blockId".into(), "b1".into()), ("level".into(), "1".into())].into(),
            Fragment::from(vec![Node::text("Hello")]),
        )]);
        sync_model_to_ydoc(&pair.doc_a, &model_a);

        // B edits the text
        let model_b = make_doc(vec![make_para("b1", "Hello world")]);
        sync_model_to_ydoc(&pair.doc_b, &model_b);

        exchange_updates(&pair);
        let (result, _) = assert_convergence(&pair);

        // Convergence is primary. Both a type change (remove+insert) and text edit
        // happened concurrently on the same block.
        assert!(result.child_count() >= 1);
    }

    #[test]
    fn concurrent_add_blocks_same_position() {
        let initial = make_doc(vec![make_para("b1", "First")]);
        let pair = make_client_pair(&initial);

        // A appends a new paragraph
        let model_a = make_doc(vec![
            make_para("b1", "First"),
            make_para("ba", "From A"),
        ]);
        sync_model_to_ydoc(&pair.doc_a, &model_a);

        // B appends a new paragraph
        let model_b = make_doc(vec![
            make_para("b1", "First"),
            make_para("bb", "From B"),
        ]);
        sync_model_to_ydoc(&pair.doc_b, &model_b);

        exchange_updates(&pair);
        let (result, _) = assert_convergence(&pair);

        // Both insertions should be preserved
        assert_eq!(result.child_count(), 3, "should have original + both new blocks");
        let all_text = result.text_content();
        assert!(all_text.contains("First"));
        assert!(all_text.contains("From A"));
        assert!(all_text.contains("From B"));
    }

    #[test]
    fn concurrent_different_marks_same_text() {
        let initial = make_doc(vec![Node::element_with_attrs(
            NodeType::Paragraph,
            [("blockId".into(), "b1".into())].into(),
            Fragment::from(vec![Node::text("Hello")]),
        )]);
        let pair = make_client_pair(&initial);

        // A applies Bold
        let model_a = make_doc(vec![Node::element_with_attrs(
            NodeType::Paragraph,
            [("blockId".into(), "b1".into())].into(),
            Fragment::from(vec![Node::text_with_marks("Hello", vec![Mark::new(MarkType::Bold)])]),
        )]);
        sync_model_to_ydoc(&pair.doc_a, &model_a);

        // B applies Italic
        let model_b = make_doc(vec![Node::element_with_attrs(
            NodeType::Paragraph,
            [("blockId".into(), "b1".into())].into(),
            Fragment::from(vec![Node::text_with_marks("Hello", vec![Mark::new(MarkType::Italic)])]),
        )]);
        sync_model_to_ydoc(&pair.doc_b, &model_b);

        exchange_updates(&pair);
        let (result, _) = assert_convergence(&pair);

        // Both clients fully rewrite children when marks differ (remove_range +
        // write_node), so yrs sees two independent insertions → text may be
        // duplicated. Convergence (A==B) is the key assertion.
        let text = result.child(0).unwrap().text_content();
        assert!(text.contains("Hello"), "original text should be present: got '{text}'");
    }

    #[test]
    fn concurrent_split_vs_edit() {
        let initial = make_doc(vec![make_para("b1", "Hello world")]);
        let pair = make_client_pair(&initial);

        // A splits into two paragraphs (simulating Enter key)
        let model_a = make_doc(vec![
            make_para("b1", "Hello"),
            make_para("b2", "world"),
        ]);
        sync_model_to_ydoc(&pair.doc_a, &model_a);

        // B edits the original paragraph
        let model_b = make_doc(vec![make_para("b1", "Hello world!")]);
        sync_model_to_ydoc(&pair.doc_b, &model_b);

        exchange_updates(&pair);
        let (result, _) = assert_convergence(&pair);

        // Complex structural conflict. Convergence is the key assertion.
        assert!(result.child_count() >= 1);
    }

    #[test]
    fn out_of_order_update_delivery() {
        // Test that updates from different clients arrive at a third client
        // in different orders but still converge. This is the real-world
        // scenario: A and B edit concurrently, their updates reach C in
        // arbitrary order.
        let initial = make_doc(vec![
            make_para("b1", "Block 1"),
            make_para("b2", "Block 2"),
        ]);
        let bytes = doc_to_ydoc_bytes(&initial);

        let doc_a = Doc::with_client_id(1);
        let doc_b = Doc::with_client_id(2);
        {
            let mut txn = doc_a.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }
        {
            let mut txn = doc_b.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }

        let updates_a: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let updates_b: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let cap_a = Rc::clone(&updates_a);
        let cap_b = Rc::clone(&updates_b);
        let _sub_a = doc_a.observe_update_v1(move |_, e| { cap_a.borrow_mut().push(e.update.clone()); }).unwrap();
        let _sub_b = doc_b.observe_update_v1(move |_, e| { cap_b.borrow_mut().push(e.update.clone()); }).unwrap();

        // A edits b1, B edits b2 (concurrently)
        sync_model_to_ydoc(&doc_a, &make_doc(vec![
            make_para("b1", "Block 1 by A"), make_para("b2", "Block 2"),
        ]));
        sync_model_to_ydoc(&doc_b, &make_doc(vec![
            make_para("b1", "Block 1"), make_para("b2", "Block 2 by B"),
        ]));

        let ua = updates_a.borrow().clone();
        let ub = updates_b.borrow().clone();

        // Client C receives A first, then B
        let doc_c1 = Doc::with_client_id(3);
        {
            let mut txn = doc_c1.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
            for u in &ua { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
            for u in &ub { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
        }

        // Client D receives B first, then A (reversed order)
        let doc_c2 = Doc::with_client_id(4);
        {
            let mut txn = doc_c2.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
            for u in &ub { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
            for u in &ua { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
        }

        let result_c1 = read_doc_from_ydoc(&doc_c1).unwrap();
        let result_c2 = read_doc_from_ydoc(&doc_c2).unwrap();
        assert_eq!(result_c1, result_c2, "different delivery order should still converge");
    }

    #[test]
    fn stale_state_editing() {
        let initial = make_doc(vec![make_para("b1", "Version 0")]);
        let pair = make_client_pair(&initial);

        // A makes two sequential edits
        let model_a1 = make_doc(vec![make_para("b1", "Version 1")]);
        sync_model_to_ydoc(&pair.doc_a, &model_a1);
        let model_a2 = make_doc(vec![make_para("b1", "Version 2")]);
        sync_model_to_ydoc(&pair.doc_a, &model_a2);

        // B has NOT received A's updates yet. Edits from stale "Version 0" state.
        let model_b = make_doc(vec![make_para("b1", "Version 0 plus B")]);
        sync_model_to_ydoc(&pair.doc_b, &model_b);

        // Now exchange all updates
        exchange_updates(&pair);
        let (result, _) = assert_convergence(&pair);

        // Both clients must agree. The content depends on yrs conflict resolution
        // for concurrent full-text rewrites.
        assert!(!result.child(0).unwrap().text_content().is_empty());
    }

    // ─── Three-user test ───────────────────────────────────────────

    #[test]
    fn three_users_edit_different_blocks() {
        let initial = make_doc(vec![
            make_para("b1", "Block 1"),
            make_para("b2", "Block 2"),
            make_para("b3", "Block 3"),
        ]);
        let bytes = doc_to_ydoc_bytes(&initial);

        let doc_a = Doc::with_client_id(1);
        let doc_b = Doc::with_client_id(2);
        let doc_c = Doc::with_client_id(3);
        {
            let mut txn = doc_a.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }
        {
            let mut txn = doc_b.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }
        {
            let mut txn = doc_c.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }

        let updates_a: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let updates_b: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let updates_c: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let cap_a = Rc::clone(&updates_a);
        let cap_b = Rc::clone(&updates_b);
        let cap_c = Rc::clone(&updates_c);
        let _sub_a = doc_a.observe_update_v1(move |_, e| { cap_a.borrow_mut().push(e.update.clone()); }).unwrap();
        let _sub_b = doc_b.observe_update_v1(move |_, e| { cap_b.borrow_mut().push(e.update.clone()); }).unwrap();
        let _sub_c = doc_c.observe_update_v1(move |_, e| { cap_c.borrow_mut().push(e.update.clone()); }).unwrap();

        // Each user edits their own block
        sync_model_to_ydoc(&doc_a, &make_doc(vec![
            make_para("b1", "Block 1 by A"), make_para("b2", "Block 2"), make_para("b3", "Block 3"),
        ]));
        sync_model_to_ydoc(&doc_b, &make_doc(vec![
            make_para("b1", "Block 1"), make_para("b2", "Block 2 by B"), make_para("b3", "Block 3"),
        ]));
        sync_model_to_ydoc(&doc_c, &make_doc(vec![
            make_para("b1", "Block 1"), make_para("b2", "Block 2"), make_para("b3", "Block 3 by C"),
        ]));

        // Full mesh exchange
        let ua = updates_a.borrow().clone();
        let ub = updates_b.borrow().clone();
        let uc = updates_c.borrow().clone();

        {
            let mut txn = doc_a.transact_mut();
            for u in &ub { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
            for u in &uc { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
        }
        {
            let mut txn = doc_b.transact_mut();
            for u in &ua { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
            for u in &uc { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
        }
        {
            let mut txn = doc_c.transact_mut();
            for u in &ua { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
            for u in &ub { txn.apply_update(Update::decode_v1(u).unwrap()).unwrap(); }
        }

        let result_a = read_doc_from_ydoc(&doc_a).unwrap();
        let result_b = read_doc_from_ydoc(&doc_b).unwrap();
        let result_c = read_doc_from_ydoc(&doc_c).unwrap();
        assert_eq!(result_a, result_b, "A and B should converge");
        assert_eq!(result_b, result_c, "B and C should converge");

        // All three edits should be preserved (different blocks)
        assert_eq!(result_a.child(0).unwrap().text_content(), "Block 1 by A");
        assert_eq!(result_a.child(1).unwrap().text_content(), "Block 2 by B");
        assert_eq!(result_a.child(2).unwrap().text_content(), "Block 3 by C");
    }

    // ─── Reconnect/refresh tests ───────────────────────────────────

    #[test]
    fn offline_edit_reconnect_via_state_vector() {
        let initial = make_doc(vec![
            make_para("b1", "Shared"),
            make_para("b2", "Content"),
        ]);
        let bytes = doc_to_ydoc_bytes(&initial);

        let doc_a = Doc::with_client_id(1);
        let doc_b = Doc::with_client_id(2);
        {
            let mut txn = doc_a.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }
        {
            let mut txn = doc_b.transact_mut();
            txn.apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        }

        // A goes offline, makes 3 edits
        sync_model_to_ydoc(&doc_a, &make_doc(vec![
            make_para("b1", "Shared edit1"), make_para("b2", "Content"),
        ]));
        sync_model_to_ydoc(&doc_a, &make_doc(vec![
            make_para("b1", "Shared edit1"), make_para("b2", "Content"),
            make_para("b3", "New block"),
        ]));
        sync_model_to_ydoc(&doc_a, &make_doc(vec![
            make_para("b1", "Shared edit1 edit2"), make_para("b2", "Content"),
            make_para("b3", "New block"),
        ]));

        // B makes 1 edit online
        sync_model_to_ydoc(&doc_b, &make_doc(vec![
            make_para("b1", "Shared"), make_para("b2", "Content by B"),
        ]));

        // Reconnect via state vector exchange
        let sv_b = { let txn = doc_b.transact(); txn.state_vector() };
        let sv_a = { let txn = doc_a.transact(); txn.state_vector() };
        let diff_a_for_b = {
            let txn = doc_a.transact();
            txn.encode_state_as_update_v1(&sv_b)
        };
        let diff_b_for_a = {
            let txn = doc_b.transact();
            txn.encode_state_as_update_v1(&sv_a)
        };

        {
            let mut txn = doc_b.transact_mut();
            txn.apply_update(Update::decode_v1(&diff_a_for_b).unwrap()).unwrap();
        }
        {
            let mut txn = doc_a.transact_mut();
            txn.apply_update(Update::decode_v1(&diff_b_for_a).unwrap()).unwrap();
        }

        let result_a = read_doc_from_ydoc(&doc_a).unwrap();
        let result_b = read_doc_from_ydoc(&doc_b).unwrap();
        assert_eq!(result_a, result_b, "state vector reconnect should converge");

        // B's edit to b2 should be preserved since it's a different block
        assert_eq!(result_a.child(1).unwrap().text_content(), "Content by B");
        // A's new block should appear
        assert!(result_a.child_count() >= 3);
    }

    #[test]
    fn chained_snapshot_refresh_no_duplication() {
        // Session 1: initial + edits
        let initial = make_doc(vec![make_para("b1", "Start")]);
        let snapshot_0 = doc_to_ydoc_bytes(&initial);

        let session1 = Doc::with_client_id(1);
        {
            let mut txn = session1.transact_mut();
            txn.apply_update(Update::decode_v1(&snapshot_0).unwrap()).unwrap();
        }
        let cap1: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let c1 = Rc::clone(&cap1);
        let _s1 = session1.observe_update_v1(move |_, e| { c1.borrow_mut().push(e.update.clone()); }).unwrap();

        sync_model_to_ydoc(&session1, &make_doc(vec![make_para("b1", "Start edit1")]));
        sync_model_to_ydoc(&session1, &make_doc(vec![make_para("b1", "Start edit1 edit2")]));

        // Server creates snapshot_1 = snapshot_0 + session1 updates
        let server1 = Doc::new();
        {
            let mut txn = server1.transact_mut();
            txn.apply_update(Update::decode_v1(&snapshot_0).unwrap()).unwrap();
            for u in cap1.borrow().iter() {
                txn.apply_update(Update::decode_v1(u).unwrap()).unwrap();
            }
        }
        let snapshot_1 = { let txn = server1.transact(); txn.encode_state_as_update_v1(&yrs::StateVector::default()) };

        // Session 2: load snapshot_1, make more edits
        let session2 = Doc::with_client_id(2);
        {
            let mut txn = session2.transact_mut();
            txn.apply_update(Update::decode_v1(&snapshot_1).unwrap()).unwrap();
        }
        let cap2: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let c2 = Rc::clone(&cap2);
        let _s2 = session2.observe_update_v1(move |_, e| { c2.borrow_mut().push(e.update.clone()); }).unwrap();

        sync_model_to_ydoc(&session2, &make_doc(vec![
            make_para("b1", "Start edit1 edit2"),
            make_para("b2", "New para"),
        ]));

        // Server creates snapshot_2
        let server2 = Doc::new();
        {
            let mut txn = server2.transact_mut();
            txn.apply_update(Update::decode_v1(&snapshot_1).unwrap()).unwrap();
            for u in cap2.borrow().iter() {
                txn.apply_update(Update::decode_v1(u).unwrap()).unwrap();
            }
        }
        let snapshot_2 = { let txn = server2.transact(); txn.encode_state_as_update_v1(&yrs::StateVector::default()) };

        // Session 3: fresh load from snapshot_2
        let result = ydoc_bytes_to_doc(&snapshot_2).unwrap();
        assert_eq!(result.child_count(), 2, "should have exactly 2 paragraphs, no duplication");
        assert_eq!(result.child(0).unwrap().text_content(), "Start edit1 edit2");
        assert_eq!(result.child(1).unwrap().text_content(), "New para");

        // Idempotency: syncing against own state produces no updates
        let session3 = Doc::new();
        {
            let mut txn = session3.transact_mut();
            txn.apply_update(Update::decode_v1(&snapshot_2).unwrap()).unwrap();
        }
        let cap3: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let c3 = Rc::clone(&cap3);
        let _s3 = session3.observe_update_v1(move |_, e| { c3.borrow_mut().push(e.update.clone()); }).unwrap();
        sync_model_to_ydoc(&session3, &result);
        assert_eq!(cap3.borrow().len(), 0, "syncing own state should produce no updates");
    }

    // ─── Structural edge case tests ────────────────────────────────

    #[test]
    fn sync_replace_all_content_no_blockid_match() {
        let ydoc = Doc::new();
        let initial = make_doc(vec![
            make_para("b1", "old one"),
            make_para("b2", "old two"),
            make_para("b3", "old three"),
        ]);
        sync_model_to_ydoc(&ydoc, &initial);

        // Completely new content, no blockId overlap
        let replaced = make_doc(vec![
            make_para("x1", "new alpha"),
            make_para("x2", "new beta"),
        ]);
        sync_model_to_ydoc(&ydoc, &replaced);

        let result = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(result.child_count(), 2);
        assert_eq!(result.child(0).unwrap().text_content(), "new alpha");
        assert_eq!(result.child(1).unwrap().text_content(), "new beta");
    }

    #[test]
    fn sync_mixed_blockid_and_leaf_nodes() {
        let ydoc = Doc::new();
        let mut img_attrs: HashMap<String, String> = HashMap::new();
        img_attrs.insert("src".into(), "img.png".into());
        let initial = make_doc(vec![
            make_para("b1", "text"),
            Node::element(NodeType::HorizontalRule),
            make_para("b2", "more"),
            Node::element_with_attrs(NodeType::Image, img_attrs.clone(), Fragment::from(vec![])),
            make_para("b3", "end"),
        ]);
        sync_model_to_ydoc(&ydoc, &initial);

        // Reorder: move HR after b2, remove image, edit text
        let updated = make_doc(vec![
            make_para("b1", "text updated"),
            make_para("b2", "more"),
            Node::element(NodeType::HorizontalRule),
            make_para("b3", "end changed"),
        ]);
        sync_model_to_ydoc(&ydoc, &updated);

        let result = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(result.child_count(), 4);
        assert_eq!(result.child(0).unwrap().text_content(), "text updated");
        assert_eq!(result.child(1).unwrap().text_content(), "more");
        assert_eq!(result.child(2).unwrap().node_type(), Some(NodeType::HorizontalRule));
        assert_eq!(result.child(3).unwrap().text_content(), "end changed");
    }

    #[test]
    fn sync_duplicate_blockids_no_panic() {
        let ydoc = Doc::new();
        let initial = make_doc(vec![make_para("b1", "Hello")]);
        sync_model_to_ydoc(&ydoc, &initial);

        // Sync with two paragraphs having the same blockId (shouldn't happen
        // in practice, but the code should handle it gracefully)
        let duped = make_doc(vec![
            make_para("b1", "First"),
            make_para("b1", "Second"),
        ]);
        sync_model_to_ydoc(&ydoc, &duped);

        let result = read_doc_from_ydoc(&ydoc).unwrap();
        assert_eq!(result.child_count(), 2, "both paragraphs should exist");
        let all_text = result.text_content();
        assert!(all_text.contains("First"), "first text should be present");
        assert!(all_text.contains("Second"), "second text should be present");
    }
}
