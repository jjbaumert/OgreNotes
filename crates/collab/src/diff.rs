// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Document diffing: compare two yrs document versions at the block level.
//!
//! Walks each top-level XmlFragment child as a `RichBlock` carrying its
//! `node_type`, `attrs`, the `blockId` attribute (every commentable block
//! has one — see `frontend/src/editor/model.rs`), the inline content as
//! `(text, marks)` runs, and the recursive children. Block-equality is
//! defined ignoring `block_id` so that an unchanged block whose id was
//! reassigned still classifies as unchanged.
//!
//! Pairing is index-by-index. Same id (or both unidentified) with equal
//! content → no entry. Same id, different content → `Modified`. Past one
//! side → unilateral `Added` or `Removed`. After classification, runs of
//! adjacent same-side `Added` (or same-side `Removed`) entries collapse
//! into one `DiffEntry` whose `blocks` is the concatenation; `Modified`
//! never groups.

use std::collections::BTreeMap;

use serde::Serialize;
use yrs::{Any, Doc, Out, ReadTxn, Text, Transact};
use yrs::types::Attrs;
use yrs::types::text::YChange;
use yrs::types::xml::{Xml, XmlElementRef, XmlFragment, XmlOut, XmlTextRef};

/// Whether an entry describes added, removed, or modified blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffKind {
    Added,
    Removed,
    Modified,
}

/// Inline mark, mirrors `frontend/src/editor/model.rs::MarkType` — the full
/// attribute-mark set (10). This is deliberately a superset of the 8
/// schema-validated marks in `schema.rs::MarkType`: `TextColor` and
/// `Highlight` are loose CRDT-attribute marks outside the validated schema,
/// but they appear in real documents, so version-history attribution must
/// recognize them. See `schema.rs::ALL_MARK_TYPES` for the schema side.
///
/// Marks with attributes (`Link` href, `TextColor` color, `Highlight`
/// color) carry their attribute inline so the frontend can render them
/// without a second lookup.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Link { href: String },
    TextColor { color: String },
    Highlight { color: String },
    Subscript,
    Superscript,
}

/// A run of inline text with the same set of marks.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InlineRun {
    pub text: String,
    pub marks: Vec<Mark>,
}

/// A block of rich content, suitable for re-rendering in the diff modal
/// the way the editor would render it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RichBlock {
    /// Yrs tag name (e.g. `paragraph`, `heading`, `bullet_list`).
    pub node_type: String,
    /// Block-level attributes, excluding `blockId` (which is split out).
    /// `BTreeMap` so the JSON ordering is stable across builds.
    pub attrs: BTreeMap<String, String>,
    /// CRDT block id, if the block has one. `None` for legacy blocks
    /// that never had an id assigned.
    pub block_id: Option<String>,
    /// Inline content as `(text, marks)` runs. Empty for container-only
    /// blocks (lists, tables) and for leaf blocks like images.
    pub inline: Vec<InlineRun>,
    /// Nested blocks: list items inside a list, paragraphs inside a
    /// blockquote, rows inside a table, etc.
    pub children: Vec<RichBlock>,
}

/// A single difference between two document versions.
///
/// `block_id` is the first block of the run for `Added`/`Removed`, or the
/// shared id for `Modified`. `block_index` is the ordinal position of
/// the first block in v2 for `Added`/`Modified` and in v1 for `Removed`.
/// `node_type` is the first block's `node_type` (handy for the entry's
/// header label without forcing the frontend to peek into `blocks[0]`).
///
/// `user_id` and `timestamp` are attribution fields populated by the API
/// layer after the structural diff is computed; per-snapshot, not
/// per-block. They stay `None` when the diff engine runs standalone.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffEntry {
    pub kind: DiffKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    pub block_index: usize,
    pub node_type: String,
    /// One or more blocks for `Added`/`Removed`; for `Modified` exactly
    /// two entries: `blocks[0]` is the old version, `blocks[1]` the new.
    pub blocks: Vec<RichBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

// ─── Public API ──────────────────────────────────────────────────────

/// Compute differences between two yrs documents.
/// Attribution fields are left `None`; callers wanting per-version
/// attribution should use `diff_documents_attributed`.
pub fn diff_documents(old_doc: &Doc, new_doc: &Doc) -> Vec<DiffEntry> {
    let old_blocks = extract_blocks(old_doc);
    let new_blocks = extract_blocks(new_doc);

    let raw = pair_blocks(&old_blocks, &new_blocks);
    group_runs(raw)
}

/// Compute differences and stamp every entry with the same attribution
/// (per-snapshot, not per-block — see module docs).
pub fn diff_documents_attributed(
    old_doc: &Doc,
    new_doc: &Doc,
    user_id: Option<String>,
    timestamp: Option<i64>,
) -> Vec<DiffEntry> {
    let mut diffs = diff_documents(old_doc, new_doc);
    for entry in &mut diffs {
        entry.user_id = user_id.clone();
        entry.timestamp = timestamp;
    }
    diffs
}

// ─── Walker: yrs → RichBlock ─────────────────────────────────────────

fn extract_blocks(doc: &Doc) -> Vec<RichBlock> {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let len = fragment.len(&txn);
    for i in 0..len {
        if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
            out.push(read_block(&txn, &el));
        }
    }
    out
}

fn read_block<T: ReadTxn>(txn: &T, el: &XmlElementRef) -> RichBlock {
    let node_type = el.tag().to_string();
    let mut attrs: BTreeMap<String, String> = BTreeMap::new();
    let mut block_id: Option<String> = None;
    for (key, value) in el.attributes(txn) {
        if key == "blockId" {
            block_id = Some(value.to_string());
        } else {
            attrs.insert(key.to_string(), value.to_string());
        }
    }

    let mut inline = Vec::new();
    let mut children = Vec::new();
    let len = el.len(txn);
    for i in 0..len {
        match el.get(txn, i) {
            Some(XmlOut::Element(child_el)) => {
                children.push(read_block(txn, &child_el));
            }
            Some(XmlOut::Text(text)) => {
                inline.extend(read_text_runs(txn, &text));
            }
            _ => {}
        }
    }

    RichBlock {
        node_type,
        attrs,
        block_id,
        inline,
        children,
    }
}

fn read_text_runs<T: ReadTxn>(txn: &T, text: &XmlTextRef) -> Vec<InlineRun> {
    let mut out = Vec::new();
    for diff in text.diff(txn, YChange::identity) {
        if let Out::Any(Any::String(s)) = &diff.insert {
            let s: &str = s.as_ref();
            if s.is_empty() {
                continue;
            }
            let marks = diff
                .attributes
                .as_ref()
                .map(|a| attrs_to_marks(a))
                .unwrap_or_default();
            out.push(InlineRun {
                text: s.to_string(),
                marks,
            });
        }
    }
    out
}

/// Plain text of the block with the given `blockId` — the block's own
/// inline runs followed by its children, depth-first — truncated to
/// `max_chars` characters (char-boundary safe). `None` if no block in
/// the document carries that id. Public: consumed by the mentions
/// resolve endpoint (`crates/api`) for anchor-mention snippets.
pub fn block_plain_text(doc: &Doc, block_id: &str, max_chars: usize) -> Option<String> {
    fn find<'a>(blocks: &'a [RichBlock], id: &str) -> Option<&'a RichBlock> {
        for b in blocks {
            if b.block_id.as_deref() == Some(id) {
                return Some(b);
            }
            if let Some(hit) = find(&b.children, id) {
                return Some(hit);
            }
        }
        None
    }
    fn collect(b: &RichBlock, out: &mut String) {
        for run in &b.inline {
            out.push_str(&run.text);
        }
        for child in &b.children {
            if !out.is_empty() {
                out.push(' ');
            }
            collect(child, out);
        }
    }
    let blocks = extract_blocks(doc);
    let target = find(&blocks, block_id)?;
    let mut text = String::new();
    collect(target, &mut text);
    Some(text.chars().take(max_chars).collect())
}

/// Translate yrs formatting attributes to canonical `Mark` enum values.
/// Mirrors `frontend/src/editor/yrs_bridge.rs::attrs_to_marks`. Marks
/// with payload (link / textColor / highlight) carry a JSON map in the
/// yrs value; pull `href` / `color` out of it.
fn attrs_to_marks(attrs: &Attrs) -> Vec<Mark> {
    let mut marks = Vec::new();
    for (key, value) in attrs {
        let k: &str = key.as_ref();
        match k {
            "bold" if !is_null(value) => marks.push(Mark::Bold),
            "italic" if !is_null(value) => marks.push(Mark::Italic),
            "underline" if !is_null(value) => marks.push(Mark::Underline),
            "strike" if !is_null(value) => marks.push(Mark::Strike),
            "code" if !is_null(value) => marks.push(Mark::Code),
            "subscript" if !is_null(value) => marks.push(Mark::Subscript),
            "superscript" if !is_null(value) => marks.push(Mark::Superscript),
            "link" => {
                if let Any::String(s) = value {
                    let parsed: serde_json::Value =
                        serde_json::from_str(s.as_ref()).unwrap_or(serde_json::Value::Null);
                    let href = parsed
                        .get("href")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    marks.push(Mark::Link { href });
                }
            }
            "textColor" => {
                if let Any::String(s) = value {
                    let parsed: serde_json::Value =
                        serde_json::from_str(s.as_ref()).unwrap_or(serde_json::Value::Null);
                    let color = parsed
                        .get("color")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    marks.push(Mark::TextColor { color });
                }
            }
            "highlight" => {
                if let Any::String(s) = value {
                    let parsed: serde_json::Value =
                        serde_json::from_str(s.as_ref()).unwrap_or(serde_json::Value::Null);
                    let color = parsed
                        .get("color")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    marks.push(Mark::Highlight { color });
                }
            }
            _ => {}
        }
    }
    marks
}

fn is_null(value: &Any) -> bool {
    matches!(value, Any::Null)
}

// ─── Pairing & classification ────────────────────────────────────────

/// Compare two blocks for content equality, ignoring `block_id` at every
/// level of recursion. A block whose id was reassigned but whose body
/// didn't change still counts as unchanged.
fn same_content(a: &RichBlock, b: &RichBlock) -> bool {
    if a.node_type != b.node_type {
        return false;
    }
    if a.attrs != b.attrs {
        return false;
    }
    if a.inline != b.inline {
        return false;
    }
    if a.children.len() != b.children.len() {
        return false;
    }
    a.children
        .iter()
        .zip(b.children.iter())
        .all(|(c1, c2)| same_content(c1, c2))
}

fn pair_blocks(old: &[RichBlock], new: &[RichBlock]) -> Vec<DiffEntry> {
    let mut out = Vec::new();
    let max_len = old.len().max(new.len());
    let mut i = 0;
    while i < max_len {
        match (old.get(i), new.get(i)) {
            (Some(a), Some(b)) => {
                let same_id = match (&a.block_id, &b.block_id) {
                    (Some(x), Some(y)) => x == y,
                    (None, None) => true,
                    _ => false,
                };
                if same_id {
                    if !same_content(a, b) {
                        out.push(DiffEntry {
                            kind: DiffKind::Modified,
                            block_id: b.block_id.clone(),
                            block_index: i,
                            node_type: b.node_type.clone(),
                            blocks: vec![a.clone(), b.clone()],
                            user_id: None,
                            timestamp: None,
                        });
                    }
                    i += 1;
                    continue;
                }
                // Different ids at the same index. Walk the longest
                // contiguous run of mismatched-id positions starting
                // here and emit one bulk Removed + one bulk Added for
                // the whole run. Without this, every mismatched
                // position would emit its own Removed-Added pair, and
                // group_runs's adjacent-same-kind merge can't cross
                // the interleaved boundary, so a single bulk
                // replacement of N blocks would render as 2N cards.
                let start = i;
                let mut j = i;
                while j < max_len {
                    let mismatched = match (old.get(j), new.get(j)) {
                        (Some(x), Some(y)) => match (&x.block_id, &y.block_id) {
                            (Some(xi), Some(yi)) => xi != yi,
                            (None, None) => false,
                            _ => true,
                        },
                        _ => false,
                    };
                    if !mismatched {
                        break;
                    }
                    j += 1;
                }
                let removed_blocks: Vec<RichBlock> = old[start..j].to_vec();
                let added_blocks: Vec<RichBlock> = new[start..j].to_vec();
                let removed_first = removed_blocks[0].clone();
                let added_first = added_blocks[0].clone();
                out.push(DiffEntry {
                    kind: DiffKind::Removed,
                    block_id: removed_first.block_id.clone(),
                    block_index: start,
                    node_type: removed_first.node_type.clone(),
                    blocks: removed_blocks,
                    user_id: None,
                    timestamp: None,
                });
                out.push(DiffEntry {
                    kind: DiffKind::Added,
                    block_id: added_first.block_id.clone(),
                    block_index: start,
                    node_type: added_first.node_type.clone(),
                    blocks: added_blocks,
                    user_id: None,
                    timestamp: None,
                });
                i = j;
            }
            (Some(a), None) => {
                out.push(DiffEntry {
                    kind: DiffKind::Removed,
                    block_id: a.block_id.clone(),
                    block_index: i,
                    node_type: a.node_type.clone(),
                    blocks: vec![a.clone()],
                    user_id: None,
                    timestamp: None,
                });
                i += 1;
            }
            (None, Some(b)) => {
                out.push(DiffEntry {
                    kind: DiffKind::Added,
                    block_id: b.block_id.clone(),
                    block_index: i,
                    node_type: b.node_type.clone(),
                    blocks: vec![b.clone()],
                    user_id: None,
                    timestamp: None,
                });
                i += 1;
            }
            (None, None) => break,
        }
    }
    out
}

/// Merge consecutive `Added` (or consecutive `Removed`) entries whose
/// `block_index` is adjacent. `Modified` never groups. The first entry
/// of a run keeps its `block_id` and `block_index`; subsequent blocks
/// are appended to its `blocks` field.
fn group_runs(entries: Vec<DiffEntry>) -> Vec<DiffEntry> {
    let mut out: Vec<DiffEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(last) = out.last_mut() {
            let mergeable = matches!(entry.kind, DiffKind::Added | DiffKind::Removed)
                && last.kind == entry.kind
                && last.block_index + last.blocks.len() == entry.block_index;
            if mergeable {
                last.blocks.extend(entry.blocks);
                continue;
            }
        }
        out.push(entry);
    }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{
        types::Attrs,
        types::xml::{Xml, XmlElementPrelim, XmlFragment, XmlTextPrelim},
        Text, Transact, WriteTxn,
    };

    /// Build a doc whose top-level children are `(tag, text)` paragraphs
    /// of unmarked text. Each block is given a stable `blockId` so the
    /// pairing logic treats positional matches as same-block.
    fn doc_with_blocks(blocks: &[(&str, &str, &str)]) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            for &(tag, text, id) in blocks {
                let pos = frag.len(&txn);
                let el = frag.insert(&mut txn, pos, XmlElementPrelim::empty(tag));
                el.insert_attribute(&mut txn, "blockId", id);
                el.insert(&mut txn, 0, XmlTextPrelim::new(text));
            }
        }
        doc
    }

    /// Build a single-block doc whose text is split into runs with marks
    /// applied via `format` after the bare text is inserted. Mirrors how
    /// the editor produces marked yrs text in `yrs_bridge.rs`.
    fn doc_with_marked_text(tag: &str, runs: &[(&str, &[(&str, Any)])]) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            let el = frag.insert(&mut txn, 0, XmlElementPrelim::empty(tag));
            el.insert_attribute(&mut txn, "blockId", "id-0");
            // Concatenate all run text first.
            let combined: String = runs.iter().map(|(t, _)| *t).collect();
            let text = el.insert(&mut txn, 0, XmlTextPrelim::new(combined.as_str()));
            // Then apply formatting per run by computing offsets.
            let mut offset: u32 = 0;
            for (run_text, run_marks) in runs {
                let len = run_text.chars().count() as u32;
                if !run_marks.is_empty() {
                    let mut attrs = Attrs::new();
                    for (k, v) in run_marks.iter() {
                        attrs.insert(std::sync::Arc::from(*k), v.clone());
                    }
                    text.format(&mut txn, offset, len, attrs);
                }
                offset += len;
            }
        }
        doc
    }

    #[test]
    fn diff_identical_docs() {
        let doc = doc_with_blocks(&[("paragraph", "Hello", "a")]);
        assert!(diff_documents(&doc, &doc).is_empty());
    }

    #[test]
    fn diff_added_block_is_added_kind() {
        let old = doc_with_blocks(&[("paragraph", "First", "a")]);
        let new = doc_with_blocks(&[("paragraph", "First", "a"), ("paragraph", "Second", "b")]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].kind, DiffKind::Added);
        assert_eq!(diffs[0].block_id.as_deref(), Some("b"));
        assert_eq!(diffs[0].blocks.len(), 1);
        assert_eq!(diffs[0].blocks[0].inline[0].text, "Second");
    }

    #[test]
    fn diff_removed_block_is_removed_kind() {
        let old = doc_with_blocks(&[("paragraph", "First", "a"), ("paragraph", "Second", "b")]);
        let new = doc_with_blocks(&[("paragraph", "First", "a")]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].kind, DiffKind::Removed);
        assert_eq!(diffs[0].block_id.as_deref(), Some("b"));
    }

    #[test]
    fn diff_changed_text_is_modified_with_old_and_new() {
        let old = doc_with_blocks(&[("paragraph", "Hello", "a")]);
        let new = doc_with_blocks(&[("paragraph", "World", "a")]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].kind, DiffKind::Modified);
        assert_eq!(diffs[0].blocks.len(), 2);
        assert_eq!(diffs[0].blocks[0].inline[0].text, "Hello");
        assert_eq!(diffs[0].blocks[1].inline[0].text, "World");
    }

    #[test]
    fn diff_block_id_extracted_from_yrs_attributes() {
        let old = doc_with_blocks(&[("heading", "Old", "h-1")]);
        let new = doc_with_blocks(&[("heading", "New", "h-1")]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].block_id.as_deref(), Some("h-1"));
        assert!(diffs[0].blocks[0].block_id.as_deref() == Some("h-1"));
    }

    #[test]
    fn diff_two_consecutive_adds_collapse_to_one_entry() {
        let old = doc_with_blocks(&[("paragraph", "Keep", "a")]);
        let new = doc_with_blocks(&[
            ("paragraph", "Keep", "a"),
            ("paragraph", "New1", "b"),
            ("paragraph", "New2", "c"),
        ]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].kind, DiffKind::Added);
        assert_eq!(diffs[0].blocks.len(), 2);
        assert_eq!(diffs[0].blocks[0].inline[0].text, "New1");
        assert_eq!(diffs[0].blocks[1].inline[0].text, "New2");
        assert_eq!(diffs[0].block_id.as_deref(), Some("b"));
    }

    #[test]
    fn diff_two_consecutive_removes_collapse_to_one_entry() {
        let old = doc_with_blocks(&[
            ("paragraph", "Keep", "a"),
            ("paragraph", "Gone1", "b"),
            ("paragraph", "Gone2", "c"),
        ]);
        let new = doc_with_blocks(&[("paragraph", "Keep", "a")]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].kind, DiffKind::Removed);
        assert_eq!(diffs[0].blocks.len(), 2);
    }

    #[test]
    fn diff_contiguous_mid_doc_replacement_is_one_removed_one_added() {
        // Regression: old [A,B,C,D] → new [A,X,Y,D]. Two adjacent
        // mismatched-id positions in the middle. Naive pair_blocks
        // would have emitted R(B), A(X), R(C), A(Y) — four cards
        // since group_runs can't merge across an interleaved Removed
        // and Added. Want one Removed[B,C] + one Added[X,Y] instead.
        //
        // Note: this fix only handles same-length mismatched runs.
        // Asymmetric runs (e.g. [A,B,C] → [A,X,Y,Z,C]) still
        // misclassify the trailing common block because the walker
        // pairs index-by-index without LCS alignment; that needs a
        // proper diff algorithm to fix.
        let old = doc_with_blocks(&[
            ("paragraph", "A", "a"),
            ("paragraph", "B", "b"),
            ("paragraph", "C", "c"),
            ("paragraph", "D", "d"),
        ]);
        let new = doc_with_blocks(&[
            ("paragraph", "A", "a"),
            ("paragraph", "X", "x"),
            ("paragraph", "Y", "y"),
            ("paragraph", "D", "d"),
        ]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 2, "expected one Removed + one Added card");
        assert_eq!(diffs[0].kind, DiffKind::Removed);
        assert_eq!(diffs[0].blocks.len(), 2);
        assert_eq!(diffs[0].blocks[0].inline[0].text, "B");
        assert_eq!(diffs[0].blocks[1].inline[0].text, "C");
        assert_eq!(diffs[1].kind, DiffKind::Added);
        assert_eq!(diffs[1].blocks.len(), 2);
        assert_eq!(diffs[1].blocks[0].inline[0].text, "X");
        assert_eq!(diffs[1].blocks[1].inline[0].text, "Y");
    }

    #[test]
    fn diff_modified_does_not_merge_with_neighboring_added() {
        // doc layout (old → new):
        //   keep, mod[a], gone           keep, mod[a]', new1, new2
        // expectations: one Modified, then one Added group of two.
        let old = doc_with_blocks(&[
            ("paragraph", "Keep", "k"),
            ("paragraph", "Old", "m"),
            ("paragraph", "Gone", "g"),
        ]);
        let new = doc_with_blocks(&[
            ("paragraph", "Keep", "k"),
            ("paragraph", "New", "m"),
            ("paragraph", "Add1", "n1"),
            ("paragraph", "Add2", "n2"),
        ]);
        let diffs = diff_documents(&old, &new);
        // Keep at 0 → no entry.
        // 'm' at 1 changed → Modified.
        // 'g' at 2 was Removed; 'n1' at 2 was Added; 'n2' at 3 was Added.
        // Removed and Added at the same/adjacent index are different
        // kinds, so neither merges with the other; n1+n2 merge into one
        // Added entry. → Modified, Removed, Added(2).
        assert_eq!(diffs.len(), 3);
        assert_eq!(diffs[0].kind, DiffKind::Modified);
        assert_eq!(diffs[1].kind, DiffKind::Removed);
        assert_eq!(diffs[2].kind, DiffKind::Added);
        assert_eq!(diffs[2].blocks.len(), 2);
    }

    #[test]
    fn diff_preserves_inline_marks() {
        let old = doc_with_marked_text("paragraph", &[("Hello", &[])]);
        let new = doc_with_marked_text(
            "paragraph",
            &[
                ("Hello ", &[]),
                ("bold", &[("bold", Any::Bool(true))]),
                (" world", &[]),
            ],
        );
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].kind, DiffKind::Modified);
        let new_block = &diffs[0].blocks[1];
        let bold_run = new_block
            .inline
            .iter()
            .find(|r| r.text == "bold")
            .expect("bold run present");
        assert!(bold_run.marks.iter().any(|m| matches!(m, Mark::Bold)));
    }

    #[test]
    fn diff_preserves_link_href() {
        let new = doc_with_marked_text(
            "paragraph",
            &[
                ("see ", &[]),
                (
                    "here",
                    &[(
                        "link",
                        Any::String(std::sync::Arc::from(r#"{"href":"https://example.com"}"#)),
                    )],
                ),
            ],
        );
        let old = doc_with_marked_text("paragraph", &[("see ", &[])]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        let link = diffs[0].blocks[1]
            .inline
            .iter()
            .find_map(|r| {
                r.marks.iter().find_map(|m| match m {
                    Mark::Link { href } => Some(href.clone()),
                    _ => None,
                })
            })
            .expect("link mark present");
        assert_eq!(link, "https://example.com");
    }

    #[test]
    fn diff_empty_docs() {
        let old = Doc::new();
        let new = Doc::new();
        assert!(diff_documents(&old, &new).is_empty());
    }

    #[test]
    fn diff_standalone_leaves_attribution_none() {
        let old = doc_with_blocks(&[("paragraph", "Hello", "a")]);
        let new = doc_with_blocks(&[("paragraph", "World", "a")]);
        let diffs = diff_documents(&old, &new);
        assert!(diffs.iter().all(|d| d.user_id.is_none()));
        assert!(diffs.iter().all(|d| d.timestamp.is_none()));
    }

    #[test]
    fn attributed_stamps_every_entry() {
        let old = doc_with_blocks(&[("paragraph", "A", "a"), ("paragraph", "B", "b")]);
        let new = doc_with_blocks(&[("paragraph", "A'", "a"), ("paragraph", "B'", "b")]);
        let diffs = diff_documents_attributed(
            &old,
            &new,
            Some("alice".to_string()),
            Some(1_700_000_000_000_000),
        );
        assert_eq!(diffs.len(), 2);
        for d in &diffs {
            assert_eq!(d.user_id.as_deref(), Some("alice"));
            assert_eq!(d.timestamp, Some(1_700_000_000_000_000));
        }
    }

    #[test]
    fn diff_node_type_set_to_first_block_tag() {
        let old = doc_with_blocks(&[("heading", "Title", "h"), ("paragraph", "Body", "b")]);
        let new = doc_with_blocks(&[("heading", "New Title", "h"), ("paragraph", "Body", "b")]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].node_type, "heading");
    }
}

// ─── Coverage-gap tests (additive; see module docs for the contracts
//     each one pins) ─────────────────────────────────────────────────

#[cfg(test)]
mod gap_tests {
    use super::*;
    use yrs::{
        types::Attrs,
        types::xml::{Xml, XmlElementPrelim, XmlFragment, XmlTextPrelim},
        Text, Transact, WriteTxn,
    };

    /// One bullet_list (blockId "list-1") whose `items` are
    /// `(blockId, text)` list_item children.
    fn list_doc(items: &[(&str, &str)]) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            let list = frag.insert(&mut txn, 0, XmlElementPrelim::empty("bullet_list"));
            list.insert_attribute(&mut txn, "blockId", "list-1");
            for (i, (id, text)) in items.iter().enumerate() {
                let li = list.insert(&mut txn, i as u32, XmlElementPrelim::empty("list_item"));
                li.insert_attribute(&mut txn, "blockId", *id);
                li.insert(&mut txn, 0, XmlTextPrelim::new(*text));
            }
        }
        doc
    }

    /// One heading (blockId "h-1") with a `level` attribute.
    fn heading_doc(level: &str, text: &str) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            let el = frag.insert(&mut txn, 0, XmlElementPrelim::empty("heading"));
            el.insert_attribute(&mut txn, "blockId", "h-1");
            el.insert_attribute(&mut txn, "level", level);
            el.insert(&mut txn, 0, XmlTextPrelim::new(text));
        }
        doc
    }

    /// Single paragraph whose text is split into formatted runs —
    /// same shape as the sibling test module's `doc_with_marked_text`
    /// (which is private to that module).
    fn marked_doc(runs: &[(&str, &[(&str, Any)])]) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            let el = frag.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
            el.insert_attribute(&mut txn, "blockId", "id-0");
            let combined: String = runs.iter().map(|(t, _)| *t).collect();
            let text = el.insert(&mut txn, 0, XmlTextPrelim::new(combined.as_str()));
            let mut offset: u32 = 0;
            for (run_text, run_marks) in runs {
                let len = run_text.chars().count() as u32;
                if !run_marks.is_empty() {
                    let mut attrs = Attrs::new();
                    for (k, v) in run_marks.iter() {
                        attrs.insert(std::sync::Arc::from(*k), v.clone());
                    }
                    text.format(&mut txn, offset, len, attrs);
                }
                offset += len;
            }
        }
        doc
    }

    /// Find the marks of the run carrying `text` in the doc's single
    /// paragraph, via the same walker `diff_documents` uses.
    fn marks_of_run(doc: &Doc, run_text: &str) -> Vec<Mark> {
        let blocks = extract_blocks(doc);
        assert_eq!(blocks.len(), 1, "expected a single top-level block");
        blocks[0]
            .inline
            .iter()
            .find(|r| r.text == run_text)
            .unwrap_or_else(|| panic!("no run with text {run_text:?} in {blocks:?}"))
            .marks
            .clone()
    }

    /// The module contract: block equality ignores `blockId` at
    /// every recursion level. Reassigning nested list_item ids
    /// without touching content must produce an empty diff.
    #[test]
    fn nested_child_block_id_reassignment_is_unchanged() {
        let old = list_doc(&[("li-a", "One"), ("li-b", "Two")]);
        let new = list_doc(&[("li-x", "One"), ("li-y", "Two")]);
        assert!(
            diff_documents(&old, &new).is_empty(),
            "child blockId reassignment alone must not produce a diff"
        );
    }

    /// An attribute-only change (heading level) with identical text
    /// classifies as Modified, and both attr versions surface in the
    /// entry's old/new blocks.
    #[test]
    fn attr_only_change_is_modified() {
        let old = heading_doc("1", "Title");
        let new = heading_doc("2", "Title");
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].kind, DiffKind::Modified);
        assert_eq!(diffs[0].blocks[0].attrs.get("level").map(String::as_str), Some("1"));
        assert_eq!(diffs[0].blocks[1].attrs.get("level").map(String::as_str), Some("2"));
    }

    /// A change buried in a nested child (list item text) classifies
    /// the top-level container as Modified and the new child content
    /// is reachable through the entry's `children`.
    #[test]
    fn nested_child_text_change_is_modified_on_parent() {
        let old = list_doc(&[("li-a", "One"), ("li-b", "Two")]);
        let new = list_doc(&[("li-a", "One"), ("li-b", "Two changed")]);
        let diffs = diff_documents(&old, &new);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].kind, DiffKind::Modified);
        assert_eq!(diffs[0].node_type, "bullet_list");
        let new_block = &diffs[0].blocks[1];
        assert_eq!(new_block.children.len(), 2);
        assert_eq!(new_block.children[1].inline[0].text, "Two changed");
    }

    /// TextColor and Highlight are loose CRDT-attribute marks outside
    /// the validated schema, but version-history attribution must
    /// still recognize them (module docs on `Mark`). Pins the two
    /// payload-carrying match arms nothing else exercises.
    #[test]
    fn text_color_and_highlight_marks_extracted() {
        let doc = marked_doc(&[
            ("plain ", &[]),
            (
                "colored",
                &[(
                    "textColor",
                    Any::String(std::sync::Arc::from(r##"{"color":"#ff0000"}"##)),
                )],
            ),
            (
                "highlit",
                &[(
                    "highlight",
                    Any::String(std::sync::Arc::from(r##"{"color":"#ffff00"}"##)),
                )],
            ),
        ]);
        assert_eq!(
            marks_of_run(&doc, "colored"),
            vec![Mark::TextColor { color: "#ff0000".to_string() }]
        );
        assert_eq!(
            marks_of_run(&doc, "highlit"),
            vec![Mark::Highlight { color: "#ffff00".to_string() }]
        );
    }

    /// A link mark whose stored value isn't valid JSON degrades to an
    /// empty href instead of panicking or dropping the run.
    #[test]
    fn malformed_link_json_yields_empty_href() {
        let doc = marked_doc(&[(
            "broken",
            &[("link", Any::String(std::sync::Arc::from("not-json")))],
        )]);
        assert_eq!(
            marks_of_run(&doc, "broken"),
            vec![Mark::Link { href: String::new() }]
        );
    }

    /// The remaining boolean mark arms (underline, strike, code,
    /// subscript, superscript) each map to their Mark variant. Bold
    /// and italic are covered by the sibling module's tests.
    #[test]
    fn all_boolean_marks_extracted() {
        let doc = marked_doc(&[
            ("u", &[("underline", Any::Bool(true))]),
            ("s", &[("strike", Any::Bool(true))]),
            ("c", &[("code", Any::Bool(true))]),
            ("b", &[("subscript", Any::Bool(true))]),
            ("p", &[("superscript", Any::Bool(true))]),
        ]);
        assert_eq!(marks_of_run(&doc, "u"), vec![Mark::Underline]);
        assert_eq!(marks_of_run(&doc, "s"), vec![Mark::Strike]);
        assert_eq!(marks_of_run(&doc, "c"), vec![Mark::Code]);
        assert_eq!(marks_of_run(&doc, "b"), vec![Mark::Subscript]);
        assert_eq!(marks_of_run(&doc, "p"), vec![Mark::Superscript]);
    }

    /// Mixed id presence at the same index — one side identified, the
    /// other legacy-unidentified — is an id mismatch, so it classifies
    /// as Removed + Added even when the content is identical. (Both-
    /// None pairs by position; Some-vs-None must not.)
    #[test]
    fn mixed_id_presence_at_same_index_is_remove_add() {
        let with_id = {
            let doc = Doc::new();
            {
                let mut txn = doc.transact_mut();
                let frag = txn.get_or_insert_xml_fragment("content");
                let el = frag.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
                el.insert_attribute(&mut txn, "blockId", "a");
                el.insert(&mut txn, 0, XmlTextPrelim::new("Same"));
            }
            doc
        };
        let without_id = {
            let doc = Doc::new();
            {
                let mut txn = doc.transact_mut();
                let frag = txn.get_or_insert_xml_fragment("content");
                let el = frag.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
                el.insert(&mut txn, 0, XmlTextPrelim::new("Same"));
            }
            doc
        };
        let diffs = diff_documents(&with_id, &without_id);
        assert_eq!(diffs.len(), 2, "expected Removed + Added, got {diffs:?}");
        assert_eq!(diffs[0].kind, DiffKind::Removed);
        assert_eq!(diffs[0].block_id.as_deref(), Some("a"));
        assert_eq!(diffs[1].kind, DiffKind::Added);
        assert_eq!(diffs[1].block_id, None);
    }
}

// ─── block_plain_text tests ──────────────────────────────────────────
//
// Own module (mirrors the `tests`/`gap_tests` split above) so its
// `doc_with_blocks(&[(block_id, text)])` helper — id-first, tag
// defaulted to "paragraph" — can share the name used informally in
// the task brief without colliding with the tag-first
// `doc_with_blocks(&[(tag, text, id)])` helper already defined in
// `mod tests`.
#[cfg(test)]
mod block_plain_text_tests {
    use super::*;
    use yrs::{
        types::xml::{Xml, XmlElementPrelim, XmlFragment, XmlTextPrelim},
        Transact, WriteTxn,
    };

    /// Build a doc whose top-level children are unmarked `paragraph`
    /// blocks, one per `(blockId, text)` pair. Same construction idiom
    /// as the sibling test modules' doc builders (`Doc::new` →
    /// `get_or_insert_xml_fragment("content")` → `XmlElementPrelim` +
    /// `insert_attribute("blockId", ..)` + `XmlTextPrelim`).
    fn doc_with_blocks(blocks: &[(&str, &str)]) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            for &(id, text) in blocks {
                let pos = frag.len(&txn);
                let el = frag.insert(&mut txn, pos, XmlElementPrelim::empty("paragraph"));
                el.insert_attribute(&mut txn, "blockId", id);
                el.insert(&mut txn, 0, XmlTextPrelim::new(text));
            }
        }
        doc
    }

    #[test]
    fn block_plain_text_finds_block_and_truncates() {
        let doc = doc_with_blocks(&[
            ("blk-alpha", "Hello mention world"),
            ("blk-long", &"x".repeat(200)),
        ]);
        assert_eq!(
            block_plain_text(&doc, "blk-alpha", 120).as_deref(),
            Some("Hello mention world")
        );
        let long = block_plain_text(&doc, "blk-long", 120).unwrap();
        assert_eq!(long.chars().count(), 120);
    }

    #[test]
    fn block_plain_text_missing_id_is_none() {
        let doc = doc_with_blocks(&[("blk-alpha", "Hello")]);
        assert!(block_plain_text(&doc, "no-such-block", 120).is_none());
    }

    #[test]
    fn block_plain_text_truncates_on_char_boundary() {
        // Multibyte content must not panic or split a char.
        let doc = doc_with_blocks(&[("blk-uni", &"é".repeat(200))]);
        let s = block_plain_text(&doc, "blk-uni", 120).unwrap();
        assert_eq!(s.chars().count(), 120);
    }
}
