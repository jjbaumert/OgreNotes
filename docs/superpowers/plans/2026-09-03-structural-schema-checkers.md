# Structural Schema Checkers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** End the orphan-container bug class generically: make frontend/backend schema parity actually enforced, assert schema validity over every importer's output and after every editor transaction, self-heal the deeper orphan shapes the editor already knows how to fix, and close the two synchronous modal-close panics the survey found.

**Architecture:** A machine-readable schema fixture (`crates/collab/schema-duality.json`) becomes the single source both crates assert against, so drift on either side fails that side's own test. collab gains `validate::schema_violations(&yrs::Doc)`, a public structural checker generalized from the Quip importer's private test helper, and `tests/import_fuzz.rs` runs every importer through it. The frontend gains a native-only proptest that drives random command sequences and calls `Schema::validate` after each apply, plus a deeper `needs_normalize`/`normalize_node` that wraps bare text and non-item children under list containers. Two components switch to `a11y::defer_close`.

**Tech Stack:** Rust (yrs, proptest, serde_json), Leptos frontend (native `cargo test --lib`, `a11y::defer_close`).

**Status (2026-09-03):** Tasks 1–5 done on branch `test-gap-followups`. The new checks found and fixed five defects: frontend Blockquote spec listed Table (removed); from_html stranded text in list items/quotes and put blocks under lists; from_markdown emitted Heading/HorizontalRule inside list items; toggle_list wrapped a Heading in a ListItem; a cross-item delete left an empty nested list unhealed.

**Spec:** Survey items C3, K1, F1, D20, D21 in the "OgreNotes Test Gap Survey" artifact (https://claude.ai/code/artifact/dbc8825f-fecc-48c2-bfad-64b32167e0cb); backlog item 1 of `docs/superpowers/plans/2026-09-02-test-gap-remediation.md`.

## Global Constraints

- Existing tests are immutable. Add tests; never edit an existing test body.
- Identifiers stay raw `String`. No newtypes.
- Do not edit `design/`, `framework/`, or `runbook/`.
- `crates/collab/src/schema.rs` (canonical) and `frontend/src/editor/schema.rs` (parallel) must stay in lockstep; the fixture is the enforcement mechanism, not a third schema. A fixture change is a deliberate schema change.
- proptest is native-only in the frontend: gate new prop modules with `#[cfg(all(test, not(target_arch = "wasm32")))]` exactly like `transform.rs::swap_remap_prop_tests`.
- If a new property test fails, that is a defect, not a reason to weaken the property. Shrink, diagnose, fix the importer/command, and keep the property. If a fix is out of reach in this plan, stop and report it with the shrunk input rather than allowlisting it.
- Never `git add -A`. Stage by name. `git push` is denied to the agent. Commit messages end with `Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1`.
- Branch: `test-gap-followups` (already exists off main at the frontend folder-delete commit).

---

### Task 1: Schema-duality fixture asserted from both crates (C3)

**Files:**
- Create: `crates/collab/schema-duality.json`
- Modify: `crates/collab/src/schema.rs` (test module: new tests only)
- Modify: `frontend/src/editor/schema.rs` (add `Schema::node_types()`; new test module)
- Modify: `frontend/src/editor/model.rs` (add `NodeType::ALL` and `MarkType::ALL` consts)

**Interfaces:**
- Produces: fixture shape
  ```json
  { "nodes": { "Paragraph": { "children": [], "leaf": false, "inline": false }, ... },
    "marks": ["Bold", "Italic", ...] }
  ```
  keyed by enum variant Debug names, `children` sorted, node map sorted (BTreeMap).
- Produces: `frontend::editor::schema::Schema::node_types(&self) -> Vec<NodeType>`, `frontend::editor::model::NodeType::ALL: &[NodeType]`, `MarkType::ALL: &[MarkType]`.

- [x] **Step 1: Write the collab generator test (fails: no fixture file)**

In `crates/collab/src/schema.rs`, inside the existing `mod tests`, add:

```rust
    /// Render this crate's schema as the duality fixture. Sorted maps and
    /// child lists so the JSON is byte-stable.
    fn duality_fixture_json() -> String {
        use std::collections::BTreeMap;
        let mut nodes = BTreeMap::new();
        for nt in ALL_NODE_TYPES {
            let mut children: Vec<String> =
                nt.valid_children().iter().map(|c| format!("{c:?}")).collect();
            children.sort();
            nodes.insert(
                format!("{nt:?}"),
                serde_json::json!({
                    "children": children,
                    "leaf": nt.is_leaf(),
                    "inline": nt.is_inline(),
                }),
            );
        }
        let mut marks: Vec<String> = ALL_MARK_TYPES.iter().map(|m| format!("{m:?}")).collect();
        marks.sort();
        let v = serde_json::json!({ "nodes": nodes, "marks": marks });
        serde_json::to_string_pretty(&v).expect("serialize fixture") + "\n"
    }

    /// The fixture at `crates/collab/schema-duality.json` is what the
    /// frontend's `schema_duality_fixture_matches_this_crate` asserts
    /// against. This side regenerates it from the canonical schema; a
    /// mismatch means either this crate drifted from the fixture or the
    /// fixture needs a deliberate update (paste the printed JSON), after
    /// which the frontend test tells you what still has to move there.
    #[test]
    fn schema_duality_fixture_matches_this_crate() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/schema-duality.json");
        let on_disk = std::fs::read_to_string(path).expect("schema-duality.json must exist");
        let expected = duality_fixture_json();
        assert!(
            on_disk == expected,
            "schema-duality.json is out of date. Expected contents:\n{expected}"
        );
    }

    /// Every mark's attribute name round-trips; the older
    /// `cross_schema_mark_attr_names` pins only seven of nine.
    #[test]
    fn every_mark_attr_name_round_trips() {
        for m in ALL_MARK_TYPES {
            assert_eq!(MarkType::from_attr(m.attr_name()), Some(*m), "{m:?}");
        }
    }
```

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-collab --lib schema_duality_fixture --locked`
Expected: FAIL, `schema-duality.json must exist`. The panic message is not printed for a missing file; temporarily create an empty file and re-run to capture the expected JSON, or generate it with the next step.

- [x] **Step 3: Write the fixture from the generator**

Run:
```bash
cd /home/kender/projects/rust/ogre
touch crates/collab/schema-duality.json
cargo test -p ogrenotes-collab --lib schema_duality_fixture --locked 2>&1 | sed -n '/Expected contents:/,/^}$/p' | sed '1d' > crates/collab/schema-duality.json
cargo test -p ogrenotes-collab --lib schema_duality_fixture every_mark --locked
```
Expected: both tests PASS. Inspect the file: 28 node entries, 9 marks.

- [x] **Step 4: Add `NodeType::ALL`, `MarkType::ALL`, and `Schema::node_types()` on the frontend**

In `frontend/src/editor/model.rs`, inside `impl NodeType` (before `is_leaf`):

```rust
    /// Every variant, for exhaustive tests. A new variant must be added
    /// here; `schema_duality_fixture_matches_this_crate` counts them.
    pub const ALL: &'static [NodeType] = &[
        NodeType::Doc, NodeType::Paragraph, NodeType::Heading, NodeType::BulletList,
        NodeType::OrderedList, NodeType::ListItem, NodeType::TaskList, NodeType::TaskItem,
        NodeType::Blockquote, NodeType::CodeBlock, NodeType::HorizontalRule, NodeType::HardBreak,
        NodeType::Image, NodeType::Table, NodeType::TableRow, NodeType::TableCell,
        NodeType::TableHeader, NodeType::Embed, NodeType::Calendar, NodeType::CalendarEvent,
        NodeType::Kanban, NodeType::KanbanColumn, NodeType::KanbanCard, NodeType::Mention,
        NodeType::DocMention, NodeType::Mermaid, NodeType::Slide, NodeType::Frame,
    ];
```
Inside `impl MarkType`, add a `pub const ALL` listing every MarkType variant (read the enum first: `grep -n "pub enum MarkType" -A 30 frontend/src/editor/model.rs`).

In `frontend/src/editor/schema.rs`, inside `impl Schema` next to `node_spec`:

```rust
    /// Every node type this schema has a spec for.
    pub fn node_types(&self) -> Vec<NodeType> {
        self.nodes.keys().copied().collect()
    }
```

- [x] **Step 5: Write the frontend fixture test**

Append to `frontend/src/editor/schema.rs`:

```rust
#[cfg(test)]
mod duality_fixture_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The canonical schema lives in `crates/collab/src/schema.rs`; that
    /// crate regenerates `crates/collab/schema-duality.json` from it and
    /// this test holds the parallel schema to the same file. Drift on
    /// either side fails that side's own test.
    const FIXTURE: &str = include_str!("../../../crates/collab/schema-duality.json");

    #[test]
    fn schema_duality_fixture_matches_this_crate() {
        let v: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture parses");
        let nodes = v["nodes"].as_object().expect("nodes map");
        let schema = default_schema();

        assert_eq!(nodes.len(), NodeType::ALL.len(), "node count: fixture vs NodeType::ALL");
        assert_eq!(schema.node_types().len(), NodeType::ALL.len(), "every NodeType must have a spec");

        for nt in NodeType::ALL {
            let name = format!("{nt:?}");
            let entry = nodes.get(&name).unwrap_or_else(|| panic!("fixture has no node {name}"));
            let spec = schema.node_spec(*nt).unwrap_or_else(|| panic!("no spec for {name}"));

            let want: BTreeSet<String> = entry["children"]
                .as_array().expect("children")
                .iter().map(|c| c.as_str().unwrap().to_string()).collect();
            let have: BTreeSet<String> = spec.valid_children.iter().map(|c| format!("{c:?}")).collect();
            assert_eq!(have, want, "valid_children of {name} differ from the collab schema");
            assert_eq!(spec.leaf, entry["leaf"].as_bool().unwrap(), "leaf flag of {name}");
            assert_eq!(nt.is_inline(), entry["inline"].as_bool().unwrap(), "inline flag of {name}");
        }

        let marks: BTreeSet<String> = v["marks"].as_array().expect("marks")
            .iter().map(|m| m.as_str().unwrap().to_string()).collect();
        let have: BTreeSet<String> = MarkType::ALL.iter().map(|m| format!("{m:?}")).collect();
        assert_eq!(have, marks, "mark set differs from the collab schema");
    }
}
```

- [x] **Step 6: Run both sides**

Run:
```bash
cd /home/kender/projects/rust/ogre/frontend && cargo test --lib duality_fixture --locked
cd /home/kender/projects/rust/ogre && cargo test -p ogrenotes-collab --lib schema_duality --locked
```
Expected: PASS on both. If the frontend fails on a specific node, that is real drift; report it and fix the side that is wrong (the collab schema is canonical).

- [x] **Step 7: Commit**

```bash
git add crates/collab/schema-duality.json crates/collab/src/schema.rs frontend/src/editor/schema.rs frontend/src/editor/model.rs
git commit -m "test(schema): enforce frontend/backend schema duality through a shared fixture

The collab cross_schema tests compared literal arrays declared in the
same file against the same crate; nothing read the frontend. Both
crates now assert against crates/collab/schema-duality.json.

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 2: `schema_violations` in collab and the importer-wide property (K1)

**Files:**
- Create: `crates/collab/src/validate.rs`
- Modify: `crates/collab/src/lib.rs` (`pub mod validate;`)
- Modify: `crates/collab/tests/import_fuzz.rs` (append a proptest block)

**Interfaces:**
- Produces: `pub fn ogrenotes_collab::validate::schema_violations(doc: &yrs::Doc) -> Vec<String>` (empty = valid).

- [x] **Step 1: Write the module with its own unit tests**

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Structural validity of a materialized document against the canonical
//! schema. Generalizes the Quip importer's private `assert_valid_tree`:
//! every importer and transform must leave a tree this function accepts,
//! because downstream an invalid tree is document corruption — content
//! stranded outside any textblock is unreachable by `find_block_at` and
//! surfaces as undeletable fragments (the orphan-container bug class).

use yrs::types::xml::{Xml, XmlElementRef, XmlFragment, XmlOut};
use yrs::{Doc, ReadTxn, Transact};

use crate::schema::NodeType;

/// Every structural rule the document tree violates, as human-readable
/// strings with a path. Empty means valid.
///
/// Rules:
/// 1. Every element tag names a `NodeType`.
/// 2. A child element is legal under its parent per `valid_children`,
///    with the inline exemption `insert_block` also makes (`valid_children`
///    is a block-containment predicate and never lists inline leaves).
/// 3. A leaf node has no children.
/// 4. Raw text may appear only inside a textblock (a non-leaf node whose
///    `valid_children` is empty: Paragraph, Heading, CodeBlock).
pub fn schema_violations(doc: &Doc) -> Vec<String> {
    let txn = doc.transact();
    let mut out = Vec::new();
    let Some(frag) = txn.get_xml_fragment("content") else {
        return out;
    };
    for i in 0..frag.len(&txn) {
        if let Some(XmlOut::Element(el)) = frag.get(&txn, i) {
            walk(&txn, &el, NodeType::Doc, &format!("content[{i}]"), &mut out);
        }
    }
    out
}

fn is_textblock(nt: NodeType) -> bool {
    !nt.is_leaf() && nt.valid_children().is_empty() && !nt.is_inline()
}

fn walk<T: ReadTxn>(txn: &T, el: &XmlElementRef, parent: NodeType, path: &str, out: &mut Vec<String>) {
    let tag = el.tag().to_string();
    let Some(nt) = NodeType::from_tag(&tag) else {
        out.push(format!("{path}: unknown tag <{tag}>"));
        return;
    };
    if !(nt.is_inline() || parent.valid_children().contains(&nt)) {
        out.push(format!("{path}: {nt:?} is not a legal child of {parent:?}"));
    }
    let len = el.len(txn);
    if nt.is_leaf() && len > 0 {
        out.push(format!("{path}: leaf {nt:?} has {len} children"));
    }
    for i in 0..len {
        let child_path = format!("{path}/{tag}[{i}]");
        match el.get(txn, i) {
            Some(XmlOut::Element(child)) => walk(txn, &child, nt, &child_path, out),
            Some(XmlOut::Text(_)) => {
                if !is_textblock(nt) {
                    out.push(format!("{child_path}: raw text inside {nt:?}"));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::types::xml::{XmlElementPrelim, XmlTextPrelim};
    use yrs::WriteTxn;

    fn doc_with(build: impl FnOnce(&mut yrs::TransactionMut, &yrs::XmlFragmentRef)) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            build(&mut txn, &frag);
        }
        doc
    }

    #[test]
    fn paragraph_with_text_is_valid() {
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty("paragraph"));
            p.insert(txn, 0, XmlTextPrelim::new("hi"));
        });
        assert!(schema_violations(&doc).is_empty());
    }

    #[test]
    fn bare_text_inside_a_list_is_reported() {
        let doc = doc_with(|txn, frag| {
            let ul = frag.insert(txn, 0, XmlElementPrelim::empty("bullet_list"));
            ul.insert(txn, 0, XmlTextPrelim::new("orphan"));
        });
        let v = schema_violations(&doc);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("raw text inside BulletList"), "{v:?}");
    }

    #[test]
    fn paragraph_directly_under_a_list_is_reported() {
        let doc = doc_with(|txn, frag| {
            let ul = frag.insert(txn, 0, XmlElementPrelim::empty("bullet_list"));
            ul.insert(txn, 0, XmlElementPrelim::empty("paragraph"));
        });
        let v = schema_violations(&doc);
        assert!(v.iter().any(|s| s.contains("Paragraph is not a legal child of BulletList")), "{v:?}");
    }

    #[test]
    fn unknown_tag_and_leaf_with_children_are_reported() {
        let doc = doc_with(|txn, frag| {
            frag.insert(txn, 0, XmlElementPrelim::empty("marquee"));
            let hr = frag.insert(txn, 1, XmlElementPrelim::empty("horizontal_rule"));
            hr.insert(txn, 0, XmlTextPrelim::new("x"));
        });
        let v = schema_violations(&doc);
        assert!(v.iter().any(|s| s.contains("unknown tag <marquee>")), "{v:?}");
        assert!(v.iter().any(|s| s.contains("leaf HorizontalRule has 1 children")), "{v:?}");
    }
}
```
Check the tag names used (`bullet_list`, `horizontal_rule`) against `NodeType::tag_name` in `crates/collab/src/schema.rs:99` and adjust.

Add `pub mod validate;` to `crates/collab/src/lib.rs` after `pub mod themes;`.

- [x] **Step 2: Run the unit tests**

Run: `cargo test -p ogrenotes-collab --lib validate:: --locked`
Expected: 4 passed.

- [x] **Step 3: Add the importer-wide property to `import_fuzz.rs`**

Append after the existing top-level `proptest!` block (before `mod binary`):

```rust
use ogrenotes_collab::import_spreadsheet::from_csv;
use ogrenotes_collab::validate::schema_violations;

fn assert_schema_valid(doc: &yrs::Doc, importer: &str, src: &str) -> Result<(), TestCaseError> {
    let v = schema_violations(doc);
    prop_assert!(v.is_empty(), "{importer} produced an invalid tree for {src:?}:\n{}", v.join("\n"));
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    /// Every text importer leaves a tree the canonical schema accepts —
    /// the generic form of the orphan-container guard that only the Quip
    /// path had.
    #[test]
    fn text_importers_emit_schema_valid_trees(s in "\\PC*") {
        assert_schema_valid(&from_markdown(&s), "from_markdown", &s)?;
        assert_schema_valid(&from_html(&s), "from_html", &s)?;
        assert_schema_valid(&from_quip_html(&s).doc, "from_quip_html", &s)?;
        assert_schema_valid(&from_csv(&s), "from_csv", &s)?;
    }

    /// Markup that *looks* like the shapes that produced past orphans:
    /// text and blocks directly inside list / table containers.
    #[test]
    fn html_importer_never_strands_text_in_containers(
        pre in "[a-z ]{0,8}", inner in "[a-z ]{0,8}", post in "[a-z ]{0,8}",
        container in prop::sample::select(vec!["ul", "ol", "table", "tr", "blockquote", "li"]),
    ) {
        let html = format!("<{container}>{pre}<p>{inner}</p>{post}</{container}>");
        assert_schema_valid(&from_html(&html), "from_html", &html)?;
        let md_ish = format!("- {pre}\n\n{inner}\n  - {post}\n");
        assert_schema_valid(&from_markdown(&md_ish), "from_markdown", &md_ish)?;
    }
}
```
Inside `mod binary`, add one more case to its existing proptest block (new fn only):

```rust
        #[test]
        fn binary_importers_emit_schema_valid_trees(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            if let Ok(doc) = ogrenotes_collab::import_docx::from_docx(&bytes) {
                super::assert_schema_valid(&doc, "from_docx", "<bytes>")?;
            }
            if let Ok(doc) = ogrenotes_collab::import_spreadsheet::from_xlsx(&bytes) {
                super::assert_schema_valid(&doc, "from_xlsx", "<bytes>")?;
            }
        }
```

- [x] **Step 4: Run the fuzz targets**

Run:
```bash
cd /home/kender/projects/rust/ogre
cargo test -p ogrenotes-collab --test import_fuzz --locked --features xlsx,docx,pdf 2>&1 | grep -E "^test |^test result|minimal failing|panicked|invalid tree" | head -30
```
Expected: PASS. If a property fails, proptest prints the shrunk input; treat it as a defect in that importer, fix it in the importer (never in the property), and re-run. Record each such fix in its own commit.

- [x] **Step 5: Commit**

```bash
git add crates/collab/src/validate.rs crates/collab/src/lib.rs crates/collab/tests/import_fuzz.rs
git commit -m "test(collab): schema_violations checker and importer-wide schema-validity property

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 3: Deep normalization of list orphans in the editor (D20)

**Files:**
- Modify: `frontend/src/editor/state.rs` (`needs_normalize`)
- Modify: `frontend/src/editor/model.rs` (`normalize_node`)
- Test: `frontend/src/editor/state.rs` (new test module)

- [x] **Step 1: Write the failing test**

Append to `frontend/src/editor/state.rs`:

```rust
#[cfg(test)]
mod deep_normalize_tests {
    use super::*;
    use crate::editor::model::{Fragment, Node, NodeType};
    use crate::editor::schema::default_schema;

    fn p(text: &str) -> Node {
        Node::element_with_content(NodeType::Paragraph, Fragment::from(vec![Node::text(text)]))
    }
    fn li(children: Vec<Node>) -> Node {
        Node::element_with_content(NodeType::ListItem, Fragment::from(children))
    }
    fn doc(children: Vec<Node>) -> Node {
        Node::element_with_content(NodeType::Doc, Fragment::from(children))
    }

    /// `needs_normalize` used to look two levels deep, so bare text or a
    /// bare paragraph directly inside a list (depth 3) survived `apply`.
    /// Every past instance was patched per-path; this is the generic cure.
    #[test]
    fn apply_self_heals_a_bare_text_orphan_nested_inside_a_list() {
        let corrupt = doc(vec![Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![li(vec![p("A")]), Node::text("orphan"), p("bare")]),
        )]);
        assert!(default_schema().validate(&corrupt).is_err(), "precondition: corrupt");

        let state = EditorState::create_default(corrupt);
        let healed = state.apply(state.transaction());

        assert!(
            default_schema().validate(&healed.doc).is_ok(),
            "apply must leave a schema-valid doc: {:?}",
            healed.doc
        );
        let list = healed.doc.child(0).unwrap();
        assert_eq!(list.child_count(), 3, "orphan text and bare paragraph each became an item");
        assert!(healed.doc.text_content().contains("orphan"), "no content lost");
    }

    #[test]
    fn apply_is_a_no_op_on_a_valid_doc() {
        let ok = doc(vec![Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![li(vec![p("A")]), li(vec![p("B")])]),
        )]);
        let state = EditorState::create_default(ok.clone());
        let after = state.apply(state.transaction());
        assert_eq!(after.doc, ok);
    }
}
```
Check the accessor names (`child`, `child_count`, `text_content`) exist on `Node` (`grep -n "pub fn child\b\|pub fn child_count\|pub fn text_content" frontend/src/editor/model.rs`) and that `Node: PartialEq`.

- [x] **Step 2: Run to verify it fails**

Run: `cd frontend && cargo test --lib deep_normalize_tests --locked`
Expected: the first test FAILS (`apply must leave a schema-valid doc`); the second passes.

- [x] **Step 3: Widen `needs_normalize` and teach `normalize_node` about list containers**

In `state.rs`, replace the body of `needs_normalize` with a recursive walk that keeps every existing top-level rule and adds the list rule:

```rust
fn needs_normalize(doc: &Node) -> bool {
    let Node::Element { content, node_type, .. } = doc else { return false };
    if *node_type != NodeType::Doc { return false; }
    content.children.iter().any(|child| match child {
        Node::Text { .. } => true, // bare text under Doc
        Node::Element { node_type, content: child_content, .. } => {
            matches!(node_type,
                NodeType::ListItem | NodeType::TaskItem
                | NodeType::TableRow | NodeType::TableCell | NodeType::TableHeader
            )
            || (matches!(node_type,
                NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList
                | NodeType::Table
            ) && child_content.children.is_empty())
            || (node_type.is_textblock() && child_content.children.iter().any(|gc| {
                matches!(gc, Node::Element { node_type: nt, .. } if nt.is_block() && !nt.is_inline())
            }))
            || has_list_orphan(child)
        }
    })
}

/// Deep rule: a list container may hold only its item type. Bare text or
/// any other block directly inside it is the orphan-container class
/// (unreachable by `find_block_at`, undeletable in the UI).
fn has_list_orphan(node: &Node) -> bool {
    let Node::Element { node_type, content, .. } = node else { return false };
    let item = match node_type {
        NodeType::BulletList | NodeType::OrderedList => Some(NodeType::ListItem),
        NodeType::TaskList => Some(NodeType::TaskItem),
        _ => None,
    };
    if let Some(item) = item {
        if content.children.iter().any(|c| !matches!(c, Node::Element { node_type: nt, .. } if *nt == item)) {
            return true;
        }
    }
    content.children.iter().any(has_list_orphan)
}
```

In `model.rs::normalize_node`, add a list-container arm before the textblock arm (inside the `Node::Element { node_type, content, .. } =>` branch, after the orphaned-structural-node early return):

```rust
            // List containers hold only their item type; wrap anything else
            // (bare text, a stray paragraph) with `ensure_list_item`, the
            // same cure the paste path applies, and recurse so nested lists
            // are healed too.
            if let Some(item) = match node_type {
                NodeType::BulletList | NodeType::OrderedList => Some(NodeType::ListItem),
                NodeType::TaskList => Some(NodeType::TaskItem),
                _ => None,
            } {
                let children: Vec<Node> = content
                    .children
                    .iter()
                    .flat_map(|c| normalize_node(c, *node_type))
                    .map(|c| ensure_list_item(c, item))
                    .collect();
                return vec![node.copy_with_content(Fragment::from(children))];
            }
```
Make sure the generic recursion at the end of `normalize_node` (the arm that rebuilds children for other containers) still runs for non-list containers, so a list nested under a blockquote is reached. Read the rest of `normalize_node` first (`sed -n 1120,1160p frontend/src/editor/model.rs`).

- [x] **Step 4: Run the editor suites**

Run: `cd frontend && cargo test --lib editor:: --locked 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: all pass, including the new module. Any pre-existing test that now fails is a behavior change to surface, not to paper over.

- [x] **Step 5: Commit**

```bash
git add frontend/src/editor/state.rs frontend/src/editor/model.rs
git commit -m "fix(editor): normalize orphans nested inside list containers on apply

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 4: Every command leaves a schema-valid doc (F1)

**Files:**
- Create: `frontend/src/editor/structural_props.rs`
- Modify: `frontend/src/editor/mod.rs` (register, native-only)

- [x] **Step 1: Write the property module**

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Native-only structural property: no sequence of editor commands can
//! leave the document outside the schema. Schema::validate recurses fully
//! and would catch every orphan class, but its only callers were its own
//! unit tests; this drives it after every `apply`.

use proptest::prelude::*;

use super::commands::{delete_selection, shift_tab_command, tab_command, toggle_list};
use super::model::{Fragment, Node, NodeType};
use super::schema::default_schema;
use super::selection::Selection;
use super::state::{EditorState, Transaction};

fn p(text: &str) -> Node {
    Node::element_with_content(NodeType::Paragraph, Fragment::from(vec![Node::text(text)]))
}
fn li(text: &str) -> Node {
    Node::element_with_content(NodeType::ListItem, Fragment::from(vec![p(text)]))
}

/// Seed corpus: one doc per container family the past orphans came from.
fn seeds() -> Vec<Node> {
    let d = |children: Vec<Node>| Node::element_with_content(NodeType::Doc, Fragment::from(children));
    vec![
        d(vec![p("alpha"), p("beta")]),
        d(vec![
            p("intro"),
            Node::element_with_content(NodeType::BulletList, Fragment::from(vec![li("one"), li("two")])),
            p("outro"),
        ]),
        d(vec![
            Node::element_with_content(NodeType::Blockquote, Fragment::from(vec![p("quoted")])),
            Node::element_with_content(NodeType::CodeBlock, Fragment::from(vec![Node::text("let x = 1;")])),
        ]),
        d(vec![
            Node::element_with_attrs(
                NodeType::Heading,
                [("level".to_string(), "2".to_string())].into_iter().collect(),
                Fragment::from(vec![Node::text("Title")]),
            ),
            Node::element(NodeType::HorizontalRule),
            p("after rule"),
        ]),
    ]
}

#[derive(Debug, Clone)]
enum Op {
    Insert(String),
    Split,
    JoinBackward,
    JoinForward,
    DeleteSelection,
    Tab,
    ShiftTab,
    ToggleBullet,
    MoveCursor(usize),
    Select(usize, usize),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        "[a-z ]{1,4}".prop_map(Op::Insert),
        Just(Op::Split),
        Just(Op::JoinBackward),
        Just(Op::JoinForward),
        Just(Op::DeleteSelection),
        Just(Op::Tab),
        Just(Op::ShiftTab),
        Just(Op::ToggleBullet),
        (0usize..64).prop_map(Op::MoveCursor),
        (0usize..64, 0usize..64).prop_map(|(a, b)| Op::Select(a.min(b), a.max(b))),
    ]
}

fn run_command(
    state: &EditorState,
    f: impl Fn(&EditorState, Option<&dyn Fn(Transaction)>) -> bool,
) -> EditorState {
    let captured = std::cell::RefCell::new(None);
    let dispatch = |txn: Transaction| {
        *captured.borrow_mut() = Some(txn);
    };
    f(state, Some(&dispatch));
    match captured.into_inner() {
        Some(txn) => state.apply(txn),
        None => state.clone(),
    }
}

fn snap_cursor(doc: &Node, pos: usize) -> Selection {
    let pos = pos.min(doc.content_size());
    Selection::find_from(doc, pos, 1)
        .or_else(|| Selection::find_from(doc, pos, -1))
        .unwrap_or_else(|| Selection::cursor(1))
}

fn step(state: EditorState, op: &Op) -> EditorState {
    match op {
        Op::Insert(s) => match state.transaction().insert_text(s) {
            Ok(txn) => state.apply(txn),
            Err(_) => state,
        },
        Op::Split => match state.transaction().split_block() {
            Ok(txn) => state.apply(txn),
            Err(_) => state,
        },
        Op::JoinBackward => match state.transaction().join_backward() {
            Ok(txn) => state.apply(txn),
            Err(_) => state,
        },
        Op::JoinForward => match state.transaction().join_forward() {
            Ok(txn) => state.apply(txn),
            Err(_) => state,
        },
        Op::DeleteSelection => run_command(&state, delete_selection),
        Op::Tab => run_command(&state, tab_command),
        Op::ShiftTab => run_command(&state, shift_tab_command),
        Op::ToggleBullet => run_command(&state, |s, d| {
            toggle_list(NodeType::BulletList, NodeType::ListItem, s, d)
        }),
        Op::MoveCursor(pos) => EditorState {
            selection: snap_cursor(&state.doc, *pos),
            ..state
        },
        Op::Select(a, b) => {
            let size = state.doc.content_size();
            let (a, b) = ((*a).min(size), (*b).min(size));
            let from = snap_cursor(&state.doc, a).from();
            let to = snap_cursor(&state.doc, b).from().max(from);
            EditorState {
                selection: Selection::range(from, to),
                ..state
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// After every applied transaction the document validates against the
    /// schema — no bare text or stray block inside a container, no leaf
    /// with children, no illegal parent/child pair.
    #[test]
    fn every_command_leaves_a_schema_valid_doc(
        seed in 0usize..4,
        ops in proptest::collection::vec(arb_op(), 1..20),
    ) {
        let schema = default_schema();
        let mut state = EditorState::create_default(seeds()[seed].clone());
        prop_assert!(schema.validate(&state.doc).is_ok(), "seed {seed} must be valid");
        for (i, op) in ops.iter().enumerate() {
            let before = state.doc.clone();
            state = step(state, op);
            if let Err(e) = schema.validate(&state.doc) {
                prop_assert!(
                    false,
                    "op #{i} {op:?} broke the schema: {e}\nbefore: {before:?}\nafter:  {:?}\nops: {ops:?}",
                    state.doc
                );
            }
        }
    }
}
```
Check `Selection::range` exists (`grep -n "pub fn range\|pub fn text" frontend/src/editor/selection.rs`) and that `EditorState: Clone`. If range selections are built differently, use that constructor.

In `frontend/src/editor/mod.rs` add:
```rust
// Native-only: proptest doesn't build for wasm32 (see frontend/Cargo.toml).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod structural_props;
```

- [x] **Step 2: Run it**

Run: `cd frontend && cargo test --lib structural_props --locked 2>&1 | grep -E "^test |^test result|minimal failing|broke the schema" -A 12 | head -60`
Expected: PASS. A failure prints the op sequence and before/after docs; that is a real editor defect. Fix it in the command that produced it (or in `normalize_node` if it is a shape `apply` should heal), keep the property, re-run.

- [x] **Step 3: Commit**

```bash
git add frontend/src/editor/structural_props.rs frontend/src/editor/mod.rs
git commit -m "test(editor): proptest that every command sequence leaves a schema-valid doc

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 5: Deferred close for the template picker and find bar (D21)

**Files:**
- Modify: `frontend/src/components/template_picker_modal.rs:132-215`
- Modify: `frontend/src/components/find_replace_bar.rs:167`

- [x] **Step 1: Template picker**

Replace each `on:click=move |_| on_close.run(())` (backdrop at line 133, the two ✕ buttons at 156 and 212) with `on:click=move |_| crate::a11y::defer_close(on_close)`. Add a focus trap and Escape handler mirroring `folder_picker.rs:33,127-150`:

- before the `view!`: `let dialog_ref = NodeRef::<leptos::html::Div>::new(); crate::a11y::install_focus_trap(dialog_ref, visible.into());` (check `install_focus_trap`'s `visible: Signal<bool>` accepts `ReadSignal` via `.into()`).
- on the `role="dialog"` div add `node_ref=dialog_ref` and
  ```rust
  on:keydown=move |e: web_sys::KeyboardEvent| {
      if e.key() == "Escape" {
          crate::a11y::defer_close(on_close);
          return;
      }
      if let Some(node) = dialog_ref.get() {
          crate::a11y::handle_tab_trap(&e, node.as_ref());
      }
  }
  ```
Leave the async success path in `submit_copy` alone: it runs after an await, not inside the click turn.

- [x] **Step 2: Find bar**

Change the close button to `on:click=move |_| crate::a11y::defer_close(on_close)`.

- [x] **Step 3: Verify it compiles for both targets**

Run:
```bash
cd frontend && cargo check --locked && cargo check --target wasm32-unknown-unknown --locked 2>&1 | grep -E "^error|Finished"
```
Expected: `Finished` for both, no errors. There is no automated test for this change: the doctor's new global pageerror gate is the regression net for the panic class, and no CI scenario opens the template picker today. Say so in the commit message.

- [x] **Step 4: Commit**

```bash
git add frontend/src/components/template_picker_modal.rs frontend/src/components/find_replace_bar.rs
git commit -m "fix(frontend): defer template-picker and find-bar closes past the click turn

Synchronous <Show> teardown inside on:click is the shape of the
\"closure invoked recursively or after being dropped\" panic class.
Also gives the template picker Escape + a focus trap like every other
dialog. No automated test: the doctor pageerror gate is the net.

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 6: Verification and handoff

- [ ] **Step 1: Run everything CI runs for the touched crates**

```bash
cd /home/kender/projects/rust/ogre
cargo test -p ogrenotes-collab --lib --locked 2>&1 | grep -E "^test result|FAILED"
cargo test -p ogrenotes-collab --tests --locked --features xlsx,docx,pdf 2>&1 | grep -E "^test result|FAILED"
cargo check --workspace --all-targets --locked 2>&1 | tail -1
cd frontend && cargo test --lib --locked 2>&1 | grep -E "^test result|FAILED"
cargo check --target wasm32-unknown-unknown --locked 2>&1 | tail -1
```
Expected: no `FAILED`.

- [ ] **Step 2: Hand off**

Ask the user to push `test-gap-followups`, open the PR with `gh pr create`, watch CI with `gh run watch --exit-status`. Then run the Playwright validation last: `gh workflow run playwright.yml --ref <branch>` and watch it.
