use std::collections::HashMap;

use super::model::{Mark, MarkType, Node, NodeType};

/// Schema definition for document validation.
/// Maps node types to their specs and mark types to their specs.
#[derive(Debug, Clone)]
pub struct Schema {
    nodes: HashMap<NodeType, NodeSpec>,
    marks: HashMap<MarkType, MarkSpec>,
}

/// Specification for a node type.
#[derive(Debug, Clone)]
pub struct NodeSpec {
    /// Valid child node types. Empty for text-containing nodes (paragraph, heading, code block).
    pub valid_children: Vec<NodeType>,
    /// Whether this node contains inline content (text + inline elements).
    pub inline_content: bool,
    /// Whether this node is a block node.
    pub block: bool,
    /// Whether this is a leaf node (no content).
    pub leaf: bool,
    /// Whether content is treated as code (no marks).
    pub code: bool,
    /// Whether this is an atom (selected as a unit).
    pub atom: bool,
    /// Whether this node defines its type when content is replaced.
    pub defining: bool,
    /// Whether editing operations don't cross this node's boundaries.
    pub isolating: bool,
    /// Default attributes for this node type.
    pub default_attrs: HashMap<String, String>,
    /// Allowed mark types. None means all marks allowed.
    /// Some(empty vec) means no marks allowed.
    pub allowed_marks: Option<Vec<MarkType>>,
}

/// Specification for a mark type.
#[derive(Debug, Clone)]
pub struct MarkSpec {
    /// Whether the mark extends to content typed at its boundary.
    pub inclusive: bool,
    /// Mark types excluded by this mark. Empty means no exclusions.
    /// Use `exclude_all: true` for marks like Code that exclude everything.
    pub exclude_all: bool,
    /// Specific mark types excluded by this mark.
    pub excludes: Vec<MarkType>,
}

impl Schema {
    /// Check if a sequence of child nodes is valid for a parent node type.
    pub fn content_matches(&self, parent_type: NodeType, children: &[&Node]) -> bool {
        let Some(spec) = self.nodes.get(&parent_type) else {
            return false;
        };

        // Leaf nodes accept no children
        if spec.leaf {
            return children.is_empty();
        }

        // Inline content nodes accept text and inline elements.
        // Code nodes only accept text (no inline elements like HardBreak).
        if spec.inline_content {
            return children.iter().all(|child| match child {
                Node::Text { .. } => true,
                Node::Element { node_type, .. } => {
                    if spec.code {
                        false // code blocks: text only
                    } else {
                        node_type.is_inline()
                    }
                }
            });
        }

        // Block content nodes: check each child is in valid_children
        if spec.valid_children.is_empty() {
            return children.is_empty();
        }

        // Enforce minimum cardinality: list containers require 1+ children
        let requires_children = matches!(
            parent_type,
            NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList
        );
        if requires_children && children.is_empty() {
            return false;
        }

        children.iter().all(|child| match child {
            Node::Text { .. } => false, // text not allowed in block-content nodes
            Node::Element { node_type, .. } => spec.valid_children.contains(node_type),
        })
    }

    /// Check if a mark type can be applied to text content within a node type.
    /// This checks whether the node allows marks on its inline content,
    /// not whether the mark can be placed on the node element itself.
    pub fn can_apply_mark(&self, node_type: NodeType, mark_type: MarkType) -> bool {
        let Some(node_spec) = self.nodes.get(&node_type) else {
            return false;
        };

        // Code nodes don't allow marks
        if node_spec.code {
            return false;
        }

        // Check node's allowed marks whitelist
        if let Some(ref allowed) = node_spec.allowed_marks {
            if !allowed.contains(&mark_type) {
                return false;
            }
        }

        true
    }

    /// Check if two marks are compatible (can coexist on the same text).
    /// Two marks of the same type with different attrs are compatible
    /// (e.g., two links with different hrefs -- though this is unusual).
    /// However, if the mark type has `exclude_all`, even duplicates are invalid.
    pub fn marks_compatible(&self, a: &Mark, b: &Mark) -> bool {
        let a_spec = self.marks.get(&a.mark_type);
        let b_spec = self.marks.get(&b.mark_type);

        // Check if a excludes b
        if let Some(spec) = a_spec {
            if spec.exclude_all && a.mark_type != b.mark_type {
                return false;
            }
            if spec.exclude_all && a.mark_type == b.mark_type && a != b {
                // Two different instances of an exclude_all mark (e.g., two Code marks)
                return false;
            }
            if spec.excludes.contains(&b.mark_type) {
                return false;
            }
        }

        // Check if b excludes a
        if let Some(spec) = b_spec {
            if spec.exclude_all && a.mark_type != b.mark_type {
                return false;
            }
            if spec.exclude_all && a.mark_type == b.mark_type && a != b {
                return false;
            }
            if spec.excludes.contains(&a.mark_type) {
                return false;
            }
        }

        // Same type, same attrs -- compatible (they're duplicates, dedup handles this)
        // Different types, no exclusion rules triggered -- compatible
        true
    }

    /// Check if a mark set is valid (no exclusion violations).
    pub fn marks_valid(&self, marks: &[Mark]) -> bool {
        for i in 0..marks.len() {
            for j in (i + 1)..marks.len() {
                if !self.marks_compatible(&marks[i], &marks[j]) {
                    return false;
                }
            }
        }
        true
    }

    /// Get the node spec for a node type.
    pub fn node_spec(&self, node_type: NodeType) -> Option<&NodeSpec> {
        self.nodes.get(&node_type)
    }

    /// Get the mark spec for a mark type.
    pub fn mark_spec(&self, mark_type: MarkType) -> Option<&MarkSpec> {
        self.marks.get(&mark_type)
    }

    /// Validate an entire document against the schema.
    pub fn validate(&self, doc: &Node) -> Result<(), SchemaError> {
        self.validate_node(doc)
    }

    fn validate_node(&self, node: &Node) -> Result<(), SchemaError> {
        match node {
            Node::Text { marks, .. } => {
                if !self.marks_valid(marks) {
                    return Err(SchemaError::InvalidMarks);
                }
                Ok(())
            }
            Node::Element {
                node_type,
                content,
                marks,
                ..
            } => {
                if !self.marks_valid(marks) {
                    return Err(SchemaError::InvalidMarks);
                }

                // Validate children
                let child_refs: Vec<&Node> = content.children.iter().collect();
                if !self.content_matches(*node_type, &child_refs) {
                    return Err(SchemaError::InvalidContent(*node_type));
                }

                // Recursively validate children
                for child in &content.children {
                    self.validate_node(child)?;
                }

                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum SchemaError {
    InvalidContent(NodeType),
    InvalidMarks,
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::InvalidContent(nt) => write!(f, "invalid content for {nt:?}"),
            SchemaError::InvalidMarks => write!(f, "invalid mark combination"),
        }
    }
}

// ─── Default Schema ─────────────────────────────────────────────

/// Build the default OgreNotes document schema with all MVP node and mark types.
pub fn default_schema() -> Schema {
    let mut nodes = HashMap::new();

    // Document root
    nodes.insert(
        NodeType::Doc,
        NodeSpec {
            valid_children: vec![
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
            inline_content: false,
            block: false,
            leaf: false,
            code: false,
            atom: false,
            defining: false,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]), // no marks on doc
        },
    );

    // Paragraph: contains inline content
    nodes.insert(
        NodeType::Paragraph,
        NodeSpec {
            valid_children: vec![],
            inline_content: true,
            block: true,
            leaf: false,
            code: false,
            atom: false,
            defining: false,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: None, // all marks
        },
    );

    // Heading: contains inline content, has level attr
    let mut heading_attrs = HashMap::new();
    heading_attrs.insert("level".to_string(), "1".to_string());
    nodes.insert(
        NodeType::Heading,
        NodeSpec {
            valid_children: vec![],
            inline_content: true,
            block: true,
            leaf: false,
            code: false,
            atom: false,
            defining: true,
            isolating: false,
            default_attrs: heading_attrs,
            allowed_marks: None,
        },
    );

    // BulletList: contains list items
    nodes.insert(
        NodeType::BulletList,
        NodeSpec {
            valid_children: vec![NodeType::ListItem],
            inline_content: false,
            block: true,
            leaf: false,
            code: false,
            atom: false,
            defining: false,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]),
        },
    );

    // OrderedList: contains list items
    nodes.insert(
        NodeType::OrderedList,
        NodeSpec {
            valid_children: vec![NodeType::ListItem],
            inline_content: false,
            block: true,
            leaf: false,
            code: false,
            atom: false,
            defining: false,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]),
        },
    );

    // ListItem: contains blocks (paragraph, nested lists, etc.)
    nodes.insert(
        NodeType::ListItem,
        NodeSpec {
            valid_children: vec![
                NodeType::Paragraph,
                NodeType::BulletList,
                NodeType::OrderedList,
                NodeType::TaskList,
                NodeType::Blockquote,
                NodeType::CodeBlock,
            ],
            inline_content: false,
            block: false,
            leaf: false,
            code: false,
            atom: false,
            defining: true,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]),
        },
    );

    // TaskList: contains task items
    nodes.insert(
        NodeType::TaskList,
        NodeSpec {
            valid_children: vec![NodeType::TaskItem],
            inline_content: false,
            block: true,
            leaf: false,
            code: false,
            atom: false,
            defining: false,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]),
        },
    );

    // TaskItem: contains blocks, has checked attr
    let mut task_attrs = HashMap::new();
    task_attrs.insert("checked".to_string(), "false".to_string());
    nodes.insert(
        NodeType::TaskItem,
        NodeSpec {
            valid_children: vec![
                NodeType::Paragraph,
                NodeType::BulletList,
                NodeType::OrderedList,
                NodeType::TaskList,
                NodeType::Blockquote,
                NodeType::CodeBlock,
            ],
            inline_content: false,
            block: false,
            leaf: false,
            code: false,
            atom: false,
            defining: true,
            isolating: false,
            default_attrs: task_attrs,
            allowed_marks: Some(vec![]),
        },
    );

    // Blockquote: contains blocks
    nodes.insert(
        NodeType::Blockquote,
        NodeSpec {
            valid_children: vec![
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
            inline_content: false,
            block: true,
            leaf: false,
            code: false,
            atom: false,
            defining: false,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]),
        },
    );

    // CodeBlock: contains text only, no marks
    let mut code_attrs = HashMap::new();
    code_attrs.insert("language".to_string(), String::new());
    nodes.insert(
        NodeType::CodeBlock,
        NodeSpec {
            valid_children: vec![],
            inline_content: true,
            block: true,
            leaf: false,
            code: true,
            atom: false,
            defining: true,
            isolating: false,
            default_attrs: code_attrs,
            allowed_marks: Some(vec![]), // no marks in code
        },
    );

    // HorizontalRule: leaf block
    nodes.insert(
        NodeType::HorizontalRule,
        NodeSpec {
            valid_children: vec![],
            inline_content: false,
            block: true,
            leaf: true,
            code: false,
            atom: true,
            defining: false,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]),
        },
    );

    // HardBreak: leaf inline
    nodes.insert(
        NodeType::HardBreak,
        NodeSpec {
            valid_children: vec![],
            inline_content: false,
            block: false,
            leaf: true,
            code: false,
            atom: false,
            defining: false,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]),
        },
    );

    // Image: leaf block atom
    nodes.insert(
        NodeType::Image,
        NodeSpec {
            valid_children: vec![],
            inline_content: false,
            block: true,
            leaf: true,
            code: false,
            atom: true,
            defining: false,
            isolating: false,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]),
        },
    );

    // ── Mark specs ──

    let mut marks = HashMap::new();

    marks.insert(
        MarkType::Bold,
        MarkSpec {
            inclusive: true,
            exclude_all: false,
            excludes: vec![],
        },
    );

    marks.insert(
        MarkType::Italic,
        MarkSpec {
            inclusive: true,
            exclude_all: false,
            excludes: vec![],
        },
    );

    marks.insert(
        MarkType::Underline,
        MarkSpec {
            inclusive: true,
            exclude_all: false,
            excludes: vec![],
        },
    );

    marks.insert(
        MarkType::Strike,
        MarkSpec {
            inclusive: true,
            exclude_all: false,
            excludes: vec![],
        },
    );

    marks.insert(
        MarkType::Code,
        MarkSpec {
            inclusive: false,
            exclude_all: true,
            excludes: vec![],
        },
    );

    marks.insert(
        MarkType::Link,
        MarkSpec {
            inclusive: false,
            exclude_all: false,
            excludes: vec![],
        },
    );

    Schema { nodes, marks }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::Fragment;

    fn schema() -> Schema {
        default_schema()
    }

    // ── Content validation ──

    #[test]
    fn doc_accepts_paragraphs() {
        let s = schema();
        let p = Node::element(NodeType::Paragraph);
        assert!(s.content_matches(NodeType::Doc, &[&p]));
    }

    #[test]
    fn doc_accepts_headings() {
        let s = schema();
        let h = Node::element(NodeType::Heading);
        assert!(s.content_matches(NodeType::Doc, &[&h]));
    }

    #[test]
    fn doc_accepts_mixed_blocks() {
        let s = schema();
        let p = Node::element(NodeType::Paragraph);
        let hr = Node::element(NodeType::HorizontalRule);
        let bq = Node::element(NodeType::Blockquote);
        assert!(s.content_matches(NodeType::Doc, &[&p, &hr, &bq]));
    }

    #[test]
    fn doc_rejects_text() {
        let s = schema();
        let t = Node::text("hello");
        assert!(!s.content_matches(NodeType::Doc, &[&t]));
    }

    #[test]
    fn doc_rejects_list_item() {
        let s = schema();
        let li = Node::element(NodeType::ListItem);
        assert!(!s.content_matches(NodeType::Doc, &[&li]));
    }

    #[test]
    fn paragraph_accepts_text() {
        let s = schema();
        let t = Node::text("hello");
        assert!(s.content_matches(NodeType::Paragraph, &[&t]));
    }

    #[test]
    fn paragraph_accepts_hard_break() {
        let s = schema();
        let br = Node::element(NodeType::HardBreak);
        assert!(s.content_matches(NodeType::Paragraph, &[&br]));
    }

    #[test]
    fn paragraph_accepts_mixed_inline() {
        let s = schema();
        let t = Node::text("hello ");
        let br = Node::element(NodeType::HardBreak);
        let t2 = Node::text("world");
        assert!(s.content_matches(NodeType::Paragraph, &[&t, &br, &t2]));
    }

    #[test]
    fn paragraph_rejects_block() {
        let s = schema();
        let p2 = Node::element(NodeType::Paragraph);
        assert!(!s.content_matches(NodeType::Paragraph, &[&p2]));
    }

    #[test]
    fn heading_accepts_text() {
        let s = schema();
        let t = Node::text("Title");
        assert!(s.content_matches(NodeType::Heading, &[&t]));
    }

    #[test]
    fn heading_rejects_block() {
        let s = schema();
        let p = Node::element(NodeType::Paragraph);
        assert!(!s.content_matches(NodeType::Heading, &[&p]));
    }

    #[test]
    fn bullet_list_accepts_list_items() {
        let s = schema();
        let li = Node::element(NodeType::ListItem);
        assert!(s.content_matches(NodeType::BulletList, &[&li]));
    }

    #[test]
    fn bullet_list_rejects_paragraph() {
        let s = schema();
        let p = Node::element(NodeType::Paragraph);
        assert!(!s.content_matches(NodeType::BulletList, &[&p]));
    }

    #[test]
    fn list_item_accepts_paragraph() {
        let s = schema();
        let p = Node::element(NodeType::Paragraph);
        assert!(s.content_matches(NodeType::ListItem, &[&p]));
    }

    #[test]
    fn list_item_accepts_nested_list() {
        let s = schema();
        let p = Node::element(NodeType::Paragraph);
        let ul = Node::element(NodeType::BulletList);
        assert!(s.content_matches(NodeType::ListItem, &[&p, &ul]));
    }

    #[test]
    fn list_item_rejects_hr() {
        let s = schema();
        let hr = Node::element(NodeType::HorizontalRule);
        assert!(!s.content_matches(NodeType::ListItem, &[&hr]));
    }

    #[test]
    fn list_item_rejects_image() {
        let s = schema();
        let img = Node::element(NodeType::Image);
        assert!(!s.content_matches(NodeType::ListItem, &[&img]));
    }

    #[test]
    fn code_block_accepts_text() {
        let s = schema();
        let t = Node::text("fn main() {}");
        assert!(s.content_matches(NodeType::CodeBlock, &[&t]));
    }

    #[test]
    fn code_block_rejects_elements() {
        let s = schema();
        let br = Node::element(NodeType::HardBreak);
        assert!(!s.content_matches(NodeType::CodeBlock, &[&br]));
    }

    #[test]
    fn leaf_rejects_children() {
        let s = schema();
        let t = Node::text("oops");
        assert!(!s.content_matches(NodeType::HorizontalRule, &[&t]));
        assert!(s.content_matches(NodeType::HorizontalRule, &[]));
    }

    #[test]
    fn blockquote_accepts_blocks() {
        let s = schema();
        let p = Node::element(NodeType::Paragraph);
        let h = Node::element(NodeType::Heading);
        assert!(s.content_matches(NodeType::Blockquote, &[&p, &h]));
    }

    #[test]
    fn task_list_accepts_task_items() {
        let s = schema();
        let ti = Node::element(NodeType::TaskItem);
        assert!(s.content_matches(NodeType::TaskList, &[&ti]));
    }

    #[test]
    fn task_list_rejects_list_items() {
        let s = schema();
        let li = Node::element(NodeType::ListItem);
        assert!(!s.content_matches(NodeType::TaskList, &[&li]));
    }

    // ── Mark validation ──

    #[test]
    fn can_apply_bold_to_paragraph() {
        let s = schema();
        assert!(s.can_apply_mark(NodeType::Paragraph, MarkType::Bold));
    }

    #[test]
    fn cannot_apply_mark_to_code_block() {
        let s = schema();
        assert!(!s.can_apply_mark(NodeType::CodeBlock, MarkType::Bold));
        assert!(!s.can_apply_mark(NodeType::CodeBlock, MarkType::Italic));
    }

    #[test]
    fn code_mark_excludes_all_others() {
        let s = schema();
        let code = Mark::new(MarkType::Code);
        let bold = Mark::new(MarkType::Bold);
        assert!(!s.marks_compatible(&code, &bold));
        assert!(!s.marks_compatible(&bold, &code));
    }

    #[test]
    fn bold_and_italic_compatible() {
        let s = schema();
        let bold = Mark::new(MarkType::Bold);
        let italic = Mark::new(MarkType::Italic);
        assert!(s.marks_compatible(&bold, &italic));
    }

    #[test]
    fn marks_valid_bold_italic() {
        let s = schema();
        let marks = vec![Mark::new(MarkType::Bold), Mark::new(MarkType::Italic)];
        assert!(s.marks_valid(&marks));
    }

    #[test]
    fn marks_invalid_code_with_bold() {
        let s = schema();
        let marks = vec![Mark::new(MarkType::Code), Mark::new(MarkType::Bold)];
        assert!(!s.marks_valid(&marks));
    }

    #[test]
    fn marks_valid_code_alone() {
        let s = schema();
        let marks = vec![Mark::new(MarkType::Code)];
        assert!(s.marks_valid(&marks));
    }

    // ── Document validation ──

    #[test]
    fn validate_empty_doc() {
        let s = schema();
        let doc = Node::empty_doc();
        assert!(s.validate(&doc).is_ok());
    }

    #[test]
    fn validate_doc_with_content() {
        let s = schema();
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hello")]),
                ),
                Node::element(NodeType::HorizontalRule),
            ]),
        );
        assert!(s.validate(&doc).is_ok());
    }

    #[test]
    fn validate_rejects_text_in_doc() {
        let s = schema();
        let doc = Node::Element {
            node_type: NodeType::Doc,
            attrs: HashMap::new(),
            content: Fragment::from(vec![Node::text("bare text")]),
            marks: vec![],
        };
        assert!(s.validate(&doc).is_err());
    }

    #[test]
    fn validate_rejects_block_in_paragraph() {
        let s = schema();
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::Element {
                node_type: NodeType::Paragraph,
                attrs: HashMap::new(),
                content: Fragment::from(vec![Node::element(NodeType::Paragraph)]),
                marks: vec![],
            }]),
        );
        assert!(s.validate(&doc).is_err());
    }

    #[test]
    fn validate_nested_list() {
        let s = schema();
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![
                        Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("item")]),
                        ),
                        Node::element_with_content(
                            NodeType::BulletList,
                            Fragment::from(vec![Node::element_with_content(
                                NodeType::ListItem,
                                Fragment::from(vec![Node::element_with_content(
                                    NodeType::Paragraph,
                                    Fragment::from(vec![Node::text("nested")]),
                                )]),
                            )]),
                        ),
                    ]),
                )]),
            )]),
        );
        assert!(s.validate(&doc).is_ok());
    }

    #[test]
    fn validate_rejects_invalid_marks() {
        let s = schema();
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks(
                    "bad",
                    vec![Mark::new(MarkType::Code), Mark::new(MarkType::Bold)],
                )]),
            )]),
        );
        assert!(s.validate(&doc).is_err());
    }

    // ── Code block special case ──

    #[test]
    fn code_block_inline_content_text_only() {
        let s = schema();
        let t = Node::text("code");
        assert!(s.content_matches(NodeType::CodeBlock, &[&t]));
    }

    // ── Additional tests from review ──

    #[test]
    fn empty_bullet_list_rejected() {
        let s = schema();
        assert!(!s.content_matches(NodeType::BulletList, &[]));
    }

    #[test]
    fn empty_ordered_list_rejected() {
        let s = schema();
        assert!(!s.content_matches(NodeType::OrderedList, &[]));
    }

    #[test]
    fn empty_task_list_rejected() {
        let s = schema();
        assert!(!s.content_matches(NodeType::TaskList, &[]));
    }

    #[test]
    fn list_item_rejects_heading() {
        let s = schema();
        let h = Node::element(NodeType::Heading);
        assert!(!s.content_matches(NodeType::ListItem, &[&h]));
    }

    #[test]
    fn nested_blockquotes_valid() {
        let s = schema();
        let inner_bq = Node::element_with_content(
            NodeType::Blockquote,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("nested")]),
            )]),
        );
        let outer_bq = Node::element_with_content(
            NodeType::Blockquote,
            Fragment::from(vec![inner_bq.clone()]),
        );
        assert!(s.content_matches(NodeType::Blockquote, &[&inner_bq]));

        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![outer_bq]),
        );
        assert!(s.validate(&doc).is_ok());
    }

    #[test]
    fn two_code_marks_invalid() {
        let s = schema();
        let marks = vec![Mark::new(MarkType::Code), Mark::new(MarkType::Code)];
        // Two identical Code marks -- dedup would handle this upstream,
        // but marks_valid should still accept them (they're identical, compatible)
        assert!(s.marks_valid(&marks));
    }

    #[test]
    fn validate_empty_list_rejected() {
        let s = schema();
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::Element {
                node_type: NodeType::BulletList,
                attrs: HashMap::new(),
                content: Fragment::empty(),
                marks: vec![],
            }]),
        );
        assert!(s.validate(&doc).is_err());
    }

    #[test]
    fn task_item_empty_accepted() {
        // TaskItem can have zero children (unlike list containers).
        // In ProseMirror, content expression is "paragraph block*" (requires 1+),
        // but for the MVP we don't enforce minimum content in non-list containers.
        let s = schema();
        assert!(s.content_matches(NodeType::TaskItem, &[]));
    }

    #[test]
    fn can_apply_mark_unknown_node_returns_false() {
        // NodeType::Doc with mark Bold -- allowed_marks is Some(vec![])
        let s = schema();
        assert!(!s.can_apply_mark(NodeType::Doc, MarkType::Bold));
    }
}
