// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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
                // yrs Doc defaults to OffsetKind::Bytes, so format() expects
                // byte offsets into the UTF-8 content.
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

            // #92 Option B safety latch: strategy 3 of find_match
            // ("container-tag fallback") assumes every container yrs
            // Element written by post-d92dac4 code carries a
            // `blockId` attribute. The model constructors
            // (`Node::element`, `element_with_content`,
            // `element_with_attrs`) all gate the assignment on
            // `NodeType::needs_block_id()`, which is exhaustive over
            // every NodeType variant — so the invariant holds
            // through the normal construction paths. This
            // debug_assert catches the case where a future caller
            // builds a `Node::Element { ... }` literal directly
            // without going through the constructors, which would
            // silently produce a blockId-less yrs Element that
            // strategy 3 could later false-match against. Compiles
            // out in release.
            debug_assert!(
                !node_type.needs_block_id() || attrs.contains_key("blockId"),
                "write_node: container element {:?} written without blockId — \
                 strategy 3 of find_match assumes every container yrs Element \
                 carries a blockId. If this fires, the call site constructing \
                 this node bypassed the model constructors (Node::element et al).",
                node_type
            );

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
    let _ = sync_model_to_ydoc_diffed(ydoc, new_doc, None);
}

/// Like [`sync_model_to_ydoc`], but skips subtrees that are unchanged
/// since `last_synced` — the normalized doc this function returned the
/// previous time the caller synced this ydoc (#121: a one-cell commit
/// used to walk every node of the doc through yrs attribute/text reads).
///
/// `last_synced` must reflect the ydoc's content as last seen by the
/// caller: store this function's return value after a local sync, and
/// refresh from `read_doc_from_ydoc` after applying a remote update.
/// Old/new children are paired positionally (under that invariant, the
/// ydoc child at index `i` is `last_synced`'s child `i`). The skip is
/// safe even when a concurrent remote update has invalidated the
/// invariant: equality with `last_synced` means we have no *local*
/// change to contribute for that subtree, so skipping at worst
/// preserves the remote content — where a full sync would have
/// overwritten it with our identical-to-stale state.
pub fn sync_model_to_ydoc_diffed(
    ydoc: &Doc,
    new_doc: &Node,
    last_synced: Option<&Node>,
) -> Node {
    let normalized = super::model::normalize_doc(new_doc);
    {
        let mut txn = ydoc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");

        let new_children = match &normalized {
            Node::Element { content, .. } => &content.children,
            Node::Text { .. } => return normalized,
        };
        let old_children = match last_synced {
            Some(Node::Element { content, .. }) => Some(content.children.as_slice()),
            _ => None,
        };

        sync_children(&mut txn, &fragment, new_children, old_children);
    }
    normalized
}


/// #92: whether two docs have the identical ordered list of top-level
/// blockIds. This is the safety predicate for folding a possibly-stale
/// editor model into a ydoc that just applied a remote update:
/// `sync_children` treats the model's child list as authoritative, so a
/// fold while the lists differ would delete a remotely-added block (see
/// the `fold_after_remote_apply_deletes_peer_content` tripwire). When
/// the lists match, reconciliation can only touch per-block subtrees —
/// blocks the user hasn't edited are skipped by the equality check and
/// remote content inside them survives.
pub fn same_top_level_block_ids(a: &Node, b: &Node) -> bool {
    fn ids(doc: &Node) -> Option<Vec<&str>> {
        match doc {
            Node::Element { content, .. } => Some(
                content
                    .children
                    .iter()
                    .map(|c| match c {
                        Node::Element { attrs, .. } => {
                            attrs.get("blockId").map(|s| s.as_str()).unwrap_or("")
                        }
                        Node::Text { .. } => "",
                    })
                    .collect(),
            ),
            Node::Text { .. } => None,
        }
    }
    match (ids(a), ids(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Sync a list of model children into a yrs container.
/// Uses blockId-based matching to minimize yrs operations.
fn sync_children<C: XmlFragment>(
    txn: &mut yrs::TransactionMut<'_>,
    container: &C,
    new_children: &[Node],
    old_children: Option<&[Node]>,
) {
    let (actions, matched, yrs_blocks) = match_children(txn, container, new_children);

    // Diagnostic counters for the "every keystroke produces a 60 KB
    // rewrite" pathology — see crate::observability docstrings on
    // SYNC_CALLS / SYNC_MODEL_BLOCKS / SYNC_MATCHED_BLOCKS for the
    // ratios that pin which path is failing.
    crate::observability::inc(crate::observability::SYNC_CALLS);
    crate::observability::add(
        crate::observability::SYNC_MODEL_BLOCKS,
        new_children.len() as u64,
    );
    let matched_count = matched.iter().filter(|m| **m).count();
    crate::observability::add(
        crate::observability::SYNC_MATCHED_BLOCKS,
        matched_count as u64,
    );

    remove_unmatched(txn, container, &matched, &yrs_blocks, old_children);
    apply_actions(txn, container, &actions, &matched, old_children);
}

/// Match model children to existing yrs blocks by blockId (or tag for leaf atoms).
/// Returns the SyncActions, a bitmask of which yrs blocks were matched, and
/// the per-block identity info (`remove_unmatched` needs the blockIds to
/// scope deletions against the caller's baseline).
fn match_children<'a, C: XmlFragment>(
    txn: &yrs::TransactionMut<'_>,
    container: &C,
    new_children: &'a [Node],
) -> (Vec<SyncAction<'a>>, Vec<bool>, Vec<YrsBlockInfo>) {
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

    (actions, matched, yrs_blocks)
}

/// Try to find a matching yrs block for a model node.
///
/// Three matching strategies are tried in order:
///
/// 1. **Real blockId match** — the post-d92dac4 happy path. Both
///    sides carry a real blockId; lookup in `block_id_map`.
///
/// 2. **Leaf-tag fallback** — for atomic leaves (`HorizontalRule`,
///    `HardBreak`, `Image`) that have no blockId in yrs by
///    construction (their model constructors don't assign one). Tag
///    alone is sufficient identity because these are interchangeable.
///
/// 3. **Container-tag fallback** (#92 Option B) — for pre-d92dac4
///    docs whose container Elements (BulletList, Table, TableRow,
///    etc.) were persisted without blockId attributes. `read_element`
///    already synthesizes a fresh nanoid into the model node's
///    attrs via `element_with_attrs`'s `or_insert_with(generate_
///    block_id)`. So the model side has a real blockId, but the
///    yrs side doesn't yet; the blockId map miss in step 1 doesn't
///    mean "no match," it means "we generated a new nanoid for a
///    legacy block." This branch finds the first unmatched
///    blockId-less yrs Element with the same tag.
///
///    On the next `sync_block_content` call, `sync_attrs` writes the
///    model's nanoid back to yrs as a real `blockId` attribute. From
///    then on the block is fully migrated and step 1 matches it. So
///    legacy docs heal incrementally on first edit. See #92 for the
///    full design.
///
///    The fallback is safe for post-d92dac4 docs because their yrs
///    Elements all have blockIds, so `info.block_id.is_none()`
///    filters them out — a "really new" Insert in a clean doc still
///    correctly takes the Insert path.
///
///    **Edge case — reorder-before-heal.** If a user opens a legacy
///    doc and reorders two same-tag blockId-less siblings *before*
///    any healing edit lands (e.g., drag-drops two BulletLists past
///    each other as the very first action), strategy 3 walks
///    yrs_blocks in original order and assigns the first unmatched
///    matching tag to each model child. The model's reordered B
///    ends up logically pointing at yrs index 0 (which holds A's
///    content); sync_block_content then overwrites yrs[0] with B's
///    content and yrs[1] with A's content. The end state is correct
///    — yrs now has [B, A] with proper blockIds — but the first
///    sync emits a large update (every byte of both containers
///    rewritten). Subsequent edits are fast because both blocks
///    now have real blockIds and take strategy 1. This is bounded
///    one-time cost and matches the "first edit on a legacy doc is
///    expensive" property the durable-migration follow-up will
///    eventually retire.
fn find_match(
    node: &Node,
    block_id_map: &HashMap<String, usize>,
    yrs_blocks: &[YrsBlockInfo],
    matched: &[bool],
) -> Option<usize> {
    let bid = node.block_id().map(|s| s.to_string());
    let tag = node.node_type().map(node_type_to_tag);

    // Strategy 1: real blockId match.
    if let Some(ref bid) = bid {
        if let Some(&idx) = block_id_map.get(bid) {
            if !matched[idx] {
                return Some(idx);
            }
        }
    }

    // Strategy 2: leaf-tag fallback for atomic leaves whose
    // constructors never assigned a blockId on either side. This
    // local list MUST mirror `NodeType::is_leaf()` (schema.rs) —
    // a divergent list silently drops a bare leaf into
    // Insert-Delete churn, breaking CRDT identity across syncs.
    let is_leaf = matches!(
        node.node_type(),
        Some(
            NodeType::HorizontalRule
                | NodeType::HardBreak
                | NodeType::Image
                | NodeType::Embed
                | NodeType::CalendarEvent
                | NodeType::KanbanCard
        )
    );
    if bid.is_none() && is_leaf {
        return yrs_blocks.iter().enumerate().find(|(i, info)| {
            !matched[*i] && info.block_id.is_none() && info.tag.as_deref() == tag
        }).map(|(i, _)| i);
    }

    // Strategy 3 (#92 Option B): container-tag fallback for
    // pre-d92dac4 legacy blocks. Triggers when the model node
    // carries a blockId (set by `element_with_attrs` at read time)
    // but the blockId-map lookup missed — meaning the yrs side
    // has no blockId attribute for any block. Find the first
    // unmatched blockId-less yrs Element with the same tag.
    if bid.is_some() && !is_leaf {
        let hit = yrs_blocks.iter().enumerate().find(|(i, info)| {
            !matched[*i] && info.block_id.is_none() && info.tag.as_deref() == tag
        }).map(|(i, _)| i);
        if hit.is_some() {
            // #96: count each legacy block healed via the container-tag
            // fallback. Purely observational — the matching logic is
            // unchanged. The counter is the drain gauge for eventually
            // retiring this path (see observability::BLOCKID_CONTAINER_FALLBACK).
            crate::observability::inc(crate::observability::BLOCKID_CONTAINER_FALLBACK);
        }
        return hit;
    }

    None
}

/// Remove yrs blocks that weren't matched to any model node.
/// Iterates in reverse to keep indices stable during removal.
/// Baseline-scoped (#92 follow-up): when the caller supplies
/// `old_children` (its own last-known view of this container), an
/// unmatched live block is only deleted if the caller's baseline *also*
/// referenced its blockId. An unmatched block absent from BOTH the new
/// model and the baseline is one the caller never had the chance to
/// observe — most commonly a concurrent remote update applied to this
/// container after the baseline was captured (two WS frames landing
/// back-to-back before the swap timer fires, or a local send racing a
/// remote apply). Treating "the model doesn't mention it" as
/// authoritative for deletion in that case would destroy the peer's
/// content — the model literally could not have listed a block it has
/// never seen — and the observer would propagate the deletion to every
/// client. A block present in the baseline but dropped from the model
/// is a deliberate LOCAL deletion and still syncs. Blocks without a
/// real blockId (pre-migration legacy containers matched via
/// `find_match`'s strategy-3 fallback) keep the prior
/// unconditional-delete behavior — they aren't identifiable against a
/// baseline by blockId, and refusing would deadlock the healing path.
fn remove_unmatched<C: XmlFragment>(
    txn: &mut yrs::TransactionMut<'_>,
    container: &C,
    matched: &[bool],
    yrs_blocks: &[YrsBlockInfo],
    old_children: Option<&[Node]>,
) {
    let old_ids: Option<std::collections::HashSet<&str>> =
        old_children.map(|old| old.iter().filter_map(|n| n.block_id()).collect());
    for i in (0..matched.len()).rev() {
        if matched[i] {
            continue;
        }
        if let (Some(old_ids), Some(bid)) = (&old_ids, yrs_blocks[i].block_id.as_deref()) {
            if !old_ids.contains(bid) {
                continue; // unknown to the caller's baseline — keep it
            }
        }
        container.remove(txn, i as u32);
    }
}

/// Write the matched/new actions to the yrs container.
/// Uses a fast path when matched blocks are already in their target order,
/// falls back to clearing and rewriting everything when blocks were reordered.
///
/// ## Which user edits take the slow path? (#93 catalog)
///
/// The slow path fires when `reused_indices.windows(2).all(|w| w[0] < w[1])`
/// returns false — i.e. any two consecutive matched blocks point at yrs
/// positions that aren't strictly ascending. In user terms:
///
/// 1. **Drag-and-drop reorder** of any block at any level — top-level
///    paragraph swap, list-item move within a bullet/ordered/task list,
///    table-row move. The model post-edit names the same blockIds in a
///    different order, so `find_match` resolves each block but the
///    reused-index sequence is non-monotonic.
/// 2. **Sort table rows** — same reason as drag-drop. Every cycle in
///    the row permutation translates to an out-of-order pair.
/// 3. **Paste a fragment that contains existing blockIds** — rare in
///    practice (most pastes mint fresh ids) but possible if a user
///    cuts then pastes elsewhere in the same doc.
///
/// Routine text edits — typing a character, deleting one, inserting a
/// new block at the end — DO NOT take the slow path. The size-budget
/// tests (`single_char_edit_*_under_budget`) and the sustained-typing
/// regression test (`sustained_typing_stays_within_budget_and_fast_path`)
/// pin that contract.
///
/// The slow path is correct but heavy: it rewrites every block from
/// scratch, producing the same ~60 KB-per-edit pattern that the d92dac4
/// fix arc set out to kill. The Phase-1 tripwire metric
/// (`client.collab.sync_slow_path_total`) plus the unit tests in this
/// file make a future regression of the fast-path predicate observable
/// in production telemetry AND blocked in CI. Replacing the wholesale
/// rewrite with a real `compute_minimum_moves` algorithm is the
/// long-term proper fix — tracked separately in the issue.
fn apply_actions<C: XmlFragment>(
    txn: &mut yrs::TransactionMut<'_>,
    container: &C,
    actions: &[SyncAction],
    matched: &[bool],
    old_children: Option<&[Node]>,
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
                    // #121: positional old/new pairing — at last sync the
                    // ydoc's child `yrs_idx` was written from (or read as)
                    // `old_children[yrs_idx]`. If the model node is equal,
                    // we have no local change to contribute for this
                    // subtree: skip it entirely (no yrs attr/text reads,
                    // no recursion).
                    let old_node = old_children.and_then(|o| o.get(*yrs_idx));
                    if old_node == Some(*node) {
                        continue;
                    }
                    sync_block_content(txn, container, current_pos, node, old_node);
                }
            }
        }
    } else {
        // Slow path: blocks are reordered. Clear and rewrite everything.
        //
        // Phase 1 of #93 — observability for the slow path so a
        // regression of the find_match ordering invariants (or any
        // future change that makes this branch fire on routine
        // edits) is visible. Counter ticks per call; warn fires
        // once per editor session so support bundles surface the
        // first occurrence without being flooded.
        crate::observability::inc(crate::observability::SYNC_SLOW_PATH);
        warn_slow_path_first_time();

        let remaining = container.len(txn);
        if remaining > 0 {
            container.remove_range(txn, 0, remaining);
        }
        for (i, action) in actions.iter().enumerate() {
            write_node(txn, container, i as u32, action.node());
        }
    }
}

/// Once-per-thread tripwire for the apply_actions slow path. Fires
/// `editor::debug::warn` the first time the slow path is taken;
/// subsequent calls in the same session are silent (the counter
/// tracks the rate). WASM is single-threaded so this is
/// effectively per-tab.
fn warn_slow_path_first_time() {
    use std::cell::Cell;
    thread_local! {
        static WARNED: Cell<bool> = const { Cell::new(false) };
    }
    WARNED.with(|w| {
        if !w.get() {
            w.set(true);
            crate::editor::debug::warn(
                "yrs_bridge",
                "slow path taken (apply_actions reorder) — \
                 a real reorder edit is fine; a steady stream of these \
                 means find_match ordering regressed; see GH #93",
            );
        }
    });
}

/// Update the content of an existing yrs block to match the model node.
/// `old_node` is the positionally-paired node from the caller's
/// `last_synced` doc (see `sync_model_to_ydoc_diffed`); equal parts of
/// it license skipping the corresponding yrs reads/writes.
fn sync_block_content<C: XmlFragment>(
    txn: &mut yrs::TransactionMut<'_>,
    container: &C,
    pos: u32,
    model_node: &Node,
    old_node: Option<&Node>,
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

    // #121: decompose the old node once; equal attrs / equal content
    // let us skip the per-node yrs attribute scan and the leaf
    // text+marks comparison, both of which are yrs reads.
    let (old_attrs, old_content) = match old_node {
        Some(Node::Element { attrs, content, node_type, .. })
            if node_type == model_type =>
        {
            (Some(attrs), Some(content))
        }
        _ => (None, None),
    };

    if old_attrs != Some(model_attrs) {
        sync_attrs(txn, el, model_attrs);
    }

    // Container blocks recurse into children; leaf blocks compare
    // text + marks. Live-app blocks (Calendar, Kanban, KanbanColumn)
    // ARE containers — their child elements carry state in
    // attributes rather than text, so the text-and-marks-match
    // short-circuit incorrectly reports "unchanged" and skips the
    // subtree sync. Symptom: adding a KanbanCard or CalendarEvent
    // to an existing block updated the local model but NEVER
    // reached yrs, so refresh restored the pre-add state.
    // Observed on doc `5xBzUM-KS8u_bi8XPcPzN`.
    let is_container = matches!(model_type,
        NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList |
        NodeType::Blockquote | NodeType::ListItem | NodeType::TaskItem |
        NodeType::Table | NodeType::TableRow | NodeType::TableCell | NodeType::TableHeader |
        NodeType::Calendar | NodeType::Kanban | NodeType::KanbanColumn
    );

    if is_container {
        sync_children(
            txn,
            el,
            &model_content.children,
            old_content.map(|c| c.children.as_slice()),
        );
    } else if old_content != Some(model_content)
        && !text_and_marks_match(txn, el, model_node, &model_content.children)
    {
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
        NodeType::Embed => "embed",
        NodeType::Calendar => "calendar",
        NodeType::CalendarEvent => "calendar_event",
        NodeType::Kanban => "kanban",
        NodeType::KanbanColumn => "kanban_column",
        NodeType::KanbanCard => "kanban_card",
        // #148 slice 6 — mention node tag. Matches
        // `crates/collab/src/schema.rs::NodeType::Mention::tag_name`
        // so both sides of the yrs bridge speak the same wire.
        NodeType::Mention => "mention",
        NodeType::Mermaid => "mermaid",
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
        "embed" => Some(NodeType::Embed),
        "calendar" => Some(NodeType::Calendar),
        "calendar_event" => Some(NodeType::CalendarEvent),
        "kanban" => Some(NodeType::Kanban),
        "kanban_column" => Some(NodeType::KanbanColumn),
        "kanban_card" => Some(NodeType::KanbanCard),
        "mention" => Some(NodeType::Mention),
        "mermaid" => Some(NodeType::Mermaid),
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
        MarkType::Subscript => "subscript",
        MarkType::Superscript => "superscript",
        MarkType::Mention => "mention",
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
        "subscript" => Some(MarkType::Subscript),
        "superscript" => Some(MarkType::Superscript),
        "mention" => Some(MarkType::Mention),
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
            MarkType::Link | MarkType::TextColor | MarkType::Highlight | MarkType::Mention => {
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
                MarkType::Link | MarkType::TextColor | MarkType::Highlight | MarkType::Mention => {
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
    fn strategy3_container_fallback_increments_drain_counter() {
        // #96: when a pre-d92dac4 legacy container (no blockId in yrs) is
        // healed by the container-tag fallback, the heal is counted on the
        // drain gauge — without changing the match result. Once this gauge
        // sits at 0 in production, the fallback can be retired.
        let _ = crate::observability::drain(); // clear this thread's buffer

        // Model node: a BulletList container carrying a (synthetic) blockId,
        // exactly as `read_element` produces for a legacy block.
        let mut attrs = HashMap::new();
        attrs.insert("blockId".to_string(), "synthetic-nanoid".to_string());
        let node = Node::element_with_attrs(NodeType::BulletList, attrs, Fragment::empty());

        // yrs side: one blockId-less Element with the matching tag, and an
        // empty blockId map so strategy 1 misses.
        let tag = node_type_to_tag(NodeType::BulletList);
        let yrs_blocks = vec![YrsBlockInfo {
            tag: Some(tag.to_string()),
            block_id: None,
        }];
        let block_id_map: HashMap<String, usize> = HashMap::new();
        let matched = vec![false];

        let hit = find_match(&node, &block_id_map, &yrs_blocks, &matched);
        assert_eq!(hit, Some(0), "strategy 3 must match the legacy block by tag");

        let count = crate::observability::drain()
            .into_iter()
            .find(|(n, _)| *n == crate::observability::BLOCKID_CONTAINER_FALLBACK)
            .map(|(_, v)| v)
            .unwrap_or(0);
        assert_eq!(count, 1, "the fallback heal must increment the drain gauge");
    }

    #[test]
    fn strategy1_real_blockid_match_does_not_count_fallback() {
        // A clean (post-d92dac4) block matches via strategy 1 and must NOT
        // touch the fallback gauge.
        let _ = crate::observability::drain();

        let mut attrs = HashMap::new();
        attrs.insert("blockId".to_string(), "real-id".to_string());
        let node = Node::element_with_attrs(NodeType::BulletList, attrs, Fragment::empty());

        let tag = node_type_to_tag(NodeType::BulletList);
        let yrs_blocks = vec![YrsBlockInfo {
            tag: Some(tag.to_string()),
            block_id: Some("real-id".to_string()),
        }];
        let mut block_id_map: HashMap<String, usize> = HashMap::new();
        block_id_map.insert("real-id".to_string(), 0);
        let matched = vec![false];

        let hit = find_match(&node, &block_id_map, &yrs_blocks, &matched);
        assert_eq!(hit, Some(0), "strategy 1 should match by blockId");

        let count = crate::observability::drain()
            .into_iter()
            .find(|(n, _)| *n == crate::observability::BLOCKID_CONTAINER_FALLBACK)
            .map(|(_, v)| v)
            .unwrap_or(0);
        assert_eq!(count, 0, "strategy 1 must not touch the fallback gauge");
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

    // ─── #121: diff-aware sync (sync_model_to_ydoc_diffed) ─────────
    //
    // The diffed path must produce a ydoc byte-equivalent (at the model
    // level) to two full syncs; the cache only licenses *skipping*
    // writes for subtrees we already synced, never different writes.

    fn spreadsheet_doc(rows: usize, cols: usize, cells: &[(usize, usize, &str)]) -> Node {
        let table_rows: Vec<Node> = (0..rows)
            .map(|r| {
                let tds: Vec<Node> = (0..cols)
                    .map(|c| {
                        let text = cells
                            .iter()
                            .find(|&&(cr, cc, _)| cr == r && cc == c)
                            .map(|&(_, _, t)| t)
                            .unwrap_or("");
                        let mut pattrs = HashMap::new();
                        pattrs.insert("blockId".to_string(), format!("ss:S0:p:{r}:{c}"));
                        let para = Node::element_with_attrs(
                            NodeType::Paragraph,
                            pattrs,
                            if text.is_empty() {
                                Fragment::empty()
                            } else {
                                Fragment::from(vec![Node::text(text)])
                            },
                        );
                        let mut cattrs = HashMap::new();
                        cattrs.insert("blockId".to_string(), format!("ss:S0:c:{r}:{c}"));
                        Node::element_with_attrs(
                            NodeType::TableCell,
                            cattrs,
                            Fragment::from(vec![para]),
                        )
                    })
                    .collect();
                // Deterministic blockIds throughout — `element_with_content`
                // would generate RANDOM ids, which never match across two
                // separately-built docs and would force the Insert+remove
                // path instead of the Reuse path the diff optimizes.
                let mut rattrs = HashMap::new();
                rattrs.insert("blockId".to_string(), format!("ss:S0:r:{r}"));
                Node::element_with_attrs(NodeType::TableRow, rattrs, Fragment::from(tds))
            })
            .collect();
        let mut tattrs = HashMap::new();
        tattrs.insert("blockId".to_string(), "ss:S0:table".to_string());
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Table,
                tattrs,
                Fragment::from(table_rows),
            )]),
        )
    }

    /// Sync doc1 then doc2 through the diffed path (with cache) and the
    /// full path (without); the resulting ydocs must read back equal.
    fn assert_diffed_equals_full(doc1: &Node, doc2: &Node) {
        let ydoc_a = Doc::new();
        let norm1 = sync_model_to_ydoc_diffed(&ydoc_a, doc1, None);
        let _ = sync_model_to_ydoc_diffed(&ydoc_a, doc2, Some(&norm1));

        let ydoc_b = Doc::new();
        sync_model_to_ydoc(&ydoc_b, doc1);
        sync_model_to_ydoc(&ydoc_b, doc2);

        let a = read_doc_from_ydoc(&ydoc_a).unwrap();
        let b = read_doc_from_ydoc(&ydoc_b).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn diffed_single_cell_change_equals_full_sync() {
        let doc1 = spreadsheet_doc(30, 5, &[(0, 0, "head"), (12, 3, "old")]);
        let doc2 = spreadsheet_doc(30, 5, &[(0, 0, "head"), (12, 3, "new")]);
        assert_diffed_equals_full(&doc1, &doc2);
    }

    #[test]
    fn diffed_no_change_equals_full_sync() {
        let doc1 = spreadsheet_doc(10, 4, &[(2, 2, "x")]);
        assert_diffed_equals_full(&doc1, &doc1.clone());
    }

    #[test]
    fn diffed_row_count_change_equals_full_sync() {
        // Structural change: positional old/new pairing misaligns below
        // the insertion point — those subtrees fail the equality check
        // and take the full sync path, which must converge identically.
        let doc1 = spreadsheet_doc(10, 4, &[(2, 2, "x"), (9, 0, "tail")]);
        let doc2 = spreadsheet_doc(12, 4, &[(2, 2, "x"), (9, 0, "tail"), (11, 1, "added")]);
        assert_diffed_equals_full(&doc1, &doc2);
    }

    #[test]
    fn diffed_attrs_only_change_equals_full_sync() {
        let doc1 = spreadsheet_doc(8, 3, &[(1, 1, "v")]);
        let mut doc2 = spreadsheet_doc(8, 3, &[(1, 1, "v")]);
        // Mutate one cell's attrs (e.g. a style key) without touching text.
        if let Node::Element { content, .. } = &mut doc2 {
            if let Node::Element { content: t, .. } = &mut content.children[0] {
                if let Node::Element { content: row, .. } = &mut t.children[1] {
                    if let Node::Element { attrs, .. } = &mut row.children[1] {
                        attrs.insert("cellStyle".to_string(), "bold".to_string());
                    }
                }
            }
        }
        assert_diffed_equals_full(&doc1, &doc2);
    }

    #[test]
    fn diffed_doc_with_leading_paragraph_equals_full_sync() {
        let wrap = |table_doc: Node, para_text: &str| {
            let Node::Element { content, .. } = table_doc else { unreachable!() };
            let mut children = vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text(para_text)]),
            )];
            children.extend(content.children);
            Node::element_with_content(NodeType::Doc, Fragment::from(children))
        };
        let doc1 = wrap(spreadsheet_doc(6, 3, &[(1, 1, "a")]), "note");
        let doc2 = wrap(spreadsheet_doc(6, 3, &[(1, 1, "b")]), "note");
        assert_diffed_equals_full(&doc1, &doc2);
    }

    #[test]
    fn diffed_skip_preserves_concurrent_remote_content() {
        // A peer changed A1 in the ydoc after our last sync (cache is
        // stale). Our local model still equals the cache for that cell —
        // we have no local change to contribute — so the diffed sync
        // must SKIP it and leave the remote content in place, rather
        // than stomping it back to our stale value (which is what an
        // unconditional full diff used to do).
        let local = spreadsheet_doc(5, 3, &[(0, 0, "mine")]);
        let ydoc = Doc::new();
        let norm = sync_model_to_ydoc_diffed(&ydoc, &local, None);

        let remote = spreadsheet_doc(5, 3, &[(0, 0, "theirs")]);
        sync_model_to_ydoc(&ydoc, &remote); // simulate the remote write

        let _ = sync_model_to_ydoc_diffed(&ydoc, &local, Some(&norm));

        let read = read_doc_from_ydoc(&ydoc).unwrap();
        let cell_text = read
            .child(0)
            .and_then(|t| t.child(0))
            .and_then(|row| row.child(0))
            .map(|cell| cell.text_content())
            .unwrap_or_default();
        assert_eq!(cell_text, "theirs");
    }

    #[test]
    fn diffed_leaf_skip_preserves_concurrent_remote_text_change() {
        // Leaf-level variant of the skip-safety test: a peer rewrote a
        // cell's TEXT after our last sync; our model still equals the
        // cache for that leaf, so sync_block_content's old_content
        // guard must skip — preserving the remote text — rather than
        // writing our stale-but-unchanged content over it.
        let local = spreadsheet_doc(3, 2, &[(1, 0, "original")]);
        let ydoc = Doc::new();
        let norm = sync_model_to_ydoc_diffed(&ydoc, &local, None);

        let remote = spreadsheet_doc(3, 2, &[(1, 0, "remote-edit")]);
        sync_model_to_ydoc(&ydoc, &remote);

        let _ = sync_model_to_ydoc_diffed(&ydoc, &local, Some(&norm));

        let read = read_doc_from_ydoc(&ydoc).unwrap();
        let cell_text = read
            .child(0)
            .and_then(|t| t.child(1))
            .and_then(|row| row.child(0))
            .map(|cell| cell.text_content())
            .unwrap_or_default();
        assert_eq!(cell_text, "remote-edit");
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
    fn roundtrip_bold_multibyte_text() {
        // Invariant: marks applied via text_ref.format() must cover the full
        // text for strings containing multi-byte codepoints. yrs defaults to
        // OffsetKind::Bytes, so the length must be in UTF-8 bytes — this
        // test breaks loudly if anyone switches to char count or UTF-16
        // units without updating the Doc offset_kind.
        let s = "café 日本語 \u{1F600}";
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks(
                    s,
                    vec![Mark::new(MarkType::Bold)],
                )]),
            )]),
        );
        let bytes = doc_to_ydoc_bytes(&doc);
        let restored = ydoc_bytes_to_doc(&bytes).unwrap();
        let para = restored.child(0).unwrap();
        assert_eq!(para.text_content(), s);
        // Every child segment that carries text must be bold — if the mark
        // range was short (byte len > char len shortened via saturation), the
        // tail would come back unmarked.
        for i in 0..para.child_count() {
            let child = para.child(i).unwrap();
            if !child.text_content().is_empty() {
                assert!(
                    child.marks().iter().any(|m| m.mark_type == MarkType::Bold),
                    "segment {:?} lost bold mark",
                    child.text_content()
                );
            }
        }
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

    // ─── Update-size budgets (the d92dac4 regression guard) ─────
    //
    // The pre-d92dac4 bug let four container types (BulletList,
    // OrderedList, TaskList, TableRow) fall through `is_commentable`
    // and so never get a structural `blockId`. `find_match` then
    // missed them on every edit and `apply_actions` rewrote the
    // whole list / table from scratch, producing ~60 KB yrs updates
    // for a single typed character.
    //
    // These tests fix the *incremental update size* contract: a
    // single-character insert against a representative doc must
    // encode in well under 1 KiB. The realistic threshold is much
    // smaller (~50-200 bytes); the budget is set well above the
    // expected size and far below the pathological size, so any
    // future change that loses block-id matching for *any*
    // container type fires this guard before it reaches a deploy.
    //
    // If you find yourself wanting to raise the budget: the
    // correct answer is almost certainly "no — the bridge just
    // regressed for some node type." Re-read `needs_block_id` and
    // the `is_leaf` fallback in `find_match` before touching the
    // budget itself.

    /// Upper bound on the yrs update bytes produced by inserting a
    /// single character. Generous — observed in practice ~50–300 B.
    /// Pre-d92dac4 reproduced the bug at ~58–60 KB on real docs.
    const SINGLE_CHAR_EDIT_BUDGET: usize = 1024;

    /// Compute the yrs update bytes produced by `sync_model_to_ydoc`
    /// when we go from `initial` to `updated` against a fresh yrs
    /// Doc seeded with `initial`. The returned blob is the
    /// over-the-wire MSG_UPDATE payload the WS handler would
    /// persist as a single DDB UPDATE# row.
    fn measure_incremental_update(initial: &Node, updated: &Node) -> Vec<u8> {
        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, initial);
        // State vector AFTER the initial seed. The diff produced
        // by the subsequent sync is what would flow on the wire.
        let baseline_sv = ydoc.transact().state_vector();
        sync_model_to_ydoc(&ydoc, updated);
        ydoc.transact().encode_state_as_update_v1(&baseline_sv)
    }

    /// Insert ONE character into the first text node inside the
    /// first paragraph (walks the tree until it finds text). Used
    /// by the budget tests so the modification is the smallest
    /// possible meaningful edit.
    fn insert_one_char_in_first_text(doc: &Node) -> Node {
        fn walk(node: &Node) -> Node {
            match node {
                Node::Text { text, marks } => Node::Text {
                    text: format!("X{text}"),
                    marks: marks.clone(),
                },
                Node::Element { node_type, attrs, content, marks } => {
                    let mut found = false;
                    let new_children: Vec<Node> = content.children.iter().map(|c| {
                        if found { c.clone() } else {
                            let nc = walk(c);
                            if !matches!(c, Node::Text { .. }) {
                                // Recursed into Element; check if we modified
                                // by comparing text content size.
                                if nc.text_content().len() != c.text_content().len() {
                                    found = true;
                                }
                                nc
                            } else {
                                found = true;
                                nc
                            }
                        }
                    }).collect();
                    Node::Element {
                        node_type: *node_type,
                        attrs: attrs.clone(),
                        content: Fragment::from(new_children),
                        marks: marks.clone(),
                    }
                }
            }
        }
        walk(doc)
    }

    #[test]
    fn single_char_edit_in_paragraph_under_budget() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(NodeType::Paragraph,
                    Fragment::from(vec![Node::text("one paragraph")])),
                Node::element_with_content(NodeType::Paragraph,
                    Fragment::from(vec![Node::text("two paragraph")])),
            ]),
        );
        let edited = insert_one_char_in_first_text(&doc);
        let update = measure_incremental_update(&doc, &edited);
        assert!(update.len() < SINGLE_CHAR_EDIT_BUDGET,
            "paragraph-only doc: single-char edit produced {} B (budget {})",
            update.len(), SINGLE_CHAR_EDIT_BUDGET);
    }

    #[test]
    fn single_char_edit_in_bullet_list_under_budget() {
        // Pre-d92dac4 this test would have produced an update
        // proportional to the entire list size — BulletList lacked
        // a blockId so every keystroke rewrote it from scratch.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![
                    Node::element_with_content(NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(NodeType::Paragraph,
                            Fragment::from(vec![Node::text("first item")]))])),
                    Node::element_with_content(NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(NodeType::Paragraph,
                            Fragment::from(vec![Node::text("second item")]))])),
                    Node::element_with_content(NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(NodeType::Paragraph,
                            Fragment::from(vec![Node::text("third item")]))])),
                ]),
            )]),
        );
        let edited = insert_one_char_in_first_text(&doc);
        let update = measure_incremental_update(&doc, &edited);
        assert!(update.len() < SINGLE_CHAR_EDIT_BUDGET,
            "bullet-list doc: single-char edit produced {} B (budget {}) — \
             BulletList may be missing its blockId, regressing #d92dac4",
            update.len(), SINGLE_CHAR_EDIT_BUDGET);
    }

    #[test]
    fn single_char_edit_in_table_under_budget() {
        // Pre-d92dac4 this would also blow up because TableRow has
        // no blockId — sync_block_content on the matched Table
        // recursed into rows, none matched, all were rewritten.
        let make_cell = |text: &str| Node::element_with_content(
            NodeType::TableCell,
            Fragment::from(vec![Node::element_with_content(NodeType::Paragraph,
                Fragment::from(vec![Node::text(text)]))]),
        );
        let make_row = |a: &str, b: &str| Node::element_with_content(
            NodeType::TableRow,
            Fragment::from(vec![make_cell(a), make_cell(b)]),
        );
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Table,
                Fragment::from(vec![
                    make_row("h1", "h2"),
                    make_row("r1c1", "r1c2"),
                    make_row("r2c1", "r2c2"),
                    make_row("r3c1", "r3c2"),
                ]),
            )]),
        );
        let edited = insert_one_char_in_first_text(&doc);
        let update = measure_incremental_update(&doc, &edited);
        assert!(update.len() < SINGLE_CHAR_EDIT_BUDGET,
            "table doc: single-char edit produced {} B (budget {}) — \
             TableRow may be missing its blockId, regressing #d92dac4",
            update.len(), SINGLE_CHAR_EDIT_BUDGET);
    }

    #[test]
    fn single_char_edit_in_mixed_doc_under_budget() {
        // Kitchen-sink: paragraphs + headings + ordered list +
        // task list + table all in one doc. The closest analog to
        // the briefing doc that triggered the field bug, kept short
        // enough to debug from the assertion message.
        let make_para = |t: &str| Node::element_with_content(NodeType::Paragraph,
            Fragment::from(vec![Node::text(t)]));
        let make_li = |t: &str| Node::element_with_content(NodeType::ListItem,
            Fragment::from(vec![make_para(t)]));
        let make_ti = |t: &str| Node::element_with_content(NodeType::TaskItem,
            Fragment::from(vec![make_para(t)]));
        let make_cell = |t: &str| Node::element_with_content(NodeType::TableCell,
            Fragment::from(vec![make_para(t)]));
        let make_row = |a: &str, b: &str| Node::element_with_content(
            NodeType::TableRow,
            Fragment::from(vec![make_cell(a), make_cell(b)]),
        );

        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(NodeType::Heading, Fragment::from(vec![Node::text("Title")])),
                make_para("Intro paragraph."),
                Node::element_with_content(NodeType::OrderedList,
                    Fragment::from(vec![make_li("one"), make_li("two")])),
                Node::element_with_content(NodeType::TaskList,
                    Fragment::from(vec![make_ti("todo 1"), make_ti("todo 2")])),
                Node::element_with_content(NodeType::Table,
                    Fragment::from(vec![make_row("a", "b"), make_row("c", "d")])),
                make_para("Outro paragraph."),
            ]),
        );
        let edited = insert_one_char_in_first_text(&doc);
        let update = measure_incremental_update(&doc, &edited);
        assert!(update.len() < SINGLE_CHAR_EDIT_BUDGET,
            "mixed doc: single-char edit produced {} B (budget {})",
            update.len(), SINGLE_CHAR_EDIT_BUDGET);
    }

    // ─── Slow-path observability (Phase 1 of #93) ───────────────
    //
    // The "blocks reordered" branch of apply_actions is a legitimate
    // path for a real reorder edit (drag-and-drop list item, sort
    // table rows) but should be rare in steady state. The counter
    // `client.collab.sync_slow_path_total` tracks the rate; a
    // non-zero steady-state value means find_match ordering
    // invariants regressed.
    //
    // This test verifies the tripwire fires when the path is
    // intentionally exercised — swapping two paragraphs by their
    // blockId. If a future bridge refactor moves the slow-path
    // emit, this test will catch the omission.

    fn slow_path_counter_value() -> u64 {
        crate::observability::drain()
            .into_iter()
            .find(|(name, _)| *name == crate::observability::SYNC_SLOW_PATH)
            .map(|(_, v)| v)
            .unwrap_or(0)
    }

    /// Build a Doc-shaped Node containing the given top-level blocks,
    /// each with an explicit blockId so find_match resolves them by
    /// identity. Doc itself stays blockId-less (`needs_block_id` is
    /// false for `NodeType::Doc`).
    fn doc_with_blocks(blocks: Vec<Node>) -> Node {
        Node::element_with_content(NodeType::Doc, Fragment::from(blocks))
    }

    fn para_with_id(bid: &str, text: &str) -> Node {
        Node::element_with_attrs(
            NodeType::Paragraph,
            [("blockId".to_string(), bid.to_string())].into(),
            Fragment::from(vec![Node::text(text)]),
        )
    }

    #[test]
    fn deliberate_reorder_triggers_slow_path_counter() {
        // Drain any counters accumulated by prior test setup on
        // this thread so the slow-path delta is clean.
        let _ = crate::observability::drain();

        let para_a = para_with_id("p-a", "alpha");
        let para_b = para_with_id("p-b", "bravo");
        let initial = doc_with_blocks(vec![para_a.clone(), para_b.clone()]);

        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &initial);
        // Discard initial-sync counters; they don't take the slow
        // path (no prior yrs state to reorder against), but we
        // want a clean delta for the reorder step.
        let _ = crate::observability::drain();

        // Swap the two top-level paragraphs by blockId: model is
        // now [p-b, p-a]. yrs container is [p-a, p-b]. match_children
        // resolves both by blockId → reused_indices = [1, 0] →
        // not strictly ascending → slow path fires.
        let reordered = doc_with_blocks(vec![para_b, para_a]);
        sync_model_to_ydoc(&ydoc, &reordered);

        assert_eq!(
            slow_path_counter_value(),
            1,
            "a deliberate two-block reorder should take the slow path exactly once"
        );
    }

    fn list_item_with_id(bid: &str, text: &str) -> Node {
        Node::element_with_attrs(
            NodeType::ListItem,
            [("blockId".to_string(), bid.to_string())].into(),
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text(text)]),
            )]),
        )
    }

    fn bullet_list_with_items(bid: &str, items: Vec<Node>) -> Node {
        Node::element_with_attrs(
            NodeType::BulletList,
            [("blockId".to_string(), bid.to_string())].into(),
            Fragment::from(items),
        )
    }

    fn table_row_with_id(bid: &str, cells: Vec<&str>) -> Node {
        let cell_nodes: Vec<Node> = cells
            .into_iter()
            .map(|t| {
                Node::element_with_content(
                    NodeType::TableCell,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text(t)]),
                    )]),
                )
            })
            .collect();
        Node::element_with_attrs(
            NodeType::TableRow,
            [("blockId".to_string(), bid.to_string())].into(),
            Fragment::from(cell_nodes),
        )
    }

    fn table_with_rows(bid: &str, rows: Vec<Node>) -> Node {
        Node::element_with_attrs(
            NodeType::Table,
            [("blockId".to_string(), bid.to_string())].into(),
            Fragment::from(rows),
        )
    }

    /// #93 catalog entry: drag-and-drop a list item up one slot.
    /// This is the exact shape a user produces by dragging the
    /// second bullet above the first. Slow path must fire — three
    /// reused indices in [0, 2, 1] order, which fails the
    /// strictly-ascending check.
    #[test]
    fn drag_drop_list_item_reorder_triggers_slow_path() {
        let _ = crate::observability::drain();

        let li_a = list_item_with_id("li-a", "apple");
        let li_b = list_item_with_id("li-b", "banana");
        let li_c = list_item_with_id("li-c", "cherry");
        let initial = doc_with_blocks(vec![bullet_list_with_items(
            "ul-1",
            vec![li_a.clone(), li_b.clone(), li_c.clone()],
        )]);

        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &initial);
        let _ = crate::observability::drain();

        // Move "banana" above "apple": [li-b, li-a, li-c]. The
        // bullet-list container itself is unchanged (same blockId,
        // ul-1, fast path on the top level). Inside the list,
        // reused_indices = [1, 0, 2] → slow path.
        let reordered = doc_with_blocks(vec![bullet_list_with_items(
            "ul-1",
            vec![li_b, li_a, li_c],
        )]);
        sync_model_to_ydoc(&ydoc, &reordered);

        assert_eq!(
            slow_path_counter_value(),
            1,
            "drag-drop list-item reorder should take the slow path \
             exactly once (only the inner list container reordered)"
        );
    }

    /// #93 catalog entry: sort table rows. A typical "sort by
    /// column A ascending" reordering produces a permutation of the
    /// existing TableRow blockIds — the same children, different
    /// order. Slow path must fire on the Table container.
    #[test]
    fn table_row_sort_triggers_slow_path() {
        let _ = crate::observability::drain();

        let r1 = table_row_with_id("tr-1", vec!["zebra", "10"]);
        let r2 = table_row_with_id("tr-2", vec!["apple", "20"]);
        let r3 = table_row_with_id("tr-3", vec!["mango", "30"]);
        let initial = doc_with_blocks(vec![table_with_rows(
            "t-1",
            vec![r1.clone(), r2.clone(), r3.clone()],
        )]);

        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &initial);
        let _ = crate::observability::drain();

        // Sort by first column alphabetically: [tr-2, tr-3, tr-1].
        // reused_indices = [1, 2, 0] — not strictly ascending →
        // slow path.
        let sorted = doc_with_blocks(vec![table_with_rows(
            "t-1",
            vec![r2, r3, r1],
        )]);
        sync_model_to_ydoc(&ydoc, &sorted);

        assert_eq!(
            slow_path_counter_value(),
            1,
            "table-row sort should take the slow path exactly once \
             (only the inner Table container reordered)"
        );
    }

    /// Companion guard for the catalog: editing a single cell inside
    /// a table row must NOT take the slow path. If it does, every
    /// keystroke in a spreadsheet-shaped doc would trigger a wholesale
    /// rewrite — a worse pathology than the original d92dac4 bug
    /// because tables are dense with content.
    #[test]
    fn in_cell_text_edit_does_not_trigger_slow_path() {
        let _ = crate::observability::drain();

        let initial = doc_with_blocks(vec![table_with_rows(
            "t-1",
            vec![
                table_row_with_id("tr-1", vec!["alpha", "1"]),
                table_row_with_id("tr-2", vec!["bravo", "2"]),
            ],
        )]);

        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &initial);
        let _ = crate::observability::drain();

        // Edit the first cell of the first row from "alpha" → "alphaX".
        // Row order unchanged; cell content changed.
        let edited = doc_with_blocks(vec![table_with_rows(
            "t-1",
            vec![
                table_row_with_id("tr-1", vec!["alphaX", "1"]),
                table_row_with_id("tr-2", vec!["bravo", "2"]),
            ],
        )]);
        sync_model_to_ydoc(&ydoc, &edited);

        assert_eq!(
            slow_path_counter_value(),
            0,
            "text edit inside a stable table cell must not take the slow path"
        );
    }

    #[test]
    fn fast_path_edits_do_not_trigger_slow_path_counter() {
        // Companion guard: a routine text edit must NOT take the
        // slow path. If the counter ticks here, find_match's
        // ordering predicate has regressed and the d92dac4 bug
        // class is back.
        let _ = crate::observability::drain();

        let initial = doc_with_blocks(vec![
            para_with_id("p-1", "first"),
            para_with_id("p-2", "second"),
            para_with_id("p-3", "third"),
        ]);

        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &initial);
        let _ = crate::observability::drain();

        // Edit the second paragraph's text — block identities and
        // ordering unchanged.
        let edited = doc_with_blocks(vec![
            para_with_id("p-1", "first"),
            para_with_id("p-2", "secondX"),
            para_with_id("p-3", "third"),
        ]);
        sync_model_to_ydoc(&ydoc, &edited);

        assert_eq!(
            slow_path_counter_value(),
            0,
            "a same-order text edit must not take the slow path"
        );
    }

    // ─── Sustained-typing regression (gap #1 of the test-coverage plan) ─────
    //
    // The single-character size-budget tests above prove ONE edit produces a
    // small update on a representative doc. They do not prove 100 sequential
    // edits each stay small — a regression that turns the slow path on after
    // N edits, or that leaks accumulated state into every yrs update, would
    // pass the existing tests and still break the user-visible "edits
    // persist on refresh" contract in steady-state editing.
    //
    // This test types 100 single-character edits against a long-lived ydoc
    // and asserts (a) every per-edit update stays under
    // `SINGLE_CHAR_EDIT_BUDGET`, and (b) `sync_slow_path_total` ticks zero
    // times across the whole loop. Together those would have caught the
    // bridge bug class fixed in d92dac4 on the very first regression.

    #[test]
    fn sustained_typing_stays_within_budget_and_fast_path() {
        // Same kitchen-sink fixture as
        // `single_char_edit_in_mixed_doc_under_budget`: paragraphs,
        // heading, ordered list, task list, table, outro. Closest
        // approximation we have to the briefing-doc shape that
        // originally surfaced the bridge bug.
        let make_para = |t: &str| Node::element_with_content(NodeType::Paragraph,
            Fragment::from(vec![Node::text(t)]));
        let make_li = |t: &str| Node::element_with_content(NodeType::ListItem,
            Fragment::from(vec![make_para(t)]));
        let make_ti = |t: &str| Node::element_with_content(NodeType::TaskItem,
            Fragment::from(vec![make_para(t)]));
        let make_cell = |t: &str| Node::element_with_content(NodeType::TableCell,
            Fragment::from(vec![make_para(t)]));
        let make_row = |a: &str, b: &str| Node::element_with_content(
            NodeType::TableRow,
            Fragment::from(vec![make_cell(a), make_cell(b)]),
        );

        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(NodeType::Heading,
                    Fragment::from(vec![Node::text("Title")])),
                make_para("Intro paragraph."),
                Node::element_with_content(NodeType::OrderedList,
                    Fragment::from(vec![make_li("one"), make_li("two")])),
                Node::element_with_content(NodeType::TaskList,
                    Fragment::from(vec![make_ti("todo 1"), make_ti("todo 2")])),
                Node::element_with_content(NodeType::Table,
                    Fragment::from(vec![make_row("a", "b"), make_row("c", "d")])),
                make_para("Outro paragraph."),
            ]),
        );

        // One long-lived ydoc that survives across all 100 edits — the
        // production shape, where the WS client's CollabClient holds the
        // ydoc for the whole session. Drains the observability buffer at
        // start so the slow-path count is a clean delta over the loop.
        let _ = crate::observability::drain();
        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &initial);

        const N: usize = 100;
        let mut current = initial;
        for i in 0..N {
            let baseline_sv = ydoc.transact().state_vector();
            let edited = insert_one_char_in_first_text(&current);
            sync_model_to_ydoc(&ydoc, &edited);
            let update = ydoc.transact().encode_state_as_update_v1(&baseline_sv);
            assert!(
                update.len() < SINGLE_CHAR_EDIT_BUDGET,
                "iter {i}: single-char edit produced {} B (budget {})",
                update.len(),
                SINGLE_CHAR_EDIT_BUDGET
            );
            current = edited;
        }

        // Drain consumes the counter values from this thread. If the slow
        // path fired at any point during the loop the counter would be
        // non-zero — and that would be a strong signal the find_match
        // ordering invariants regressed under sustained editing.
        assert_eq!(
            slow_path_counter_value(),
            0,
            "sustained {N}-keystroke session must not take the slow path"
        );
    }

    // ─── Legacy persisted shape regression (gap #2 of the plan) ──────────
    //
    // Documents written before commit d92dac4 have yrs containers
    // (BulletList, OrderedList, TaskList, TableRow) with NO `blockId`
    // attribute on the persisted side, because the pre-fix
    // `is_commentable`-gated rule excluded those types. After d92dac4 the
    // editor model always carries a blockId on those nodes, but the yrs
    // side of an existing doc still doesn't — so `find_match` misses, the
    // container is treated as a fresh Insert, and every keystroke
    // re-writes the entire subtree (the same ~60 KB-per-edit pathology
    // d92dac4 fixed, just for already-persisted docs).
    //
    // This test pins that current (broken) behavior. The fixture below
    // — a yrs Doc built directly with `XmlFragment::insert` and
    // *intentionally* missing the blockId attribute on the BulletList —
    // is load-bearing for issue #92: that ticket's migration step will
    // need exactly this shape as input. When the migration lands, the
    // mechanical change to this test is to invert the assertion direction
    // (`>` → `<`); the fixture itself is the long-lived contract.

    #[test]
    fn legacy_blockid_less_doc_heals_via_container_tag_fallback() {
        // #92 Option B: this test was originally
        // `legacy_blockid_less_doc_currently_degenerates` and
        // asserted the broken behavior (update > budget) so the
        // fix PR would be a focused diff. Option B made
        // find_match's container-tag fallback (strategy 3) cover
        // the case, so a single-char edit on a legacy doc now
        // produces a small update — the model's freshly-generated
        // nanoid for the legacy BulletList matches a tag-and-no-
        // blockId yrs Element. The Reuse path runs; sync_attrs
        // writes the nanoid back to yrs, healing the doc on disk.
        let _ = crate::observability::drain();

        // Build a legacy-shape yrs Doc directly: BulletList container
        // with NO blockId, populated with enough ListItems that a
        // "rewrite everything" update is unambiguously larger than the
        // single-char budget. The inner ListItem and Paragraph nodes DO
        // have blockIds (those types were already commentable
        // pre-d92dac4), matching the real shape on disk.
        const N_ITEMS: usize = 30;
        let ydoc = Doc::new();
        {
            let mut txn = ydoc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");

            let bullet_list = fragment.insert(
                &mut txn, 0, XmlElementPrelim::empty("bullet_list")
            );
            // Intentionally NOT setting "blockId" on bullet_list — this
            // is the entire point of the test. The non-commentable
            // container types (BulletList / OrderedList / TaskList /
            // TableRow) all fell through this gap pre-d92dac4.

            for i in 0..N_ITEMS {
                let item = bullet_list.insert(
                    &mut txn,
                    i as u32,
                    XmlElementPrelim::empty("list_item"),
                );
                item.insert_attribute(
                    &mut txn,
                    "blockId",
                    &format!("legacy-li-{i}"),
                );

                let para = item.insert(
                    &mut txn, 0, XmlElementPrelim::empty("paragraph")
                );
                para.insert_attribute(
                    &mut txn,
                    "blockId",
                    &format!("legacy-p-{i}"),
                );
                para.insert(
                    &mut txn,
                    0,
                    XmlTextPrelim::new(&format!(
                        "Legacy bullet item {i} carrying enough text to make a \
                         rewrite measurable against the single-char budget."
                    )),
                );
            }
        }

        // Capture the state vector now — the size of the update emitted
        // by the next sync_model_to_ydoc is what we're testing.
        let baseline_sv = ydoc.transact().state_vector();

        // Read the doc as a model. The model side, post-d92dac4, will
        // auto-generate a blockId for the BulletList (because
        // `needs_block_id` is true for it). Under Option B, the
        // container-tag fallback in find_match (strategy 3)
        // matches the fresh nanoid against the tag-and-no-blockId
        // yrs Element. The Reuse path produces a small incremental
        // update; sync_attrs writes the nanoid into yrs so the
        // next session reads a fully-migrated block.
        let model = read_doc_from_ydoc(&ydoc).expect("read legacy doc");
        let edited = insert_one_char_in_first_text(&model);
        sync_model_to_ydoc(&ydoc, &edited);

        let update = ydoc.transact().encode_state_as_update_v1(&baseline_sv);

        assert!(
            update.len() < SINGLE_CHAR_EDIT_BUDGET,
            "container-tag fallback should keep the legacy edit under the \
             single-char budget; got {} B (budget {}). Did strategy 3 of \
             find_match regress?",
            update.len(),
            SINGLE_CHAR_EDIT_BUDGET,
        );

        // The doc should also have healed: on a second read, the
        // BulletList now carries the same blockId we synthesized at
        // first read. Subsequent sessions take the strategy-1
        // (real-blockId) fast path with no fallback involvement.
        let healed_model = read_doc_from_ydoc(&ydoc).expect("re-read healed doc");
        let healed_bullet_id = first_bullet_list_block_id(&healed_model)
            .expect("BulletList should still exist in healed doc");
        assert!(
            !healed_bullet_id.is_empty(),
            "after first edit, the BulletList must carry a persisted blockId"
        );

        let baseline_sv2 = ydoc.transact().state_vector();
        let edited2 = insert_one_char_in_first_text(&healed_model);
        sync_model_to_ydoc(&ydoc, &edited2);
        let update2 = ydoc.transact().encode_state_as_update_v1(&baseline_sv2);
        assert!(
            update2.len() < SINGLE_CHAR_EDIT_BUDGET,
            "second edit on the now-migrated doc should also be small; \
             got {} B (budget {})",
            update2.len(),
            SINGLE_CHAR_EDIT_BUDGET,
        );
    }

    /// #92 Option B regression guard: in a post-d92dac4 (clean)
    /// doc, inserting a NEW block must take the Insert path, not
    /// false-match against an existing block via the container-tag
    /// fallback. The fallback's `info.block_id.is_none()` predicate
    /// is what guarantees this — every yrs block in a clean doc has
    /// a blockId, so the fallback finds nothing to match against
    /// and the new block correctly inserts fresh.
    #[test]
    fn container_tag_fallback_does_not_false_match_in_clean_doc() {
        let _ = crate::observability::drain();

        // Two bullet lists in a clean post-d92dac4 doc — both
        // carry real blockIds via the normal constructor.
        let li_a = list_item_with_id("li-a", "apple");
        let li_b = list_item_with_id("li-b", "banana");
        let initial = doc_with_blocks(vec![
            bullet_list_with_items("ul-1", vec![li_a.clone()]),
            bullet_list_with_items("ul-2", vec![li_b.clone()]),
        ]);

        let ydoc = Doc::new();
        sync_model_to_ydoc(&ydoc, &initial);

        // Now insert a THIRD bullet list at the front. The model
        // has three lists; yrs has two. find_match on the new list
        // must NOT match either existing yrs list via the
        // container-tag fallback — both yrs lists have blockIds,
        // so `info.block_id.is_none()` is false for them, and the
        // new list falls through to Insert.
        let li_c = list_item_with_id("li-c", "cherry");
        let inserted = doc_with_blocks(vec![
            bullet_list_with_items("ul-3", vec![li_c]),
            bullet_list_with_items("ul-1", vec![li_a]),
            bullet_list_with_items("ul-2", vec![li_b]),
        ]);
        sync_model_to_ydoc(&ydoc, &inserted);

        // Confirm the resulting yrs state has three distinct bullet
        // lists with the three distinct blockIds we sent — i.e.,
        // ul-1, ul-2, ul-3 are all present, no two collapsed.
        let healed = read_doc_from_ydoc(&ydoc).expect("read");
        let all_bullet_ids: Vec<String> = collect_bullet_list_block_ids(&healed);
        assert_eq!(
            all_bullet_ids.len(),
            3,
            "expected exactly 3 distinct bullet lists, got {all_bullet_ids:?}"
        );
        assert!(all_bullet_ids.contains(&"ul-1".to_string()));
        assert!(all_bullet_ids.contains(&"ul-2".to_string()));
        assert!(all_bullet_ids.contains(&"ul-3".to_string()));
    }

    fn collect_bullet_list_block_ids(node: &Node) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(node: &Node, out: &mut Vec<String>) {
            if let Node::Element { node_type, attrs, content, .. } = node {
                if *node_type == NodeType::BulletList {
                    if let Some(bid) = attrs.get("blockId") {
                        out.push(bid.clone());
                    }
                }
                for child in &content.children {
                    walk(child, out);
                }
            }
        }
        walk(node, &mut out);
        out
    }

    /// Regression: adding a KanbanCard to an existing Kanban block
    /// used to be a silent no-op at the yrs layer because
    /// sync_block_content's `is_container` list omitted Kanban /
    /// Calendar / KanbanColumn. The else branch's
    /// `text_and_marks_match` short-circuit returned true (cards
    /// carry state in attrs, not text nodes), so the new card
    /// was never written to yrs. Symptom on doc
    /// `5xBzUM-KS8u_bi8XPcPzN`: cards visible in the browser
    /// vanished on refresh.
    #[test]
    fn kanban_card_add_persists_through_yrs_diff_sync() {
        use super::super::model::Fragment;
        // Build a doc with an empty Kanban → sync once → add a card
        // → sync again → decode and confirm the card is present at
        // the correct level (inside KanbanColumn, not floating at
        // Kanban level).
        fn make_doc(cards: &[&str]) -> Node {
            let mut kanban_attrs = HashMap::new();
            kanban_attrs.insert("blockId".to_string(), "K1".to_string());
            let mut col_attrs = HashMap::new();
            col_attrs.insert("blockId".to_string(), "C1".to_string());
            col_attrs.insert("title".to_string(), "To Do".to_string());
            let card_nodes: Vec<Node> = cards.iter().map(|title| {
                let mut card_attrs = HashMap::new();
                card_attrs.insert("blockId".to_string(), format!("card-{title}"));
                card_attrs.insert("title".to_string(), title.to_string());
                Node::element_with_attrs(NodeType::KanbanCard, card_attrs, Fragment::empty())
            }).collect();
            let column = Node::element_with_attrs(
                NodeType::KanbanColumn,
                col_attrs,
                Fragment::from(card_nodes),
            );
            let kanban = Node::element_with_attrs(
                NodeType::Kanban,
                kanban_attrs,
                Fragment::from(vec![column]),
            );
            Node::element_with_content(NodeType::Doc, Fragment::from(vec![kanban]))
        }

        let ydoc = Doc::new();
        // Sync #1: empty column.
        let doc0 = make_doc(&[]);
        let synced0 = sync_model_to_ydoc_diffed(&ydoc, &doc0, None);
        // Sync #2: same doc + one card (Reuse path fires on the
        // Kanban element).
        let doc1 = make_doc(&["Test"]);
        let _ = sync_model_to_ydoc_diffed(&ydoc, &doc1, Some(&synced0));

        // Read back and assert structure.
        let read_back = read_doc_from_ydoc(&ydoc).unwrap();
        let kanban_child = read_back.child(0).unwrap();
        let Node::Element { content: kanban_content, node_type: kanban_type, .. } = kanban_child else {
            panic!("expected kanban element");
        };
        assert_eq!(*kanban_type, NodeType::Kanban);
        assert_eq!(kanban_content.children.len(), 1,
            "kanban should have exactly one direct child (the column) — got {}",
            kanban_content.children.len());
        let col = &kanban_content.children[0];
        let Node::Element { content: col_content, node_type: col_type, .. } = col else {
            panic!("expected column element");
        };
        assert_eq!(*col_type, NodeType::KanbanColumn);
        assert_eq!(col_content.children.len(), 1, "column must have the card");
        let card = &col_content.children[0];
        let Node::Element { attrs: card_attrs, node_type: card_type, .. } = card else {
            panic!("expected card element");
        };
        assert_eq!(*card_type, NodeType::KanbanCard);
        assert_eq!(card_attrs.get("title").map(|s| s.as_str()), Some("Test"));
    }

    /// Find the blockId of the first BulletList anywhere in the doc.
    /// Used by the #92 healing test to confirm the migration wrote
    /// the synthesized id back to yrs.
    fn first_bullet_list_block_id(node: &Node) -> Option<String> {
        match node {
            Node::Element { node_type, attrs, content, .. } => {
                if *node_type == NodeType::BulletList {
                    return attrs.get("blockId").cloned();
                }
                content.children.iter().find_map(first_bullet_list_block_id)
            }
            Node::Text { .. } => None,
        }
    }

    /// Collect all text content of a doc, in order. Assertion helper for
    /// the #92 merge tests.
    fn collect_text(node: &Node, out: &mut String) {
        match node {
            Node::Text { text, .. } => out.push_str(text),
            Node::Element { content, .. } => {
                for child in &content.children {
                    collect_text(child, out);
                }
            }
        }
    }

    fn doc_text(node: &Node) -> String {
        let mut s = String::new();
        collect_text(node, &mut s);
        s
    }

    /// #92 (editor drops first keystrokes): keystrokes that exist only in
    /// the editor model — typed inside the send-debounce window, not yet
    /// folded into the ydoc — must survive a remote update that rebuilds
    /// the view from the ydoc. The recv path folds the current editor
    /// model into the ydoc BEFORE reading it back, turning the wholesale
    /// swap into a CRDT merge. This test replays the exact race:
    /// REST-initialized client ydoc, unsynced local keystrokes, initial
    /// SyncStep2 carrying a peer edit.
    #[test]
    fn merge_preserves_unsynced_keystrokes_across_remote_swap() {
        // REST-loaded initial content: one paragraph "Hello".
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let initial_bytes = doc_to_ydoc_bytes(&initial);

        // Client ydoc — as CollabClient::new builds it — and the baseline
        // the editor has actually seen (the initial content).
        let client_ydoc = Doc::new();
        {
            let mut txn = client_ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap())
                .unwrap();
        }
        let baseline = read_doc_from_ydoc(&client_ydoc).unwrap();

        // User types "al" at the head of the paragraph. This lives only in
        // the editor model — the debounced send has NOT folded it into the
        // ydoc yet.
        let typed = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("alHello")]),
            )]),
        );

        // Meanwhile the server (same lineage) gained a peer edit: a second
        // paragraph. Its full state arrives as the initial SyncStep2 and is
        // applied into the client ydoc, exactly like apply_remote_update.
        let server_ydoc = Doc::new();
        {
            let mut txn = server_ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap())
                .unwrap();
        }
        let server_model = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b1".into())].into(),
                    Fragment::from(vec![Node::text("Hello")]),
                ),
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b2".into())].into(),
                    Fragment::from(vec![Node::text("Peer")]),
                ),
            ]),
        );
        sync_model_to_ydoc_diffed(&server_ydoc, &server_model, Some(&initial));
        let server_state = server_ydoc
            .transact()
            .encode_state_as_update_v1(&yrs::StateVector::default());

        // The fix: FOLD the editor model into the ydoc first (diffed
        // against its own baseline — the diff is exactly the local
        // keystrokes), THEN apply the remote update and read the yrs
        // merge. This is the order the ws_client recv path uses.
        let normalized = sync_model_to_ydoc_diffed(&client_ydoc, &typed, Some(&baseline));
        assert!(
            doc_text(&normalized).contains("alHello"),
            "fold baseline sanity"
        );
        {
            let mut txn = client_ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&server_state).unwrap())
                .unwrap();
        }
        let merged = read_doc_from_ydoc(&client_ydoc).unwrap();
        let text = doc_text(&merged);
        assert!(
            text.contains("alHello"),
            "typed keystrokes must survive the swap, got {text:?}"
        );
        assert!(
            text.contains("Peer"),
            "the peer's remote edit must also survive the merge, got {text:?}"
        );
    }

    /// Companion to the test above. Historically the fold HAD to happen
    /// before the remote update was applied: in the reverse order,
    /// `remove_unmatched` treated the peer's paragraph (which the stale
    /// editor model had never seen) as a local deletion and removed it —
    /// this test originally pinned that hazard. `remove_unmatched` is now
    /// baseline-scoped (it only deletes an unmatched live block whose
    /// blockId the caller's baseline knew), so BOTH orderings preserve
    /// the peer's content and the recv path's fold/apply ordering is no
    /// longer load-bearing for safety (it still matters for merging the
    /// local edits themselves — see the test above).
    #[test]
    fn fold_after_remote_apply_preserves_peer_content() {
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let initial_bytes = doc_to_ydoc_bytes(&initial);

        let client_ydoc = Doc::new();
        {
            let mut txn = client_ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap())
                .unwrap();
        }
        let baseline = read_doc_from_ydoc(&client_ydoc).unwrap();

        let typed = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), "b1".into())].into(),
                Fragment::from(vec![Node::text("alHello")]),
            )]),
        );

        let server_ydoc = Doc::new();
        {
            let mut txn = server_ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap())
                .unwrap();
        }
        let server_model = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b1".into())].into(),
                    Fragment::from(vec![Node::text("Hello")]),
                ),
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    [("blockId".into(), "b2".into())].into(),
                    Fragment::from(vec![Node::text("Peer")]),
                ),
            ]),
        );
        sync_model_to_ydoc_diffed(&server_ydoc, &server_model, Some(&initial));
        let server_state = server_ydoc
            .transact()
            .encode_state_as_update_v1(&yrs::StateVector::default());

        // Wrong order: remote applied first, stale model folded after.
        {
            let mut txn = client_ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&server_state).unwrap())
                .unwrap();
        }
        sync_model_to_ydoc_diffed(&client_ydoc, &typed, Some(&baseline));
        let text = doc_text(&read_doc_from_ydoc(&client_ydoc).unwrap());
        assert!(
            text.contains("Peer"),
            "baseline-scoped remove_unmatched must keep the peer's block \
             even when a stale model is folded AFTER the remote apply \
             (the block's id is unknown to the fold's baseline): {text:?}"
        );
        assert!(
            text.contains("alHello"),
            "the local keystrokes must also be in the merge: {text:?}"
        );
    }

    /// #92 (swap-window half): keystrokes typed between a remote apply and
    /// the debounced swap-read exist only in the editor model. The recv
    /// timer folds them into the ydoc before swapping — but only when the
    /// model and the post-merge ydoc agree on top-level structure
    /// (`same_top_level_block_ids`), which is what makes the fold safe:
    /// per-block reconciliation can't delete a block. This exercises both
    /// branches of that guard.
    #[test]
    fn swap_window_fold_is_guarded_by_structure() {
        let para = |id: &str, text: &str| {
            Node::element_with_attrs(
                NodeType::Paragraph,
                [("blockId".into(), id.into())].into(),
                Fragment::from(vec![Node::text(text)]),
            )
        };
        let initial = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![para("b1", "Hello")]),
        );
        let initial_bytes = doc_to_ydoc_bytes(&initial);

        // ── Guard-positive: remote made no structural change. ──
        let ydoc = Doc::new();
        {
            let mut txn = ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap()).unwrap();
        }
        let baseline = read_doc_from_ydoc(&ydoc).unwrap();
        // Remote frame applied (here: an idempotent re-send of the same
        // state, like the second handshake SyncStep2)…
        {
            let mut txn = ydoc.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap()).unwrap();
        }
        // …while the user typed "al" (model-only).
        let typed = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![para("b1", "alHello")]),
        );
        let swap_state = read_doc_from_ydoc(&ydoc).unwrap();
        assert!(
            same_top_level_block_ids(&typed, &swap_state),
            "no structural divergence → fold is safe"
        );
        sync_model_to_ydoc_diffed(&ydoc, &typed, Some(&baseline));
        let merged = read_doc_from_ydoc(&ydoc).unwrap();
        assert!(
            doc_text(&merged).contains("alHello"),
            "swap-window keystrokes must be folded before the swap, got {:?}",
            doc_text(&merged)
        );

        // ── Guard-negative: remote added a block; fold must be skipped. ──
        let ydoc2 = Doc::new();
        {
            let mut txn = ydoc2.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap()).unwrap();
        }
        let server = Doc::new();
        {
            let mut txn = server.transact_mut();
            txn.apply_update(Update::decode_v1(&initial_bytes).unwrap()).unwrap();
        }
        let server_model = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![para("b1", "Hello"), para("b2", "Peer")]),
        );
        sync_model_to_ydoc_diffed(&server, &server_model, Some(&initial));
        let server_state = server
            .transact()
            .encode_state_as_update_v1(&yrs::StateVector::default());
        {
            let mut txn = ydoc2.transact_mut();
            txn.apply_update(Update::decode_v1(&server_state).unwrap()).unwrap();
        }
        let swap_state2 = read_doc_from_ydoc(&ydoc2).unwrap();
        assert!(
            !same_top_level_block_ids(&typed, &swap_state2),
            "remote structural change → the guard must refuse the fold \
             (folding would delete the peer's block, per the tripwire test)"
        );
    }
}
