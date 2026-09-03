// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

/// Node types in the OgreNotes document schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeType {
    Doc,
    Paragraph,
    Heading,
    BulletList,
    OrderedList,
    ListItem,
    TaskList,
    TaskItem,
    Blockquote,
    CodeBlock,
    HorizontalRule,
    HardBreak,
    Image,
    Table,
    TableRow,
    TableCell,
    TableHeader,
    /// Phase 5 M-P6 — sandboxed third-party embed (YouTube,
    /// Vimeo, Figma, Loom, CodeSandbox, generic per-workspace
    /// allowlisted domains). Leaf block; attributes carry `url`,
    /// optional `title`, `provider`, and `height` (200..1200 px).
    /// Renders as a sandboxed iframe in HTML export; as a
    /// `[Embed: <title>](<url>)` link in Markdown export.
    Embed,
    /// #136 — inline calendar block. Container node holding zero
    /// or more `CalendarEvent` children. Attributes carry the
    /// active view (`month`/`week`/`day`), the display cursor
    /// (`YYYY-MM` for month; `YYYY-MM-DD` for week/day), and the
    /// IANA timezone the events render in. See
    /// [design/live-app-blocks.md] for the plugin interface this
    /// block is the pilot for.
    Calendar,
    /// #136 — one event inside a `Calendar` container. Leaf atom;
    /// attributes carry `color` (six-hue enum), `allDay`
    /// (`"true"`/`"false"`), the date-or-timestamp pair
    /// (`startDate`/`endDate` when all-day; `startAt`/`endAt`
    /// RFC 3339 UTC when timed), and a short `content` string.
    CalendarEvent,
    /// #137 — Kanban board. Container of `KanbanColumn` children.
    /// Atom + isolating. See design/live-app-blocks.md.
    Kanban,
    /// #137 — one column inside a `Kanban` board. Container of
    /// `KanbanCard` children. Attrs: `title` (header text, max
    /// 60 chars), optional `wipLimit` integer.
    KanbanColumn,
    /// #137 — one card inside a `KanbanColumn`. Leaf. Attrs:
    /// `title` (max 120), optional `content` (short description),
    /// `color` (six-hue enum reusing Calendar's palette).
    KanbanCard,
    /// #148 slice 6 — inline user @-mention as a leaf atom.
    /// Replaces the pre-existing text + `MarkType::Mention`
    /// shape. Attributes:
    ///
    /// - `user_id`: opaque server-side id of the mentioned user.
    /// - `display`: display name snapshot at insert time.
    ///   Cached so re-render doesn't need a live user lookup and
    ///   so a subsequent rename doesn't rewrite historic
    ///   mentions.
    ///
    /// Inline atom (takes one position). Deletes as a single
    /// keystroke — no partial-delete state where the user_id is
    /// still there but the display name has been chewed to
    /// "@ali".
    ///
    /// `MarkType::Mention` is retained in the schema for
    /// dual-read of documents written before this slice; new
    /// inserts always produce `NodeType::Mention` nodes.
    Mention,
    /// Inline document/anchor mention as a leaf atom (mentions spec §2).
    /// Carries the pasted `url`, target `doc_id`, optional
    /// `target_block_id` ("" ⇒ document mention), and cached
    /// `title`/`snippet`. Live/degraded render state is per-viewer and
    /// never stored. Single-keystroke delete like Mention.
    DocMention,
    /// Mermaid diagram block. Leaf atom; source stored in the
    /// `source` attribute. Rendered to SVG by `ogrenotes-mermaid`
    /// on both the client (live view) and server (HTML export).
    /// Mirrors the same-named variant in
    /// `frontend/src/editor/model.rs`.
    Mermaid,
    /// design/presentations.md — one slide in a presentation deck.
    /// Container of `Frame` children. Attrs: `layout` (preset id),
    /// `background` (optional theme-relative override).
    Slide,
    /// design/presentations.md — a positioned frame on a slide.
    /// Container of ordinary blocks. Attrs: `x`,`y`,`w`,`h`
    /// (normalized 0..1), `z` (stacking), `role`
    /// (`content` | `notes`).
    Frame,
}

impl NodeType {
    /// XML element name used in yrs XmlElement.
    pub fn tag_name(&self) -> &'static str {
        match self {
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
            NodeType::Mention => "mention",
            NodeType::DocMention => "doc_mention",
            NodeType::Mermaid => "mermaid",
            NodeType::Slide => "slide",
            NodeType::Frame => "frame",
        }
    }

    /// Parse a tag name back to a NodeType.
    pub fn from_tag(tag: &str) -> Option<Self> {
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
            "doc_mention" => Some(NodeType::DocMention),
            "mermaid" => Some(NodeType::Mermaid),
            "slide" => Some(NodeType::Slide),
            "frame" => Some(NodeType::Frame),
            _ => None,
        }
    }

    /// Whether this node type is a block node.
    pub fn is_block(&self) -> bool {
        matches!(
            self,
            NodeType::Paragraph
                | NodeType::Heading
                | NodeType::BulletList
                | NodeType::OrderedList
                | NodeType::ListItem
                | NodeType::TaskList
                | NodeType::TaskItem
                | NodeType::Blockquote
                | NodeType::CodeBlock
                | NodeType::HorizontalRule
                | NodeType::Image
                | NodeType::Table
                | NodeType::TableRow
                | NodeType::TableCell
                | NodeType::TableHeader
                | NodeType::Embed
                | NodeType::Calendar
                | NodeType::CalendarEvent
                | NodeType::Kanban
                | NodeType::KanbanColumn
                | NodeType::KanbanCard
                | NodeType::Mermaid
                | NodeType::Slide
                | NodeType::Frame
        )
    }

    /// Whether this node type is an inline/leaf node.
    pub fn is_inline(&self) -> bool {
        matches!(self, NodeType::HardBreak | NodeType::Mention | NodeType::DocMention)
    }

    /// Whether this node is a leaf (no children).
    pub fn is_leaf(&self) -> bool {
        matches!(
            self,
            NodeType::HorizontalRule
                | NodeType::HardBreak
                | NodeType::Image
                | NodeType::Embed
                | NodeType::CalendarEvent
                | NodeType::KanbanCard
                | NodeType::Mention
                | NodeType::DocMention
                | NodeType::Mermaid
        )
    }

    /// Whether this node's content should be treated as code (no marks).
    pub fn is_code(&self) -> bool {
        matches!(self, NodeType::CodeBlock)
    }

    /// Valid child node types for this node.
    pub fn valid_children(&self) -> &'static [NodeType] {
        match self {
            NodeType::Doc => &[
                NodeType::Paragraph,
                NodeType::Heading,
                NodeType::BulletList,
                NodeType::OrderedList,
                NodeType::TaskList,
                NodeType::Blockquote,
                NodeType::CodeBlock,
                NodeType::HorizontalRule,
                NodeType::Image,
                NodeType::Table,
                NodeType::Embed,
                NodeType::Calendar,
                NodeType::Kanban,
                NodeType::Mermaid,
                NodeType::Slide,
            ],
            NodeType::BulletList => &[NodeType::ListItem],
            NodeType::OrderedList => &[NodeType::ListItem],
            NodeType::TaskList => &[NodeType::TaskItem],
            NodeType::ListItem => &[
                NodeType::Paragraph,
                NodeType::BulletList,
                NodeType::OrderedList,
                NodeType::TaskList,
                NodeType::Blockquote,
                NodeType::CodeBlock,
            ],
            NodeType::TaskItem => &[
                NodeType::Paragraph,
                NodeType::BulletList,
                NodeType::OrderedList,
                NodeType::TaskList,
                NodeType::Blockquote,
                NodeType::CodeBlock,
            ],
            NodeType::Blockquote => &[
                NodeType::Paragraph,
                NodeType::Heading,
                NodeType::BulletList,
                NodeType::OrderedList,
                NodeType::TaskList,
                NodeType::Blockquote,
                NodeType::CodeBlock,
                NodeType::HorizontalRule,
                NodeType::Image,
            ],
            NodeType::Table => &[NodeType::TableRow],
            NodeType::TableRow => &[NodeType::TableCell, NodeType::TableHeader],
            NodeType::TableCell | NodeType::TableHeader => &[
                NodeType::Paragraph,
                NodeType::Heading,
                NodeType::BulletList,
                NodeType::OrderedList,
                NodeType::TaskList,
                NodeType::Blockquote,
                NodeType::CodeBlock,
            ],
            NodeType::Calendar => &[NodeType::CalendarEvent],
            NodeType::Kanban => &[NodeType::KanbanColumn],
            NodeType::KanbanColumn => &[NodeType::KanbanCard],
            NodeType::Slide => &[NodeType::Frame],
            NodeType::Frame => &[
                NodeType::Paragraph,
                NodeType::Heading,
                NodeType::BulletList,
                NodeType::OrderedList,
                NodeType::TaskList,
                NodeType::Blockquote,
                NodeType::CodeBlock,
                NodeType::HorizontalRule,
                NodeType::Image,
                NodeType::Table,
                NodeType::Embed,
                NodeType::Calendar,
                NodeType::Kanban,
                NodeType::Mermaid,
            ],
            // Leaf/inline nodes and text containers have no element children
            NodeType::Paragraph
            | NodeType::Heading
            | NodeType::CodeBlock
            | NodeType::HorizontalRule
            | NodeType::HardBreak
            | NodeType::Image
            | NodeType::Embed
            | NodeType::CalendarEvent
            | NodeType::KanbanCard
            | NodeType::Mention
            | NodeType::DocMention
            | NodeType::Mermaid => &[],
        }
    }
}

/// Mark types (inline formatting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkType {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Link,
    Subscript,
    Superscript,
    /// #148 — user @-mention. Text carrying this mark renders
    /// as a mention chip; the `user_id` attribute stores the
    /// mentioned user's id and is treated as opaque server-side.
    /// Chose text+mark over a dedicated `NodeType::Mention` atom
    /// to keep the change out of the 10-file NodeType cascade
    /// (schema mirroring, yrs_bridge tags, export/markdown paths).
    Mention,
}

impl MarkType {
    /// Attribute name used in yrs text formatting.
    pub fn attr_name(&self) -> &'static str {
        match self {
            MarkType::Bold => "bold",
            MarkType::Italic => "italic",
            MarkType::Underline => "underline",
            MarkType::Strike => "strike",
            MarkType::Code => "code",
            MarkType::Link => "link",
            MarkType::Subscript => "subscript",
            MarkType::Superscript => "superscript",
            MarkType::Mention => "mention",
        }
    }

    /// Parse an attribute name back to a MarkType.
    pub fn from_attr(attr: &str) -> Option<Self> {
        match attr {
            "bold" => Some(MarkType::Bold),
            "italic" => Some(MarkType::Italic),
            "underline" => Some(MarkType::Underline),
            "strike" => Some(MarkType::Strike),
            "code" => Some(MarkType::Code),
            "link" => Some(MarkType::Link),
            "subscript" => Some(MarkType::Subscript),
            "superscript" => Some(MarkType::Superscript),
            "mention" => Some(MarkType::Mention),
            _ => None,
        }
    }

    /// Whether this mark excludes all other marks (like code).
    pub fn excludes_all(&self) -> bool {
        matches!(self, MarkType::Code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_type_tag_roundtrip() {
        let types = [
            NodeType::Doc,
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
            NodeType::Table,
            NodeType::TableRow,
            NodeType::TableCell,
            NodeType::TableHeader,
            NodeType::Embed,
            NodeType::Calendar,
            NodeType::CalendarEvent,
            NodeType::Kanban,
            NodeType::KanbanColumn,
            NodeType::KanbanCard,
            NodeType::Mention,
            NodeType::DocMention,
            NodeType::Mermaid,
            NodeType::Slide,
            NodeType::Frame,
        ];

        for nt in &types {
            let tag = nt.tag_name();
            let back = NodeType::from_tag(tag);
            assert_eq!(back, Some(*nt), "roundtrip failed for {tag}");
        }
    }

    #[test]
    fn mark_type_attr_roundtrip() {
        let types = [
            MarkType::Bold,
            MarkType::Italic,
            MarkType::Underline,
            MarkType::Strike,
            MarkType::Code,
            MarkType::Link,
            MarkType::Subscript,
            MarkType::Superscript,
            MarkType::Mention,
        ];

        for mt in &types {
            let attr = mt.attr_name();
            let back = MarkType::from_attr(attr);
            assert_eq!(back, Some(*mt), "roundtrip failed for {attr}");
        }
    }

    #[test]
    fn paragraph_accepts_no_block_children() {
        // Paragraph contains inline content (text), not block elements
        let children = NodeType::Paragraph.valid_children();
        assert!(children.is_empty());
    }

    #[test]
    fn doc_accepts_blocks() {
        let children = NodeType::Doc.valid_children();
        assert!(children.contains(&NodeType::Paragraph));
        assert!(children.contains(&NodeType::Heading));
        assert!(children.contains(&NodeType::BulletList));
        assert!(children.contains(&NodeType::Blockquote));
    }

    #[test]
    fn bullet_list_only_accepts_list_items() {
        let children = NodeType::BulletList.valid_children();
        assert_eq!(children, &[NodeType::ListItem]);
    }

    #[test]
    fn list_item_accepts_paragraphs_and_nested_lists() {
        let children = NodeType::ListItem.valid_children();
        assert!(children.contains(&NodeType::Paragraph));
        assert!(children.contains(&NodeType::BulletList)); // nested lists
        // HorizontalRule and Image are NOT valid inside list items
        assert!(!children.contains(&NodeType::HorizontalRule));
        assert!(!children.contains(&NodeType::Image));
    }

    #[test]
    fn code_block_is_code() {
        assert!(NodeType::CodeBlock.is_code());
        assert!(!NodeType::Paragraph.is_code());
    }

    #[test]
    fn horizontal_rule_is_leaf() {
        assert!(NodeType::HorizontalRule.is_leaf());
        assert!(NodeType::HardBreak.is_leaf());
        assert!(NodeType::Image.is_leaf());
        assert!(!NodeType::Paragraph.is_leaf());
    }

    #[test]
    fn code_mark_excludes_all() {
        assert!(MarkType::Code.excludes_all());
        assert!(!MarkType::Bold.excludes_all());
        assert!(!MarkType::Link.excludes_all());
    }

    #[test]
    fn unknown_tag_returns_none() {
        assert_eq!(NodeType::from_tag("unknown"), None);
        assert_eq!(MarkType::from_attr("unknown"), None);
    }

    // ── Cross-schema consistency tests ──────────────────────────────
    //
    // These tests verify that the collab crate's schema matches the
    // expected set defined in the frontend editor (model.rs + schema.rs).
    // If a node or mark type is added/removed on either side, these
    // tests must be updated to reflect the change on BOTH sides.
    //
    // See "Schema Duality" in design/mvp-detailed-design.md.

    /// Render this crate's schema as the duality fixture. Every object is a
    /// `BTreeMap` (not `json!({..})`) so key order is alphabetical whether
    /// or not some other crate in the build graph turns on serde_json's
    /// `preserve_order` feature — the workspace build does, a crate-alone
    /// build doesn't, and the file must be byte-stable under both.
    fn duality_fixture_json() -> String {
        use std::collections::BTreeMap;
        let mut nodes: BTreeMap<String, BTreeMap<&str, serde_json::Value>> = BTreeMap::new();
        for nt in ALL_NODE_TYPES {
            let mut children: Vec<String> =
                nt.valid_children().iter().map(|c| format!("{c:?}")).collect();
            children.sort();
            let mut entry = BTreeMap::new();
            entry.insert("children", serde_json::Value::from(children));
            entry.insert("leaf", serde_json::Value::from(nt.is_leaf()));
            entry.insert("inline", serde_json::Value::from(nt.is_inline()));
            nodes.insert(format!("{nt:?}"), entry);
        }
        let mut marks: Vec<String> = ALL_MARK_TYPES.iter().map(|m| format!("{m:?}")).collect();
        marks.sort();
        let mut top: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        top.insert("nodes", serde_json::to_value(nodes).expect("nodes"));
        top.insert("marks", serde_json::Value::from(marks));
        serde_json::to_string_pretty(&top).expect("serialize fixture") + "\n"
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

    /// All node types. Must match frontend/src/editor/model.rs NodeType enum.
    const ALL_NODE_TYPES: &[NodeType] = &[
        NodeType::Doc,
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
        NodeType::Table,
        NodeType::TableRow,
        NodeType::TableCell,
        NodeType::TableHeader,
        NodeType::Embed,
        NodeType::Calendar,
        NodeType::CalendarEvent,
        NodeType::Kanban,
        NodeType::KanbanColumn,
        NodeType::KanbanCard,
        NodeType::Mention,
        NodeType::DocMention,
        NodeType::Mermaid,
        NodeType::Slide,
        NodeType::Frame,
    ];

    /// The schema-validated mark types. Must match the marks registered in
    /// the frontend's `schema.rs` (its `MarkSpec` map) — NOT the frontend's
    /// `model.rs::MarkType` enum, which has two additional variants
    /// (`TextColor`, `Highlight`). Those are intentionally *outside* the
    /// validated schema: they're "loose" formatting applied to any inline
    /// text with no exclusion/content rules, stored as raw CRDT text
    /// attributes (`textColor`/`highlight`) and rendered by the frontend.
    /// The full attribute-mark set (these 8 + the 2 color marks) is
    /// enumerated separately in `diff.rs::Mark` (version-history attribution)
    /// and `model.rs::MarkType` (editor application). The backend does not
    /// validate inline marks, but it *does* export them (`export.rs`), so this
    /// list pins the schema-duality contract for the marks both schemas
    /// constrain. Subscript/Superscript (#143) are full toggle marks like
    /// Bold — schema-validated, unlike the parameterized color marks.
    const ALL_MARK_TYPES: &[MarkType] = &[
        MarkType::Bold,
        MarkType::Italic,
        MarkType::Underline,
        MarkType::Strike,
        MarkType::Code,
        MarkType::Link,
        MarkType::Subscript,
        MarkType::Superscript,
        MarkType::Mention,
    ];

    #[test]
    fn cross_schema_node_type_count() {
        // If this fails, a node type was added to or removed from the collab
        // schema without updating the expected set. Update ALL_NODE_TYPES and
        // the corresponding frontend/src/editor/model.rs NodeType enum.
        assert_eq!(ALL_NODE_TYPES.len(), 28, "expected 28 node types");
    }

    #[test]
    fn cross_schema_mark_type_count() {
        assert_eq!(ALL_MARK_TYPES.len(), 9, "expected 9 schema-validated mark types");
    }

    #[test]
    fn cross_schema_all_node_tags_roundtrip() {
        // Every node type must have a tag name, and from_tag must recover it.
        for nt in ALL_NODE_TYPES {
            let tag = nt.tag_name();
            assert_eq!(
                NodeType::from_tag(tag),
                Some(*nt),
                "tag roundtrip failed for {tag}"
            );
        }
    }

    #[test]
    fn cross_schema_all_mark_attrs_roundtrip() {
        for mt in ALL_MARK_TYPES {
            let attr = mt.attr_name();
            assert_eq!(
                MarkType::from_attr(attr),
                Some(*mt),
                "attr roundtrip failed for {attr}"
            );
        }
    }

    #[test]
    fn cross_schema_tag_names() {
        // Tag names must match what the yrs bridge in the frontend expects.
        let expected: &[(&str, NodeType)] = &[
            ("doc", NodeType::Doc),
            ("paragraph", NodeType::Paragraph),
            ("heading", NodeType::Heading),
            ("bullet_list", NodeType::BulletList),
            ("ordered_list", NodeType::OrderedList),
            ("list_item", NodeType::ListItem),
            ("task_list", NodeType::TaskList),
            ("task_item", NodeType::TaskItem),
            ("blockquote", NodeType::Blockquote),
            ("code_block", NodeType::CodeBlock),
            ("horizontal_rule", NodeType::HorizontalRule),
            ("hard_break", NodeType::HardBreak),
            ("image", NodeType::Image),
            ("table", NodeType::Table),
            ("table_row", NodeType::TableRow),
            ("table_cell", NodeType::TableCell),
            ("table_header", NodeType::TableHeader),
            ("embed", NodeType::Embed),
            ("calendar", NodeType::Calendar),
            ("calendar_event", NodeType::CalendarEvent),
            ("kanban", NodeType::Kanban),
            ("kanban_column", NodeType::KanbanColumn),
            ("kanban_card", NodeType::KanbanCard),
            ("mention", NodeType::Mention),
            ("doc_mention", NodeType::DocMention),
            ("mermaid", NodeType::Mermaid),
            ("slide", NodeType::Slide),
            ("frame", NodeType::Frame),
        ];
        for (tag, nt) in expected {
            assert_eq!(nt.tag_name(), *tag, "tag mismatch for {nt:?}");
        }
    }

    #[test]
    fn cross_schema_mark_attr_names() {
        let expected: &[(&str, MarkType)] = &[
            ("bold", MarkType::Bold),
            ("italic", MarkType::Italic),
            ("underline", MarkType::Underline),
            ("strike", MarkType::Strike),
            ("code", MarkType::Code),
            ("link", MarkType::Link),
            ("mention", MarkType::Mention),
        ];
        for (attr, mt) in expected {
            assert_eq!(mt.attr_name(), *attr, "attr name mismatch for {mt:?}");
        }
    }

    #[test]
    fn cross_schema_code_mark_excludes_all() {
        // Must match frontend MarkSpec for Code: exclude_all: true
        assert!(MarkType::Code.excludes_all());
        // All others must NOT exclude all
        for mt in ALL_MARK_TYPES {
            if *mt != MarkType::Code {
                assert!(!mt.excludes_all(), "{mt:?} should not exclude all");
            }
        }
    }

    #[test]
    fn cross_schema_leaf_nodes() {
        // Must match frontend NodeSpec: leaf: true
        let expected_leaves = [
            NodeType::HorizontalRule,
            NodeType::HardBreak,
            NodeType::Image,
            NodeType::Embed,
            NodeType::CalendarEvent,
            NodeType::KanbanCard,
            NodeType::Mention,
            NodeType::DocMention,
            NodeType::Mermaid,
        ];
        for nt in ALL_NODE_TYPES {
            let is_leaf = expected_leaves.contains(nt);
            assert_eq!(nt.is_leaf(), is_leaf, "leaf mismatch for {nt:?}");
        }
    }

    #[test]
    fn cross_schema_inline_nodes() {
        // Must match frontend: HardBreak, Mention (#148 slice 6), and
        // DocMention are inline; everything else is block-level.
        let expected_inline = [NodeType::HardBreak, NodeType::Mention, NodeType::DocMention];
        for nt in ALL_NODE_TYPES {
            let expected = expected_inline.contains(nt);
            assert_eq!(nt.is_inline(), expected, "inline mismatch for {nt:?}");
        }
    }

    #[test]
    fn cross_schema_valid_children() {
        // Must match frontend default_schema() NodeSpec::valid_children.
        // Doc children:
        assert_eq!(
            NodeType::Doc.valid_children(),
            &[
                NodeType::Paragraph, NodeType::Heading, NodeType::BulletList,
                NodeType::OrderedList, NodeType::TaskList, NodeType::Blockquote,
                NodeType::CodeBlock, NodeType::HorizontalRule, NodeType::Image,
                NodeType::Table, NodeType::Embed, NodeType::Calendar,
                NodeType::Kanban, NodeType::Mermaid, NodeType::Slide,
            ]
        );
        // List containers:
        assert_eq!(NodeType::BulletList.valid_children(), &[NodeType::ListItem]);
        assert_eq!(NodeType::OrderedList.valid_children(), &[NodeType::ListItem]);
        assert_eq!(NodeType::TaskList.valid_children(), &[NodeType::TaskItem]);
        // ListItem / TaskItem:
        let list_item_children = &[
            NodeType::Paragraph, NodeType::BulletList, NodeType::OrderedList,
            NodeType::TaskList, NodeType::Blockquote, NodeType::CodeBlock,
        ];
        assert_eq!(NodeType::ListItem.valid_children(), list_item_children);
        assert_eq!(NodeType::TaskItem.valid_children(), list_item_children);
        // Blockquote:
        assert_eq!(
            NodeType::Blockquote.valid_children(),
            &[
                NodeType::Paragraph, NodeType::Heading, NodeType::BulletList,
                NodeType::OrderedList, NodeType::TaskList, NodeType::Blockquote,
                NodeType::CodeBlock, NodeType::HorizontalRule, NodeType::Image,
            ]
        );
        // Table:
        assert_eq!(NodeType::Table.valid_children(), &[NodeType::TableRow]);
        assert_eq!(
            NodeType::TableRow.valid_children(),
            &[NodeType::TableCell, NodeType::TableHeader]
        );
        let cell_children = &[
            NodeType::Paragraph, NodeType::Heading, NodeType::BulletList,
            NodeType::OrderedList, NodeType::TaskList, NodeType::Blockquote,
            NodeType::CodeBlock,
        ];
        assert_eq!(NodeType::TableCell.valid_children(), cell_children);
        assert_eq!(NodeType::TableHeader.valid_children(), cell_children);
        // Calendar: container of events only.
        assert_eq!(
            NodeType::Calendar.valid_children(),
            &[NodeType::CalendarEvent]
        );
        assert!(NodeType::CalendarEvent.valid_children().is_empty());
        // Kanban: container of columns; column contains cards; card is leaf.
        assert_eq!(
            NodeType::Kanban.valid_children(),
            &[NodeType::KanbanColumn]
        );
        assert_eq!(
            NodeType::KanbanColumn.valid_children(),
            &[NodeType::KanbanCard]
        );
        assert!(NodeType::KanbanCard.valid_children().is_empty());
        // Presentation: slides contain frames; frames contain blocks.
        assert_eq!(
            NodeType::Slide.valid_children(),
            &[NodeType::Frame]
        );
        let frame_children = &[
            NodeType::Paragraph,
            NodeType::Heading,
            NodeType::BulletList,
            NodeType::OrderedList,
            NodeType::TaskList,
            NodeType::Blockquote,
            NodeType::CodeBlock,
            NodeType::HorizontalRule,
            NodeType::Image,
            NodeType::Table,
            NodeType::Embed,
            NodeType::Calendar,
            NodeType::Kanban,
            NodeType::Mermaid,
        ];
        assert_eq!(NodeType::Frame.valid_children(), frame_children);
        // Text containers and leaves have no element children:
        assert!(NodeType::Paragraph.valid_children().is_empty());
        assert!(NodeType::Heading.valid_children().is_empty());
        assert!(NodeType::CodeBlock.valid_children().is_empty());
        assert!(NodeType::HorizontalRule.valid_children().is_empty());
        assert!(NodeType::HardBreak.valid_children().is_empty());
        assert!(NodeType::Image.valid_children().is_empty());
        assert!(NodeType::Embed.valid_children().is_empty());
    }
}
