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
        )
    }

    /// Whether this node type is an inline/leaf node.
    pub fn is_inline(&self) -> bool {
        matches!(self, NodeType::HardBreak)
    }

    /// Whether this node is a leaf (no children).
    pub fn is_leaf(&self) -> bool {
        matches!(
            self,
            NodeType::HorizontalRule | NodeType::HardBreak | NodeType::Image
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
            // Leaf/inline nodes and text containers have no element children
            NodeType::Paragraph
            | NodeType::Heading
            | NodeType::CodeBlock
            | NodeType::HorizontalRule
            | NodeType::HardBreak
            | NodeType::Image => &[],
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

    /// All MVP node types. Must match frontend/src/editor/model.rs NodeType enum.
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
    ];

    /// All MVP mark types. Must match frontend/src/editor/model.rs MarkType enum.
    const ALL_MARK_TYPES: &[MarkType] = &[
        MarkType::Bold,
        MarkType::Italic,
        MarkType::Underline,
        MarkType::Strike,
        MarkType::Code,
        MarkType::Link,
    ];

    #[test]
    fn cross_schema_node_type_count() {
        // If this fails, a node type was added to or removed from the collab
        // schema without updating the expected set. Update ALL_NODE_TYPES and
        // the corresponding frontend/src/editor/model.rs NodeType enum.
        assert_eq!(ALL_NODE_TYPES.len(), 13, "expected 13 MVP node types");
    }

    #[test]
    fn cross_schema_mark_type_count() {
        assert_eq!(ALL_MARK_TYPES.len(), 6, "expected 6 MVP mark types");
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
        let expected_leaves = [NodeType::HorizontalRule, NodeType::HardBreak, NodeType::Image];
        for nt in ALL_NODE_TYPES {
            let is_leaf = expected_leaves.contains(nt);
            assert_eq!(nt.is_leaf(), is_leaf, "leaf mismatch for {nt:?}");
        }
    }

    #[test]
    fn cross_schema_inline_nodes() {
        // Must match frontend: only HardBreak is inline
        for nt in ALL_NODE_TYPES {
            let expected = *nt == NodeType::HardBreak;
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
        // Text containers and leaves have no element children:
        assert!(NodeType::Paragraph.valid_children().is_empty());
        assert!(NodeType::Heading.valid_children().is_empty());
        assert!(NodeType::CodeBlock.valid_children().is_empty());
        assert!(NodeType::HorizontalRule.valid_children().is_empty());
        assert!(NodeType::HardBreak.valid_children().is_empty());
        assert!(NodeType::Image.valid_children().is_empty());
    }
}
