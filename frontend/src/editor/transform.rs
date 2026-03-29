use std::collections::HashMap;

use super::model::{char_len, char_slice, Fragment, Mark, Node, NodeType, Slice};

// ─── StepMap ────────────────────────────────────────────────────

/// Records position changes from a step as ranges.
/// Each range is (start, old_size, new_size): content at `start..start+old_size`
/// was replaced with content of size `new_size`.
#[derive(Debug, Clone)]
pub struct StepMap {
    ranges: Vec<(usize, usize, usize)>,
}

impl StepMap {
    pub fn empty() -> Self {
        Self { ranges: vec![] }
    }

    pub fn new(start: usize, old_size: usize, new_size: usize) -> Self {
        Self {
            ranges: vec![(start, old_size, new_size)],
        }
    }

    /// Create a StepMap with multiple ranges.
    /// Ranges must be specified in **output-adjusted** coordinates:
    /// each range's start accounts for the size changes from all previous ranges.
    pub fn multi(ranges: Vec<(usize, usize, usize)>) -> Self {
        Self { ranges }
    }

    /// Map a position through this step's changes.
    /// `bias`: -1 = prefer left side of insertion, 1 = prefer right side.
    pub fn map(&self, mut pos: usize, bias: i32) -> usize {
        for &(start, old_size, new_size) in &self.ranges {
            let end = start + old_size;
            if pos < start {
                // Before the change, unchanged
                continue;
            }
            if pos > end {
                // After the change, shift by the size difference
                if new_size > old_size {
                    pos += new_size - old_size;
                } else {
                    pos -= old_size - new_size;
                }
                continue;
            }
            // Position is inside the changed range
            if old_size == 0 {
                // Pure insertion: use bias
                if bias > 0 {
                    pos = start + new_size;
                }
                // bias <= 0: stay at start
            } else if pos == start && bias <= 0 {
                // At the start of the replaced range, bias left
            } else if pos == end && bias >= 0 {
                // At the end of the replaced range, bias right
                pos = start + new_size;
            } else {
                // Inside the deleted range: map to start or end based on bias
                if bias <= 0 {
                    pos = start;
                } else {
                    pos = start + new_size;
                }
            }
        }
        pos
    }
}

// ─── Step ───────────────────────────────────────────────────────

/// An atomic document transformation step.
#[derive(Debug, Clone)]
pub enum Step {
    /// Replace content between `from` and `to` with a Slice.
    Replace {
        from: usize,
        to: usize,
        slice: Slice,
    },
    /// Add a mark to text in the range `from..to`.
    AddMark {
        from: usize,
        to: usize,
        mark: Mark,
    },
    /// Remove a mark from text in the range `from..to`.
    RemoveMark {
        from: usize,
        to: usize,
        mark: Mark,
    },
    /// Set an attribute on the node at position `pos`.
    SetAttr {
        pos: usize,
        attr: String,
        value: String,
    },
    /// Change the type of a block node at position `pos`.
    SetNodeType {
        pos: usize,
        node_type: NodeType,
        attrs: HashMap<String, String>,
    },
    /// Replace the structure around content, preserving the content in a "gap".
    /// Positions: `from <= gap_from <= gap_to <= to`.
    /// The content at `gap_from..gap_to` is preserved.
    /// The structure at `from..gap_from` and `gap_to..to` is replaced
    /// with the wrapper described by `insert` (Slice with open_start/open_end
    /// indicating the depth of the gap in the wrapper).
    ReplaceAround {
        from: usize,
        to: usize,
        gap_from: usize,
        gap_to: usize,
        insert: Slice,
        structure: bool,
    },
}

/// Error from applying a step.
#[derive(Debug, Clone)]
pub struct StepError(pub String);

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "step error: {}", self.0)
    }
}

impl Step {
    /// Apply this step to a document, producing a new document and a StepMap.
    pub fn apply(&self, doc: &Node) -> Result<(Node, StepMap), StepError> {
        match self {
            Step::Replace { from, to, slice } => apply_replace(doc, *from, *to, slice),
            Step::AddMark { from, to, mark } => apply_add_mark(doc, *from, *to, mark),
            Step::RemoveMark { from, to, mark } => apply_remove_mark(doc, *from, *to, mark),
            Step::SetAttr { pos, attr, value } => apply_set_attr(doc, *pos, attr, value),
            Step::SetNodeType { pos, node_type, attrs } => apply_set_node_type(doc, *pos, node_type, attrs),
            Step::ReplaceAround { from, to, gap_from, gap_to, insert, structure } => {
                apply_replace_around(doc, *from, *to, *gap_from, *gap_to, insert, *structure)
            }
        }
    }

    /// Produce a step that undoes this step when applied to the result document.
    pub fn invert(&self, doc: &Node) -> Step {
        match self {
            Step::Replace { from, to, slice } => {
                // The inverse replaces the inserted content with the original content
                let original = doc_slice(doc, *from, *to);
                Step::Replace {
                    from: *from,
                    to: from + slice.size(),
                    slice: original,
                }
            }
            Step::AddMark { from, to, mark } => Step::RemoveMark {
                from: *from,
                to: *to,
                mark: mark.clone(),
            },
            Step::RemoveMark { from, to, mark } => Step::AddMark {
                from: *from,
                to: *to,
                mark: mark.clone(),
            },
            Step::SetAttr { pos, attr, value: _ } => {
                // Get the old value
                let old_value = get_attr_at(doc, *pos, attr).unwrap_or_default();
                Step::SetAttr {
                    pos: *pos,
                    attr: attr.clone(),
                    value: old_value,
                }
            }
            Step::SetNodeType { pos, .. } => {
                let (old_type, old_attrs) = get_node_type_at(doc, *pos)
                    .unwrap_or((NodeType::Paragraph, HashMap::new()));
                Step::SetNodeType {
                    pos: *pos,
                    node_type: old_type,
                    attrs: old_attrs,
                }
            }
            Step::ReplaceAround { from, to, gap_from, gap_to, insert, structure } => {
                // The inverse must reconstruct the old wrapper from the original doc.
                // The old wrapper structure is the content at from..to with the gap
                // content (gap_from..gap_to) replaced by an empty placeholder.
                let before_gap = *gap_from - *from;
                let after_gap = *to - *gap_to;
                let old_insert = if before_gap == 0 && after_gap == 0 {
                    Slice::empty()
                } else {
                    // Extract the full node(s) at from..to from the original doc,
                    // then hollow out the gap content. The simplest approach:
                    // reconstruct the wrapper with empty content at the gap.
                    let full_slice = doc_slice(doc, *from, *to);
                    // The wrapper is the full_slice with gap content replaced by nothing.
                    // The gap is at relative offset gap_from-from in the slice.
                    // Use open_start = before_gap, open_end = after_gap to indicate
                    // the gap depth (number of boundary bytes before/after the gap).
                    let hollowed = hollow_out(&full_slice.content, before_gap, after_gap);
                    Slice::new(hollowed, before_gap, after_gap)
                };

                // In the result doc, compute the new positions.
                // The insert replaced from..gap_from (before_gap bytes) with insert.open_start bytes
                // and gap_to..to (after_gap bytes) with insert.open_end bytes.
                let new_gap_from = *from + insert.open_start;
                let gap_size = *gap_to - *gap_from;
                let new_gap_to = new_gap_from + gap_size;
                let new_to = new_gap_to + insert.open_end;

                Step::ReplaceAround {
                    from: *from,
                    to: new_to,
                    gap_from: new_gap_from,
                    gap_to: new_gap_to,
                    insert: old_insert,
                    structure: *structure,
                }
            }
        }
    }

    /// Map this step through a StepMap (for composing with concurrent changes).
    pub fn map(&self, mapping: &StepMap) -> Step {
        match self {
            Step::Replace { from, to, slice } => Step::Replace {
                from: mapping.map(*from, 1),
                to: mapping.map(*to, -1),
                slice: slice.clone(),
            },
            Step::AddMark { from, to, mark } => Step::AddMark {
                from: mapping.map(*from, 1),
                to: mapping.map(*to, -1),
                mark: mark.clone(),
            },
            Step::RemoveMark { from, to, mark } => Step::RemoveMark {
                from: mapping.map(*from, 1),
                to: mapping.map(*to, -1),
                mark: mark.clone(),
            },
            Step::SetAttr { pos, attr, value } => Step::SetAttr {
                pos: mapping.map(*pos, 1),
                attr: attr.clone(),
                value: value.clone(),
            },
            Step::SetNodeType { pos, node_type, attrs } => Step::SetNodeType {
                pos: mapping.map(*pos, 1),
                node_type: *node_type,
                attrs: attrs.clone(),
            },
            Step::ReplaceAround { from, to, gap_from, gap_to, insert, structure } => {
                Step::ReplaceAround {
                    from: mapping.map(*from, 1),
                    to: mapping.map(*to, -1),
                    gap_from: mapping.map(*gap_from, -1),
                    gap_to: mapping.map(*gap_to, 1),
                    insert: insert.clone(),
                    structure: *structure,
                }
            }
        }
    }
}

// ─── Step application helpers ───────────────────────────────────

/// Apply a Replace step: replace content at from..to with slice.
fn apply_replace(doc: &Node, from: usize, to: usize, slice: &Slice) -> Result<(Node, StepMap), StepError> {
    let new_doc = replace_in_doc(doc, from, to, slice)?;
    let map = StepMap::new(from, to - from, slice.size());
    Ok((new_doc, map))
}

/// Replace content in a document at the given positions with a slice.
/// Recursively descends into the element tree to find the deepest common
/// ancestor that contains both `from` and `to`, then replaces at that level.
fn replace_in_doc(node: &Node, from: usize, to: usize, slice: &Slice) -> Result<Node, StepError> {
    let Node::Element { node_type, attrs, content, marks } = node else {
        return Err(StepError("cannot replace in text node".into()));
    };

    if node_type.is_leaf() {
        return Err(StepError("cannot replace in leaf node".into()));
    }

    let content_size = content.size();
    if to > content_size {
        return Err(StepError(format!(
            "replace range {from}..{to} exceeds content size {content_size}"
        )));
    }

    // Check if from and to fall within the same child element.
    // If so, recurse into that child.
    let mut offset = 0;
    for (i, child) in content.children.iter().enumerate() {
        let child_size = child.node_size();
        let child_end = offset + child_size;

        if let Node::Element { node_type: ct, .. } = child {
            if !ct.is_leaf() && from >= offset + 1 && to <= child_end - 1 {
                // Both from and to are inside this child's content
                let inner_from = from - offset - 1;
                let inner_to = to - offset - 1;
                let new_child = replace_in_doc(child, inner_from, inner_to, slice)?;
                let new_content = content.replace_child(i, new_child);
                return Ok(Node::Element {
                    node_type: *node_type,
                    attrs: attrs.clone(),
                    content: new_content,
                    marks: marks.clone(),
                });
            }
        }

        offset += child_size;
    }

    // from and to don't fall within the same child -- replace at this level
    let before = if from > 0 {
        content.cut(0, from)
    } else {
        Fragment::empty()
    };
    let after = if to < content_size {
        content.cut(to, content_size)
    } else {
        Fragment::empty()
    };

    let new_content = before
        .append_fragment(slice.content.clone())
        .append_fragment(after);

    Ok(Node::Element {
        node_type: *node_type,
        attrs: attrs.clone(),
        content: new_content,
        marks: marks.clone(),
    })
}

/// Apply ReplaceAround: replace structure around content while preserving the gap.
fn apply_replace_around(
    doc: &Node,
    from: usize,
    to: usize,
    gap_from: usize,
    gap_to: usize,
    insert: &Slice,
    _structure: bool,
) -> Result<(Node, StepMap), StepError> {
    if from > gap_from || gap_from > gap_to || gap_to > to {
        return Err(StepError("invalid ReplaceAround positions".into()));
    }

    // 1. Extract the gap content (the content to preserve)
    let gap_content = if gap_from < gap_to {
        doc_slice(doc, gap_from, gap_to)
    } else {
        Slice::empty()
    };

    // 2. Build the full replacement: wrapper structure with gap content inside
    let replacement = if insert.size() == 0 && insert.open_start == 0 {
        // No wrapper -- just the gap content (unwrapping)
        gap_content
    } else {
        // Place gap content inside the wrapper at depth open_start
        let filled = fill_gap(&insert.content, insert.open_start, &gap_content.content)?;
        Slice::new(filled, 0, 0)
    };

    // 3. Replace from..to with the filled wrapper
    let new_doc = replace_in_doc(doc, from, to, &replacement)?;

    // 4. Build multi-range StepMap for correct cursor mapping.
    // The step replaces two disjoint regions:
    //   - from..gap_from (old opening structure) -> insert.open_start bytes
    //   - gap_to..to (old closing structure) -> insert.open_end bytes
    let before_gap_size = gap_from - from;
    let after_gap_size = to - gap_to;
    let insert_before = insert.open_start;
    let insert_after = insert.open_end;

    // Range 1 is in original coordinates.
    // Range 2 must be in output-adjusted coordinates (after range 1 has shifted positions).
    let adjusted_gap_to = if insert_before >= before_gap_size {
        gap_to + (insert_before - before_gap_size)
    } else {
        gap_to - (before_gap_size - insert_before)
    };

    let map = StepMap::multi(vec![
        (from, before_gap_size, insert_before),
        (adjusted_gap_to, after_gap_size, insert_after),
    ]);

    Ok((new_doc, map))
}

/// Place gap content inside a wrapper fragment at the given depth.
/// Descends `depth` levels into the fragment (always following the last child)
/// and inserts the gap content there.
fn fill_gap(wrapper: &Fragment, depth: usize, gap_content: &Fragment) -> Result<Fragment, StepError> {
    if depth == 0 {
        // At the gap level: place gap content here, replacing any placeholder children
        return Ok(gap_content.clone());
    }

    if wrapper.children.is_empty() {
        return Err(StepError("wrapper has no children at required depth".into()));
    }

    // Descend into the last child
    let mut children = wrapper.children.clone();
    let last_idx = children.len() - 1;
    match &children[last_idx] {
        Node::Element { node_type, attrs, content, marks } => {
            let inner = fill_gap(content, depth - 1, gap_content)?;
            children[last_idx] = Node::Element {
                node_type: *node_type,
                attrs: attrs.clone(),
                content: inner,
                marks: marks.clone(),
            };
            Ok(Fragment::from(children))
        }
        _ => Err(StepError("cannot descend into text node in wrapper".into())),
    }
}

/// Hollow out a fragment by removing the innermost content while preserving
/// the wrapper structure. `before_gap` is the number of boundary bytes before
/// the gap, `after_gap` is the number after. Each boundary byte corresponds
/// to one level of nesting (the open or close of an element).
fn hollow_out(fragment: &Fragment, before_gap: usize, after_gap: usize) -> Fragment {
    if before_gap == 0 && after_gap == 0 {
        return Fragment::empty();
    }
    if fragment.children.is_empty() {
        return fragment.clone();
    }

    // We need to descend before_gap levels into the fragment to find where the
    // gap content starts, then empty it out. The wrapper is the outer structure.
    hollow_out_inner(fragment, before_gap)
}

fn hollow_out_inner(fragment: &Fragment, depth: usize) -> Fragment {
    if depth == 0 {
        // At the gap level: return empty content (the gap placeholder)
        return Fragment::empty();
    }

    let mut children = fragment.children.clone();
    if children.is_empty() {
        return fragment.clone();
    }

    // Descend into the last child (which contains the gap path)
    let last_idx = children.len() - 1;
    if let Node::Element { node_type, attrs, content, marks } = &children[last_idx] {
        let inner = hollow_out_inner(content, depth - 1);
        children[last_idx] = Node::Element {
            node_type: *node_type,
            attrs: attrs.clone(),
            content: inner,
            marks: marks.clone(),
        };
    }
    Fragment::from(children)
}

/// Apply AddMark: add a mark to all text in from..to.
fn apply_add_mark(doc: &Node, from: usize, to: usize, mark: &Mark) -> Result<(Node, StepMap), StepError> {
    let new_doc = map_text_in_range(doc, from, to, |text, marks| {
        let mut new_marks = marks.to_vec();
        if !new_marks.iter().any(|m| m == mark) {
            new_marks.push(mark.clone());
            super::model::normalize_marks(&mut new_marks);
        }
        Node::text_with_marks(text, new_marks)
    })?;
    Ok((new_doc, StepMap::empty()))
}

/// Apply RemoveMark: remove a mark from all text in from..to.
fn apply_remove_mark(doc: &Node, from: usize, to: usize, mark: &Mark) -> Result<(Node, StepMap), StepError> {
    let new_doc = map_text_in_range(doc, from, to, |text, marks| {
        let new_marks: Vec<Mark> = marks.iter().filter(|m| m.mark_type != mark.mark_type).cloned().collect();
        if new_marks.is_empty() {
            Node::text(text)
        } else {
            Node::text_with_marks(text, new_marks)
        }
    })?;
    Ok((new_doc, StepMap::empty()))
}

/// Apply SetAttr: change an attribute on the element at `pos`.
/// `pos` is the position of the element's opening boundary in its parent's content.
fn apply_set_attr(doc: &Node, pos: usize, attr: &str, value: &str) -> Result<(Node, StepMap), StepError> {
    let new_doc = set_attr_at(doc, pos, attr, value)?;
    Ok((new_doc, StepMap::empty()))
}

/// Map a function over text nodes within a position range.
/// The function receives the text content and current marks, returns a new Node.
fn map_text_in_range<F>(node: &Node, from: usize, to: usize, f: F) -> Result<Node, StepError>
where
    F: Fn(&str, &[Mark]) -> Node + Copy,
{
    match node {
        Node::Text { text, marks } => {
            let len = char_len(text);
            // The entire text node is at position 0..len relative to its parent
            // from/to are relative to this node's position in the parent's content
            if from >= len || to == 0 {
                return Ok(node.clone()); // no overlap
            }
            let start = from;
            let end = to.min(len);

            if start == 0 && end >= len {
                // Whole text node affected
                return Ok(f(text, marks));
            }

            // Partial: split into before + affected + after
            let mut parts = Vec::new();
            if start > 0 {
                parts.push(Node::Text {
                    text: char_slice(text, 0, start),
                    marks: marks.clone(),
                });
            }
            parts.push(f(&char_slice(text, start, end), marks));
            if end < len {
                parts.push(Node::Text {
                    text: char_slice(text, end, len),
                    marks: marks.clone(),
                });
            }

            // Return as a fragment wrapped in a sentinel -- the caller must flatten
            // Actually, for simplicity, return the first part. The caller handles multi-part.
            // For now, handle this at the parent level.
            Err(StepError("partial text mark not handled in isolation".into()))
        }
        Node::Element { node_type, attrs, content, marks } => {
            if node_type.is_leaf() {
                return Ok(node.clone());
            }

            let mut new_children = Vec::new();
            let mut pos = 0;

            for child in &content.children {
                let child_size = child.node_size();
                let child_end = pos + child_size;

                if child_end <= from || pos >= to {
                    // Child is completely outside the range
                    new_children.push(child.clone());
                } else {
                    match child {
                        Node::Text { text, marks: child_marks } => {
                            let text_len = char_len(text);
                            let rel_from = if from > pos { from - pos } else { 0 };
                            let rel_to = if to < child_end { to - pos } else { text_len };

                            // Split text and apply function to the overlapping part
                            if rel_from > 0 {
                                new_children.push(Node::Text {
                                    text: char_slice(text, 0, rel_from),
                                    marks: child_marks.clone(),
                                });
                            }
                            new_children.push(f(&char_slice(text, rel_from, rel_to), child_marks));
                            if rel_to < text_len {
                                new_children.push(Node::Text {
                                    text: char_slice(text, rel_to, text_len),
                                    marks: child_marks.clone(),
                                });
                            }
                        }
                        Node::Element { .. } => {
                            if child.node_type().map(|t| t.is_leaf()).unwrap_or(false) {
                                new_children.push(child.clone());
                            } else {
                                // Recurse: adjust positions relative to child content
                                let inner_from = if from > pos + 1 { from - pos - 1 } else { 0 };
                                let inner_to = if to < child_end - 1 { to - pos - 1 } else { child.content_size() };
                                let mapped = map_text_in_range(child, inner_from, inner_to, f)?;
                                new_children.push(mapped);
                            }
                        }
                    }
                }
                pos = child_end;
            }

            Ok(Node::Element {
                node_type: *node_type,
                attrs: attrs.clone(),
                content: Fragment::from(new_children),
                marks: marks.clone(),
            })
        }
    }
}

/// Get a Slice from a document between two positions.
/// Recursively descends to find the content at the correct level.
fn doc_slice(node: &Node, from: usize, to: usize) -> Slice {
    if from >= to {
        return Slice::empty();
    }
    match node {
        Node::Element { content, node_type, .. } if !node_type.is_leaf() => {
            // Check if both from and to fall inside the same child
            let mut offset = 0;
            for child in &content.children {
                let child_size = child.node_size();
                let child_end = offset + child_size;

                if let Node::Element { node_type: ct, .. } = child {
                    if !ct.is_leaf() && from >= offset + 1 && to <= child_end - 1 {
                        return doc_slice(child, from - offset - 1, to - offset - 1);
                    }
                }
                offset += child_size;
            }

            // Cut at this level
            let cut = content.cut(from, to);
            Slice::new(cut, 0, 0)
        }
        _ => Slice::empty(),
    }
}

/// Get an attribute value at a position.
fn get_attr_at(doc: &Node, pos: usize, attr: &str) -> Option<String> {
    match doc {
        Node::Element { content, node_type, .. } if !node_type.is_leaf() => {
            let mut offset = 0;
            for child in &content.children {
                let size = child.node_size();
                if offset == pos {
                    return child.attrs().get(attr).cloned();
                }
                if pos > offset && pos < offset + size {
                    if let Node::Element { .. } = child {
                        if !child.node_type().map(|t| t.is_leaf()).unwrap_or(true) {
                            return get_attr_at(child, pos - offset - 1, attr);
                        }
                    }
                }
                offset += size;
            }
            None
        }
        _ => None,
    }
}

/// Set an attribute on a node at a given position.
fn set_attr_at(doc: &Node, pos: usize, attr: &str, value: &str) -> Result<Node, StepError> {
    match doc {
        Node::Element { node_type, attrs, content, marks } if !node_type.is_leaf() => {
            let mut new_children = Vec::new();
            let mut offset = 0;

            for child in &content.children {
                let size = child.node_size();
                if offset == pos {
                    // This is the target node -- set the attribute
                    match child {
                        Node::Element { node_type: ct, attrs: ca, content: cc, marks: cm } => {
                            let mut new_attrs = ca.clone();
                            new_attrs.insert(attr.to_string(), value.to_string());
                            new_children.push(Node::Element {
                                node_type: *ct,
                                attrs: new_attrs,
                                content: cc.clone(),
                                marks: cm.clone(),
                            });
                        }
                        _ => {
                            return Err(StepError("cannot set attr on text node".into()));
                        }
                    }
                } else if pos > offset && pos < offset + size {
                    if let Node::Element { .. } = child {
                        if !child.node_type().map(|t| t.is_leaf()).unwrap_or(true) {
                            let inner = set_attr_at(child, pos - offset - 1, attr, value)?;
                            new_children.push(inner);
                        } else {
                            new_children.push(child.clone());
                        }
                    } else {
                        new_children.push(child.clone());
                    }
                } else {
                    new_children.push(child.clone());
                }
                offset += size;
            }

            Ok(Node::Element {
                node_type: *node_type,
                attrs: attrs.clone(),
                content: Fragment::from(new_children),
                marks: marks.clone(),
            })
        }
        _ => Err(StepError("cannot set attr on non-element".into())),
    }
}

/// Apply SetNodeType: change the type and attributes of a block node at `pos`.
fn apply_set_node_type(
    doc: &Node,
    pos: usize,
    new_type: &NodeType,
    new_attrs: &HashMap<String, String>,
) -> Result<(Node, StepMap), StepError> {
    let new_doc = set_node_type_at(doc, pos, new_type, new_attrs)?;
    Ok((new_doc, StepMap::empty()))
}

/// Set the node type and attributes on a node at a given position.
fn set_node_type_at(
    doc: &Node,
    pos: usize,
    new_type: &NodeType,
    new_attrs: &HashMap<String, String>,
) -> Result<Node, StepError> {
    match doc {
        Node::Element { node_type, attrs, content, marks } if !node_type.is_leaf() => {
            let mut new_children = Vec::new();
            let mut offset = 0;

            for child in &content.children {
                let size = child.node_size();
                if offset == pos {
                    match child {
                        Node::Element { content: cc, marks: cm, .. } => {
                            new_children.push(Node::Element {
                                node_type: *new_type,
                                attrs: new_attrs.clone(),
                                content: cc.clone(),
                                marks: cm.clone(),
                            });
                        }
                        _ => return Err(StepError("cannot set node type on text node".into())),
                    }
                } else if pos > offset && pos < offset + size {
                    if let Node::Element { .. } = child {
                        if !child.node_type().map(|t| t.is_leaf()).unwrap_or(true) {
                            let inner = set_node_type_at(child, pos - offset - 1, new_type, new_attrs)?;
                            new_children.push(inner);
                        } else {
                            new_children.push(child.clone());
                        }
                    } else {
                        new_children.push(child.clone());
                    }
                } else {
                    new_children.push(child.clone());
                }
                offset += size;
            }

            Ok(Node::Element {
                node_type: *node_type,
                attrs: attrs.clone(),
                content: Fragment::from(new_children),
                marks: marks.clone(),
            })
        }
        _ => Err(StepError("cannot set node type on non-element".into())),
    }
}

/// Get the node type and attributes of a node at a given position.
fn get_node_type_at(doc: &Node, pos: usize) -> Option<(NodeType, HashMap<String, String>)> {
    match doc {
        Node::Element { content, node_type, .. } if !node_type.is_leaf() => {
            let mut offset = 0;
            for child in &content.children {
                let size = child.node_size();
                if offset == pos {
                    if let Node::Element { node_type: ct, attrs: ca, .. } = child {
                        return Some((*ct, ca.clone()));
                    }
                    return None;
                }
                if pos > offset && pos < offset + size {
                    if let Node::Element { .. } = child {
                        if !child.node_type().map(|t| t.is_leaf()).unwrap_or(true) {
                            return get_node_type_at(child, pos - offset - 1);
                        }
                    }
                }
                offset += size;
            }
            None
        }
        _ => None,
    }
}

// ─── Transform ──────────────────────────────────────────────────

/// A sequence of steps applied to a document, with position mapping.
#[derive(Debug, Clone)]
pub struct Transform {
    /// The current document state after all steps.
    pub doc: Node,
    /// Steps applied so far.
    pub steps: Vec<Step>,
    /// Step maps for position mapping.
    pub maps: Vec<StepMap>,
}

impl Transform {
    /// Create a new Transform starting from a document.
    pub fn new(doc: Node) -> Self {
        Self {
            doc,
            steps: Vec::new(),
            maps: Vec::new(),
        }
    }

    /// Apply a step, updating the document and recording the step.
    pub fn step(mut self, step: Step) -> Result<Self, StepError> {
        let (new_doc, map) = step.apply(&self.doc)?;
        self.doc = new_doc;
        self.steps.push(step);
        self.maps.push(map);
        Ok(self)
    }

    /// Insert content at a position.
    pub fn insert(self, pos: usize, content: Fragment) -> Result<Self, StepError> {
        self.step(Step::Replace {
            from: pos,
            to: pos,
            slice: Slice::new(content, 0, 0),
        })
    }

    /// Delete content between two positions.
    pub fn delete(self, from: usize, to: usize) -> Result<Self, StepError> {
        self.step(Step::Replace {
            from,
            to,
            slice: Slice::empty(),
        })
    }

    /// Replace content between two positions with a slice.
    pub fn replace(self, from: usize, to: usize, slice: Slice) -> Result<Self, StepError> {
        self.step(Step::Replace { from, to, slice })
    }

    /// Insert text at a position.
    pub fn insert_text(self, pos: usize, text: &str) -> Result<Self, StepError> {
        self.insert(pos, Fragment::from(vec![Node::text(text)]))
    }

    /// Add a mark to text in a range.
    pub fn add_mark(self, from: usize, to: usize, mark: Mark) -> Result<Self, StepError> {
        self.step(Step::AddMark { from, to, mark })
    }

    /// Remove a mark from text in a range.
    pub fn remove_mark(self, from: usize, to: usize, mark: Mark) -> Result<Self, StepError> {
        self.step(Step::RemoveMark { from, to, mark })
    }

    /// Set a block type for nodes in a range.
    pub fn set_block_type(
        self,
        pos: usize,
        _node_type: NodeType,
        attrs: HashMap<String, String>,
    ) -> Result<Self, StepError> {
        // SetAttr can change the type by setting a special attribute.
        // For the MVP, we'll use SetAttr to change the node type attribute.
        // A more correct implementation would use ReplaceAroundStep.
        // For now, use the SetAttr mechanism for heading level changes.
        let mut t = self;
        for (key, value) in attrs {
            t = t.step(Step::SetAttr {
                pos,
                attr: key,
                value,
            })?;
        }
        Ok(t)
    }

    /// Wrap content at from..to in wrapper nodes, preserving the content.
    /// gap_from..gap_to is the content to preserve.
    /// insert is the wrapper structure with open_start/open_end indicating gap depth.
    pub fn wrap_around(
        self,
        from: usize,
        to: usize,
        gap_from: usize,
        gap_to: usize,
        insert: Slice,
    ) -> Result<Self, StepError> {
        self.step(Step::ReplaceAround {
            from,
            to,
            gap_from,
            gap_to,
            insert,
            structure: true,
        })
    }

    /// Map a position through all steps applied so far.
    pub fn map_pos(&self, pos: usize, bias: i32) -> usize {
        let mut result = pos;
        for map in &self.maps {
            result = map.map(result, bias);
        }
        result
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::MarkType;

    fn simple_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        )
    }

    fn two_para_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
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
        )
    }

    // ── StepMap ──

    #[test]
    fn stepmap_identity() {
        let map = StepMap::empty();
        assert_eq!(map.map(5, 1), 5);
    }

    #[test]
    fn stepmap_after_insert() {
        // Insert 3 chars at position 5
        let map = StepMap::new(5, 0, 3);
        assert_eq!(map.map(3, 1), 3); // before insert
        assert_eq!(map.map(5, 1), 8); // at insert point, bias right
        assert_eq!(map.map(5, -1), 5); // at insert point, bias left
        assert_eq!(map.map(7, 1), 10); // after insert
    }

    #[test]
    fn stepmap_after_delete() {
        // Delete 3 chars at position 5 (5..8)
        let map = StepMap::new(5, 3, 0);
        assert_eq!(map.map(3, 1), 3); // before delete
        assert_eq!(map.map(6, 1), 5); // inside deleted range
        assert_eq!(map.map(10, 1), 7); // after delete
    }

    #[test]
    fn stepmap_after_replace() {
        // Replace 3 chars at position 5 with 5 chars
        let map = StepMap::new(5, 3, 5);
        assert_eq!(map.map(3, 1), 3); // before
        assert_eq!(map.map(10, 1), 12); // after: shifted by +2
    }

    #[test]
    fn stepmap_monotonic() {
        let map = StepMap::new(5, 3, 7);
        let mut prev = 0;
        for pos in 0..20 {
            let mapped = map.map(pos, 1);
            assert!(mapped >= prev, "pos {pos} mapped to {mapped}, prev was {prev}");
            prev = mapped;
        }
    }

    // ── Replace Step ──

    #[test]
    fn insert_text_at_start() {
        let doc = simple_doc();
        // Insert "Hi " at position 1 (start of paragraph content)
        let (new_doc, _map) = Step::Replace {
            from: 1,
            to: 1,
            slice: Slice::new(Fragment::from(vec![Node::text("Hi ")]), 0, 0),
        }
        .apply(&doc)
        .unwrap();

        let para = new_doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Hi Hello world");
    }

    #[test]
    fn insert_text_at_end() {
        let doc = simple_doc();
        // Insert "!" at position 12 (end of paragraph content, 1 + 11 = 12)
        let (new_doc, _map) = Step::Replace {
            from: 12,
            to: 12,
            slice: Slice::new(Fragment::from(vec![Node::text("!")]), 0, 0),
        }
        .apply(&doc)
        .unwrap();

        let para = new_doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Hello world!");
    }

    #[test]
    fn delete_range() {
        let doc = simple_doc();
        // Delete "Hello " (positions 1..7 in doc content)
        let (new_doc, _map) = Step::Replace {
            from: 1,
            to: 7,
            slice: Slice::empty(),
        }
        .apply(&doc)
        .unwrap();

        let para = new_doc.child(0).unwrap();
        assert_eq!(para.text_content(), "world");
    }

    #[test]
    fn replace_range() {
        let doc = simple_doc();
        // Replace "Hello" (1..6) with "Goodbye"
        let (new_doc, _map) = Step::Replace {
            from: 1,
            to: 6,
            slice: Slice::new(Fragment::from(vec![Node::text("Goodbye")]), 0, 0),
        }
        .apply(&doc)
        .unwrap();

        let para = new_doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Goodbye world");
    }

    // ── AddMark / RemoveMark ──

    #[test]
    fn add_mark_to_range() {
        let doc = simple_doc();
        // Bold "Hello" (positions 1..6 in doc, which is 0..5 in para content)
        let (new_doc, _) = Step::AddMark {
            from: 1,
            to: 6,
            mark: Mark::new(MarkType::Bold),
        }
        .apply(&doc)
        .unwrap();

        let para = new_doc.child(0).unwrap();
        // Should have 2 children: bold "Hello" + plain " world"
        assert_eq!(para.child_count(), 2);
        let bold_text = para.child(0).unwrap();
        assert_eq!(bold_text.text_content(), "Hello");
        assert!(bold_text.marks().iter().any(|m| m.mark_type == MarkType::Bold));

        let plain_text = para.child(1).unwrap();
        assert_eq!(plain_text.text_content(), " world");
        assert!(plain_text.marks().is_empty());
    }

    #[test]
    fn remove_mark_from_range() {
        // Start with a doc where "Hello" is bold
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text_with_marks("Hello", vec![Mark::new(MarkType::Bold)]),
                    Node::text(" world"),
                ]),
            )]),
        );

        let (new_doc, _) = Step::RemoveMark {
            from: 1,
            to: 6,
            mark: Mark::new(MarkType::Bold),
        }
        .apply(&doc)
        .unwrap();

        let para = new_doc.child(0).unwrap();
        // Should be merged into one plain text node
        assert_eq!(para.child_count(), 1);
        assert_eq!(para.text_content(), "Hello world");
        assert!(para.child(0).unwrap().marks().is_empty());
    }

    // ── SetAttr ──

    #[test]
    fn set_heading_level() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Heading,
                Fragment::from(vec![Node::text("Title")]),
            )]),
        );

        // Set level to 2 on the heading at position 0
        let (new_doc, _) = Step::SetAttr {
            pos: 0,
            attr: "level".to_string(),
            value: "2".to_string(),
        }
        .apply(&doc)
        .unwrap();

        let heading = new_doc.child(0).unwrap();
        assert_eq!(heading.attrs().get("level").unwrap(), "2");
    }

    // ── Step inversion ──

    #[test]
    fn insert_invert_roundtrip() {
        let doc = simple_doc();
        let step = Step::Replace {
            from: 1,
            to: 1,
            slice: Slice::new(Fragment::from(vec![Node::text("X")]), 0, 0),
        };

        let (modified, _) = step.apply(&doc).unwrap();
        assert_ne!(doc, modified);

        let inverse = step.invert(&doc);
        let (restored, _) = inverse.apply(&modified).unwrap();
        assert_eq!(doc, restored);
    }

    #[test]
    fn delete_invert_roundtrip() {
        let doc = simple_doc();
        let step = Step::Replace {
            from: 1,
            to: 6,
            slice: Slice::empty(),
        };

        let (modified, _) = step.apply(&doc).unwrap();
        let inverse = step.invert(&doc);
        let (restored, _) = inverse.apply(&modified).unwrap();
        assert_eq!(doc, restored);
    }

    #[test]
    fn add_mark_invert_roundtrip() {
        let doc = simple_doc();
        let step = Step::AddMark {
            from: 1,
            to: 6,
            mark: Mark::new(MarkType::Bold),
        };

        let (modified, _) = step.apply(&doc).unwrap();
        let inverse = step.invert(&doc);
        let (restored, _) = inverse.apply(&modified).unwrap();
        assert_eq!(doc, restored);
    }

    // ── Transform ──

    #[test]
    fn transform_insert_text() {
        let doc = simple_doc();
        let t = Transform::new(doc.clone())
            .insert_text(1, "Hey ")
            .unwrap();

        let para = t.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Hey Hello world");
        assert_eq!(t.steps.len(), 1);
    }

    #[test]
    fn transform_delete() {
        let doc = simple_doc();
        let t = Transform::new(doc).delete(1, 7).unwrap();

        let para = t.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "world");
    }

    #[test]
    fn transform_add_mark() {
        let doc = simple_doc();
        let t = Transform::new(doc)
            .add_mark(1, 6, Mark::new(MarkType::Italic))
            .unwrap();

        let para = t.doc.child(0).unwrap();
        assert_eq!(para.child_count(), 2);
        assert!(para.child(0).unwrap().marks().iter().any(|m| m.mark_type == MarkType::Italic));
    }

    #[test]
    fn transform_chained() {
        let doc = simple_doc();
        let t = Transform::new(doc)
            .delete(1, 7)
            .unwrap()
            .insert_text(1, "Goodbye ")
            .unwrap();

        let para = t.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Goodbye world");
        assert_eq!(t.steps.len(), 2);
    }

    #[test]
    fn transform_map_pos() {
        let doc = simple_doc();
        let t = Transform::new(doc)
            .insert_text(1, "XYZ")
            .unwrap();

        // Position 1 (at insert point, bias right) -> 4
        assert_eq!(t.map_pos(1, 1), 4);
        // Position 5 (after insert) -> 8
        assert_eq!(t.map_pos(5, 1), 8);
    }

    // ── Error cases ──

    #[test]
    fn replace_out_of_bounds() {
        let doc = simple_doc();
        let result = Step::Replace {
            from: 0,
            to: 100,
            slice: Slice::empty(),
        }
        .apply(&doc);
        assert!(result.is_err());
    }

    // ── ReplaceAround Step ──

    fn make_wrapper(outer: NodeType, inner: NodeType) -> Slice {
        // Build: Outer[Inner[]] with open_start=2, open_end=2
        // meaning the gap is 2 levels deep inside the wrapper
        let inner_node = Node::element(inner);
        let outer_node = Node::element_with_content(outer, Fragment::from(vec![inner_node]));
        Slice::new(Fragment::from(vec![outer_node]), 2, 2)
    }

    fn make_single_wrapper(wrapper_type: NodeType) -> Slice {
        // Build: Wrapper[] with open_start=1, open_end=1
        let wrapper = Node::element(wrapper_type);
        Slice::new(Fragment::from(vec![wrapper]), 1, 1)
    }

    #[test]
    fn wrap_paragraph_in_bullet_list() {
        let doc = simple_doc(); // Doc[Paragraph["Hello world"]]
        // Paragraph is at offset 0 in doc content, size 13 (2 + 11 chars)
        let para_size = doc.child(0).unwrap().node_size();
        assert_eq!(para_size, 13);

        let wrapper = make_wrapper(NodeType::BulletList, NodeType::ListItem);
        let step = Step::ReplaceAround {
            from: 0,
            to: para_size,
            gap_from: 0,
            gap_to: para_size,
            insert: wrapper,
            structure: true,
        };

        let (new_doc, map) = step.apply(&doc).unwrap();

        // New doc should be Doc[BulletList[ListItem[Paragraph["Hello world"]]]]
        let bullet_list = new_doc.child(0).unwrap();
        assert_eq!(bullet_list.node_type(), Some(NodeType::BulletList));
        let list_item = bullet_list.child(0).unwrap();
        assert_eq!(list_item.node_type(), Some(NodeType::ListItem));
        let para = list_item.child(0).unwrap();
        assert_eq!(para.node_type(), Some(NodeType::Paragraph));
        assert_eq!(para.text_content(), "Hello world");

        // Verify position mapping: old pos 1 (start of "Hello") -> new pos 3
        assert_eq!(map.map(1, 1), 3);
        // Old pos 12 (end of text) -> new pos 14
        assert_eq!(map.map(12, 1), 14);
    }

    #[test]
    fn unwrap_list_to_paragraph() {
        // Start with Doc[BulletList[ListItem[Paragraph["Hello world"]]]]
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("Hello world")]),
        );
        let list_item = Node::element_with_content(
            NodeType::ListItem,
            Fragment::from(vec![para]),
        );
        let bullet_list = Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![list_item]),
        );
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![bullet_list]),
        );

        // BulletList is at offset 0, size 17 (2 + 2 + 13)
        let list_size = doc.child(0).unwrap().node_size();
        assert_eq!(list_size, 17);

        // gap_from = 2 (after BulletList open + ListItem open)
        // gap_to = 15 (before ListItem close + BulletList close)
        let step = Step::ReplaceAround {
            from: 0,
            to: list_size,
            gap_from: 2,
            gap_to: list_size - 2,
            insert: Slice::empty(),
            structure: true,
        };

        let (new_doc, map) = step.apply(&doc).unwrap();

        // Should be Doc[Paragraph["Hello world"]]
        let first = new_doc.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Paragraph));
        assert_eq!(first.text_content(), "Hello world");

        // Position mapping: old pos 3 (start of "H" inside list) -> new pos 1
        assert_eq!(map.map(3, 1), 1);
    }

    #[test]
    fn wrap_paragraph_in_blockquote() {
        let doc = simple_doc();
        let para_size = doc.child(0).unwrap().node_size();

        let wrapper = make_single_wrapper(NodeType::Blockquote);
        let step = Step::ReplaceAround {
            from: 0,
            to: para_size,
            gap_from: 0,
            gap_to: para_size,
            insert: wrapper,
            structure: true,
        };

        let (new_doc, map) = step.apply(&doc).unwrap();

        let bq = new_doc.child(0).unwrap();
        assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
        let para = bq.child(0).unwrap();
        assert_eq!(para.text_content(), "Hello world");

        // Pos 1 -> 2 (shifted by 1 for blockquote boundary)
        assert_eq!(map.map(1, 1), 2);
    }

    #[test]
    fn wrap_invert_roundtrip() {
        let doc = simple_doc();
        let para_size = doc.child(0).unwrap().node_size();

        let wrapper = make_wrapper(NodeType::BulletList, NodeType::ListItem);
        let step = Step::ReplaceAround {
            from: 0,
            to: para_size,
            gap_from: 0,
            gap_to: para_size,
            insert: wrapper,
            structure: true,
        };

        let (wrapped_doc, _) = step.apply(&doc).unwrap();
        assert_ne!(doc, wrapped_doc);

        let inverse = step.invert(&doc);
        let (restored, _) = inverse.apply(&wrapped_doc).unwrap();
        assert_eq!(doc, restored);
    }

    #[test]
    fn unwrap_invert_roundtrip() {
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("Test")]),
        );
        let list_item = Node::element_with_content(
            NodeType::ListItem,
            Fragment::from(vec![para]),
        );
        let bullet_list = Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![list_item]),
        );
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![bullet_list]),
        );

        let list_size = doc.child(0).unwrap().node_size();
        let step = Step::ReplaceAround {
            from: 0,
            to: list_size,
            gap_from: 2,
            gap_to: list_size - 2,
            insert: Slice::empty(),
            structure: true,
        };

        let (unwrapped, _) = step.apply(&doc).unwrap();
        let inverse = step.invert(&doc);
        let (restored, _) = inverse.apply(&unwrapped).unwrap();
        assert_eq!(doc, restored);
    }

    #[test]
    fn stepmap_multi_range() {
        // Wrapping: insert 2 boundaries at start and 2 at end of a 7-size node
        let map = StepMap::multi(vec![(0, 0, 2), (9, 0, 2)]);

        // Before content: stays
        assert_eq!(map.map(0, -1), 0);
        // Inside content: shifts by 2 (opening wrappers)
        assert_eq!(map.map(1, 1), 3);
        assert_eq!(map.map(6, 1), 8);
        // After content (old pos 7): shifts by 2, then at second insertion point
        assert_eq!(map.map(7, -1), 9);
    }

    #[test]
    fn stepmap_multi_range_unwrap() {
        // Unwrapping: remove 2 boundaries from start and 2 from end
        let map = StepMap::multi(vec![(0, 2, 0), (7, 2, 0)]);

        // Inside content (old pos 3): shifted by -2
        assert_eq!(map.map(3, 1), 1);
        // After content (old pos 9): shifted by -2, then at second range, -2 more
        // Actually: range 1 (0, 2, 0): pos 9 > 2, shift to 9-2=7.
        // range 2 (7, 2, 0): pos 7 == start and old_size==2, end=9.
        // 7 is at start, bias 1 -> stays at 7? No, pos 7 == start with bias -1 stays at 7.
        // Let me check: pos=7, start=7, end=9, old_size=2, new_size=0.
        // pos is inside changed range. pos == start && bias <= 0 -> stays at start = 7.
        // With bias 1: pos is inside, goes to start + new_size = 7 + 0 = 7.
        // Actually for pos 9 (after list): range 1: 9 > 2, shift to 7. range 2: 7 is inside [7..9], not > end.
        // For bias 1: pos==7, start==7, but old_size != 0, so it's "inside the range".
        // bias >= 0, so... let's check: pos==start && bias<=0 -> no (bias is 1).
        // pos==end (9)? 7 != 9. So we fall to "inside deleted range" with bias > 0: pos = 7 + 0 = 7.
        assert_eq!(map.map(9, 1), 7);
    }
}
