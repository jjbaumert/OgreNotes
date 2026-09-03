// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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

// ─── Cross-doc-swap remap (remote / concurrent edits) ───────────

/// One model position, linearized for diffing two document snapshots.
///
/// Marks are deliberately absent from `Char`: a mark-only change must not
/// register as a position shift. Container boundaries are keyed by
/// `blockId`, so a re-typed block (paragraph→heading, same id) is also a
/// no-op shift, while a genuinely inserted/removed block — which carries a
/// fresh/absent id — is detected.
#[derive(PartialEq, Eq)]
enum PosAtom {
    Char(char),
    Open(String),
    OpenTyped(NodeType),
    Close(String),
    CloseTyped(NodeType),
    Leaf(NodeType),
}

fn push_pos_atoms(node: &Node, out: &mut Vec<PosAtom>) {
    match node {
        Node::Text { text, .. } => out.extend(text.chars().map(PosAtom::Char)),
        Node::Element {
            node_type, content, ..
        } => {
            if node_type.is_leaf() {
                out.push(PosAtom::Leaf(*node_type));
            } else {
                out.push(match node.block_id() {
                    Some(id) => PosAtom::Open(id.to_string()),
                    None => PosAtom::OpenTyped(*node_type),
                });
                for child in &content.children {
                    push_pos_atoms(child, out);
                }
                out.push(match node.block_id() {
                    Some(id) => PosAtom::Close(id.to_string()),
                    None => PosAtom::CloseTyped(*node_type),
                });
            }
        }
    }
}

/// Linearize a document's content into one [`PosAtom`] per model position.
/// `result.len() == doc.content_size()`, and index `i` is model position
/// `i` (ProseMirror-style: a container's open token sits at the block's
/// start, its content follows, then the close token).
fn linearize_positions(doc: &Node) -> Vec<PosAtom> {
    let mut out = Vec::new();
    if let Node::Element { content, .. } = doc {
        for child in &content.children {
            push_pos_atoms(child, &mut out);
        }
    }
    out
}

/// Build a [`StepMap`] mapping positions in `old` to positions in `new`
/// for a **wholesale post-merge doc swap** — a remote/concurrent edit that
/// replaces the document under the local editor. Feeding this to
/// [`HistoryPlugin::remap_through`](super::plugins::HistoryPlugin::remap_through)
/// carries the recorded undo/redo stack (and comment anchors) into the new
/// coordinate space, so local undo stays sound after a collaborator edits.
///
/// Char-precise: both docs are linearized to one atom per position and the
/// common prefix/suffix are trimmed. A single contiguous edit — the
/// dominant remote case — yields the *exact* replaced range. Multiple
/// disjoint edits batched into one debounced window collapse into a single
/// over-broad range: positions *outside* it still map exactly; positions
/// inside are approximate and are caught by the fail-safe in
/// `HistoryPlugin::undo` (which declines rather than applies a step at a
/// bad offset) instead of corrupting the document.
pub fn step_map_for_doc_swap(old: &Node, new: &Node) -> StepMap {
    let old_atoms = linearize_positions(old);
    let new_atoms = linearize_positions(new);

    let min_len = old_atoms.len().min(new_atoms.len());
    let mut prefix = 0;
    while prefix < min_len && old_atoms[prefix] == new_atoms[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < min_len - prefix
        && old_atoms[old_atoms.len() - 1 - suffix] == new_atoms[new_atoms.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let old_mid = &old_atoms[prefix..old_atoms.len() - suffix];
    let new_mid = &new_atoms[prefix..new_atoms.len() - suffix];
    let ranges = diff_middle(old_mid, new_mid, prefix);

    match ranges.len() {
        0 => StepMap::empty(),
        1 => {
            let (start, removed, added) = ranges[0];
            StepMap::new(start, removed, added)
        }
        _ => {
            // StepMap::multi wants output-adjusted starts: each range's
            // start sits in the coordinate space *after* the size changes
            // of all prior ranges. Our diff produces old-coordinate ranges,
            // so shift each start by the running (added − removed) delta.
            let mut adjusted = Vec::with_capacity(ranges.len());
            let mut delta: isize = 0;
            for (start, removed, added) in ranges {
                adjusted.push(((start as isize + delta) as usize, removed, added));
                delta += added as isize - removed as isize;
            }
            StepMap::multi(adjusted)
        }
    }
}

/// Upper bound on the LCS table size (`|old_mid| × |new_mid|` cells) before
/// we fall back to a single over-broad range. Only disjoint edits far apart
/// in a large document reach this; the fail-safe in `HistoryPlugin::undo`
/// keeps that fallback safe.
const MAX_LCS_CELLS: usize = 1 << 18; // 262_144 cells (~1 MB of u32)

/// Minimal edit script over the already-trimmed differing middle, returned
/// as ascending, non-overlapping `(old_start, removed, added)` ranges in old
/// coordinates (`base` = common-prefix length). A pure insert or delete is
/// one range; otherwise an LCS alignment keeps disjoint edits in separate
/// ranges so each maps exactly (no over-broad collapse).
fn diff_middle(
    old_mid: &[PosAtom],
    new_mid: &[PosAtom],
    base: usize,
) -> Vec<(usize, usize, usize)> {
    let m = old_mid.len();
    let n = new_mid.len();
    if m == 0 && n == 0 {
        return Vec::new();
    }
    if m == 0 {
        return vec![(base, 0, n)];
    }
    if n == 0 {
        return vec![(base, m, 0)];
    }
    if m.saturating_mul(n) > MAX_LCS_CELLS {
        // Too large to align precisely — one over-broad range. Positions
        // outside it still map exactly; inside is fail-safe-protected.
        return vec![(base, m, n)];
    }

    // LCS length table: lcs[i][j] = LCS length of old_mid[i..] vs new_mid[j..].
    let mut lcs = vec![vec![0u32; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            lcs[i][j] = if old_mid[i] == new_mid[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    // Walk forward, collapsing each run of deletes/inserts into one range.
    let mut ranges = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut cur_start: Option<usize> = None;
    let (mut removed, mut added) = (0usize, 0usize);
    while i < m && j < n {
        if old_mid[i] == new_mid[j] {
            if let Some(s) = cur_start.take() {
                ranges.push((base + s, removed, added));
                removed = 0;
                added = 0;
            }
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            cur_start.get_or_insert(i);
            removed += 1;
            i += 1;
        } else {
            cur_start.get_or_insert(i);
            added += 1;
            j += 1;
        }
    }
    if i < m {
        cur_start.get_or_insert(i);
        removed += m - i;
    }
    if j < n {
        cur_start.get_or_insert(i);
        added += n - j;
    }
    if let Some(s) = cur_start {
        ranges.push((base + s, removed, added));
    }
    ranges
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

#[cfg(test)]
mod swap_remap_tests {
    //! Tests for `step_map_for_doc_swap` (#151 generalization): the
    //! char-precise StepMap synthesized from an old→new doc swap that
    //! carries the undo stack + anchors across a remote/concurrent edit.
    //! Each case asserts concrete position mappings, including the
    //! invariants that matter most: positions *before* an edit are
    //! identity, positions *after* shift by exactly (added − removed),
    //! and type-only / mark-only changes are pure no-ops.
    use super::*;
    use crate::editor::model::MarkType;
    use std::collections::HashMap;

    fn block(ty: NodeType, id: &str, child: Node) -> Node {
        let mut attrs = HashMap::new();
        attrs.insert("blockId".to_string(), id.to_string());
        Node::element_with_attrs(ty, attrs, Fragment::from(vec![child]))
    }
    fn para(id: &str, text: &str) -> Node {
        block(NodeType::Paragraph, id, Node::text(text))
    }
    fn doc_of(blocks: Vec<Node>) -> Node {
        Node::element_with_content(NodeType::Doc, Fragment::from(blocks))
    }
    fn assert_identity(m: &StepMap, doc: &Node) {
        for p in 0..=doc.content_size() {
            assert_eq!(m.map(p, 1), p, "right-bias identity at {p}");
            assert_eq!(m.map(p, -1), p, "left-bias identity at {p}");
        }
    }

    #[test]
    fn no_change_is_identity() {
        let d = doc_of(vec![para("a", "Hello"), para("b", "World")]);
        assert_identity(&step_map_for_doc_swap(&d, &d), &d);
    }

    #[test]
    fn insert_char_midblock_is_char_exact() {
        // "Hello" -> "HeXllo": one char inserted at model position 3.
        let old = doc_of(vec![para("a", "Hello")]);
        let new = doc_of(vec![para("a", "HeXllo")]);
        let m = step_map_for_doc_swap(&old, &new);
        assert_eq!(m.map(2, 1), 2, "before the insert: unchanged");
        assert_eq!(m.map(3, -1), 3, "at the insert, left bias: stays");
        assert_eq!(m.map(3, 1), 4, "at the insert, right bias: after inserted char");
        assert_eq!(m.map(5, 1), 6, "after the insert: +1");
        assert_eq!(m.map(6, 1), 7, "block close: +1");
    }

    #[test]
    fn delete_char_midblock() {
        // "Hello" -> "Hllo": delete the 'e' at position 2.
        let old = doc_of(vec![para("a", "Hello")]);
        let new = doc_of(vec![para("a", "Hllo")]);
        let m = step_map_for_doc_swap(&old, &new);
        assert_eq!(m.map(1, 1), 1, "before delete: unchanged");
        assert_eq!(m.map(6, 1), 5, "after delete: -1");
    }

    #[test]
    fn multibyte_insert_counts_chars_not_bytes() {
        // "a😀b" -> "a😀cb". If positions were byte-based the emoji would
        // count 4 and every assertion below would be off. char-precise.
        let old = doc_of(vec![para("a", "a😀b")]);
        let new = doc_of(vec![para("a", "a😀cb")]);
        let m = step_map_for_doc_swap(&old, &new);
        // a@1 😀@2 b@3 close@4 (emoji is ONE position).
        assert_eq!(m.map(2, 1), 2, "before insert (right after emoji start): unchanged");
        assert_eq!(m.map(3, 1), 4, "the 'b' shifts right by one char");
        assert_eq!(m.map(4, 1), 5, "block close shifts by one char");
    }

    #[test]
    fn insert_block_in_middle_shifts_following_blocks() {
        let old = doc_of(vec![para("a", "X"), para("c", "Y")]);
        let new = doc_of(vec![para("a", "X"), para("b", "Z"), para("c", "Y")]);
        let m = step_map_for_doc_swap(&old, &new);
        // A occupies 0..3 (Open,X,Close); inserted B has node_size 3.
        assert_eq!(m.map(2, 1), 2, "inside block A: unchanged");
        assert_eq!(m.map(4, 1), 7, "text in block C: +node_size(B)=+3");
    }

    #[test]
    fn delete_block_in_middle() {
        let old = doc_of(vec![para("a", "X"), para("b", "Z"), para("c", "Y")]);
        let new = doc_of(vec![para("a", "X"), para("c", "Y")]);
        let m = step_map_for_doc_swap(&old, &new);
        assert_eq!(m.map(2, 1), 2, "inside block A: unchanged");
        assert_eq!(m.map(7, 1), 4, "text in block C: -node_size(B)=-3");
    }

    #[test]
    fn append_block_at_end_leaves_earlier_positions_fixed() {
        // The #151 invariant, in swap form: an end-append must not shift
        // any position inside the existing content.
        let old = doc_of(vec![para("a", "Hi")]);
        let new = doc_of(vec![para("a", "Hi"), para("b", "Yo")]);
        let m = step_map_for_doc_swap(&old, &new);
        for p in 0..=old.content_size() {
            assert_eq!(m.map(p, -1), p, "left-bias identity within original content at {p}");
        }
    }

    #[test]
    fn type_change_same_block_id_is_no_op() {
        // paragraph -> heading, same blockId: containers are keyed by id,
        // so no position shifts (node_size is unchanged either way).
        let old = doc_of(vec![para("a", "Hi")]);
        let new = doc_of(vec![block(NodeType::Heading, "a", Node::text("Hi"))]);
        assert_identity(&step_map_for_doc_swap(&old, &new), &old);
    }

    #[test]
    fn mark_only_change_is_no_op() {
        // Same text, gains a bold mark: marks carry no position, so identity.
        let old = doc_of(vec![para("a", "Hello")]);
        let new = doc_of(vec![block(
            NodeType::Paragraph,
            "a",
            Node::text_with_marks("Hello", vec![Mark::new(MarkType::Bold)]),
        )]);
        assert_identity(&step_map_for_doc_swap(&old, &new), &old);
    }

    #[test]
    fn two_disjoint_inserts_map_each_region_exactly() {
        // "abcdef" -> "aXbcdeYf": insert X after 'a' AND Y after 'e' — two
        // separate edits in one swap. Each region must map exactly, NOT
        // collapse into one over-broad range (the #2 multi-range win).
        let old = doc_of(vec![para("a", "abcdef")]);
        let new = doc_of(vec![para("a", "aXbcdeYf")]);
        let m = step_map_for_doc_swap(&old, &new);
        // old: Open@0 a@1 b@2 c@3 d@4 e@5 f@6 Close@7
        assert_eq!(m.map(1, -1), 1, "before either insert: unchanged");
        assert_eq!(m.map(2, 1), 3, "'b' is past the first insert: +1");
        assert_eq!(m.map(5, 1), 6, "'e' is between the two inserts: +1");
        assert_eq!(m.map(6, 1), 8, "'f' is past both inserts: +2");
        assert_eq!(m.map(7, 1), 9, "block close: +2");
    }

    #[test]
    fn disjoint_delete_and_append_each_exact() {
        // "Hello World" -> "ello WorldX": delete leading 'H' AND append 'X'.
        // Positions in the shared middle shift −1; the trailing boundary
        // nets to 0 (−1 from the delete, +1 from the append).
        let old = doc_of(vec![para("a", "Hello World")]);
        let new = doc_of(vec![para("a", "ello WorldX")]);
        let m = step_map_for_doc_swap(&old, &new);
        assert_eq!(m.map(2, -1), 1, "char after the deleted H shifts -1");
        assert_eq!(m.map(11, -1), 10, "last original char shifts -1");
        assert_eq!(m.map(12, 1), 12, "close: -1 delete + 1 append = net 0");
    }
}

// Native-only: proptest doesn't build for wasm32 (see frontend/Cargo.toml).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod swap_remap_prop_tests {
    //! Property tests for `step_map_for_doc_swap` over random documents.
    //! Two layers: an *exact oracle* for unambiguous edits (a unique
    //! sentinel insert must reproduce the ground-truth StepMap on every
    //! position), and *structural invariants* that any map must satisfy
    //! for an arbitrary edit (monotonicity, correct total length shift).
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashMap;

    fn doc_from(blocks: &[(String, String)]) -> Node {
        let children = blocks
            .iter()
            .map(|(id, text)| {
                let mut attrs = HashMap::new();
                attrs.insert("blockId".to_string(), id.clone());
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    attrs,
                    Fragment::from(vec![Node::text(text)]),
                )
            })
            .collect();
        Node::element_with_content(NodeType::Doc, Fragment::from(children))
    }

    /// Build same-id paragraphs `b0, b1, …` from a list of texts.
    fn id_blocks(texts: &[String]) -> Vec<(String, String)> {
        texts
            .iter()
            .enumerate()
            .map(|(k, t)| (format!("b{k}"), t.clone()))
            .collect()
    }

    proptest! {
        /// A unique sentinel insert (chars X/Y/Z, never present in the
        /// {a,b,c} base) is unambiguous, so the synthesized map must equal
        /// the exact ground-truth `StepMap` on EVERY position and bias —
        /// even when surrounding base chars repeat.
        #[test]
        fn unique_sentinel_insert_maps_exactly(
            texts in proptest::collection::vec("[abc]{0,8}", 1..=3),
            block_pick in 0usize..3,
            offset_pick in 0usize..64,
            sentinel in "[XYZ]{1,3}",
        ) {
            let blocks = id_blocks(&texts);
            let bidx = block_pick % blocks.len();
            let blen = blocks[bidx].1.chars().count();
            let offset = offset_pick % (blen + 1);

            // new = old with `sentinel` inserted into block bidx at `offset`.
            let mut new_blocks = blocks.clone();
            let mut chars: Vec<char> = new_blocks[bidx].1.chars().collect();
            for (k, c) in sentinel.chars().enumerate() {
                chars.insert(offset + k, c);
            }
            new_blocks[bidx].1 = chars.into_iter().collect();

            let old = doc_from(&blocks);
            let new = doc_from(&new_blocks);

            // Ground truth: absolute model position of (bidx, offset).
            let mut abs = 0usize;
            for b in &blocks[..bidx] {
                abs += b.1.chars().count() + 2; // paragraph node_size
            }
            abs += 1 + offset; // +1 for the block's open token
            let truth = StepMap::new(abs, 0, sentinel.chars().count());

            let map = step_map_for_doc_swap(&old, &new);
            for p in 0..=old.content_size() {
                for bias in [-1i32, 1] {
                    prop_assert_eq!(
                        map.map(p, bias),
                        truth.map(p, bias),
                        "pos={} bias={} abs={} added={}",
                        p, bias, abs, sentinel.chars().count()
                    );
                }
            }
        }

        /// For an arbitrary edit (independent random block texts under the
        /// same ids), the map must be monotone non-decreasing and shift the
        /// document end by exactly the net length change.
        #[test]
        fn map_is_monotone_and_total_length_correct(
            texts_old in proptest::collection::vec("[abc]{0,8}", 1..=3),
            texts_new in proptest::collection::vec("[abc]{0,8}", 1..=3),
        ) {
            let n = texts_old.len().min(texts_new.len());
            let old = doc_from(&id_blocks(&texts_old[..n].to_vec()));
            let new = doc_from(&id_blocks(&texts_new[..n].to_vec()));
            let map = step_map_for_doc_swap(&old, &new);

            let old_size = old.content_size();
            let mut prev = 0usize;
            for p in 0..=old_size {
                let mapped = map.map(p, 1);
                prop_assert!(mapped >= prev, "non-monotone at {}: {} < {}", p, mapped, prev);
                prev = mapped;
            }
            prop_assert_eq!(map.map(old_size, 1), new.content_size());
        }
    }
}
