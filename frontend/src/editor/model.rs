use std::collections::HashMap;

/// Generate a random block ID (8 alphanumeric chars).
/// Uses Math.random in WASM, or a simple counter in tests.
pub fn generate_block_id() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let mut id = String::with_capacity(8);
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        for _ in 0..8 {
            let idx = (js_sys::Math::random() * CHARS.len() as f64) as usize;
            id.push(CHARS[idx % CHARS.len()] as char);
        }
        id
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        format!("blk{:05}", COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Mark types for inline formatting.
/// Ordered by canonical sort priority (lower = applied first/outermost).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MarkType {
    Link = 0,
    Bold = 1,
    Italic = 2,
    Underline = 3,
    Strike = 4,
    Code = 5,
    TextColor = 6,
    Highlight = 7,
}

impl MarkType {
    /// Whether this mark excludes all other marks (e.g., inline code).
    pub fn excludes_all(&self) -> bool {
        matches!(self, MarkType::Code)
    }
}

/// A mark applied to inline content (text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub mark_type: MarkType,
    pub attrs: HashMap<String, String>,
}

impl Mark {
    pub fn new(mark_type: MarkType) -> Self {
        Self {
            mark_type,
            attrs: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: &str, value: &str) -> Self {
        self.attrs.insert(key.to_string(), value.to_string());
        self
    }

    /// Compare for sorting: by type first, then by attrs for stability.
    fn sort_key(&self) -> (MarkType, Vec<(&String, &String)>) {
        let mut attr_pairs: Vec<_> = self.attrs.iter().collect();
        attr_pairs.sort();
        (self.mark_type, attr_pairs)
    }
}

// Ord/PartialOrd consistent with PartialEq: compare type AND attrs.
impl PartialOrd for Mark {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Mark {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

/// Sort marks into canonical order and remove full duplicates.
/// Marks with the same type but different attrs (e.g., two different links)
/// are NOT deduped -- they are kept as separate marks.
pub fn normalize_marks(marks: &mut Vec<Mark>) {
    marks.sort();
    marks.dedup(); // uses PartialEq, which compares type AND attrs
}

/// Check if a mark set is compatible (no exclusion violations).
pub fn marks_compatible(marks: &[Mark]) -> bool {
    if marks.iter().any(|m| m.mark_type.excludes_all()) && marks.len() > 1 {
        return false;
    }
    true
}

// ─── Node Types ─────────────────────────────────────────────────

/// Node types in the document.
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
    /// Whether this is a leaf node (no children).
    pub fn is_leaf(&self) -> bool {
        matches!(
            self,
            NodeType::HorizontalRule | NodeType::HardBreak | NodeType::Image
        )
    }

    /// Whether this is an inline node.
    /// Note: matches collab crate -- only HardBreak is inline.
    /// Image is a block-level leaf (atom) in our schema.
    pub fn is_inline(&self) -> bool {
        matches!(self, NodeType::HardBreak)
    }

    /// Whether this is a block node.
    pub fn is_block(&self) -> bool {
        !self.is_inline() && *self != NodeType::Doc
    }

    /// Whether content is treated as code (no marks allowed).
    pub fn is_code(&self) -> bool {
        matches!(self, NodeType::CodeBlock)
    }

    /// Whether this is an atom (not directly editable, selected as a unit).
    pub fn is_atom(&self) -> bool {
        matches!(self, NodeType::HorizontalRule | NodeType::Image)
    }

    /// Whether this node type is a commentable block (gets a blockId).
    pub fn is_commentable(&self) -> bool {
        matches!(
            self,
            NodeType::Paragraph
                | NodeType::Heading
                | NodeType::ListItem
                | NodeType::TaskItem
                | NodeType::CodeBlock
                | NodeType::Blockquote
        )
    }

    /// Whether this node type contains inline content (text).
    /// Paragraph, Heading, and CodeBlock hold text directly.
    /// Container blocks (lists, blockquote) hold other blocks, not text.
    pub fn is_textblock(&self) -> bool {
        matches!(
            self,
            NodeType::Paragraph | NodeType::Heading | NodeType::CodeBlock
        )
    }

    /// Default attributes for this node type.
    pub fn default_attrs(&self) -> HashMap<String, String> {
        match self {
            NodeType::Heading => {
                let mut m = HashMap::new();
                m.insert("level".to_string(), "1".to_string());
                m
            }
            NodeType::TaskItem => {
                let mut m = HashMap::new();
                m.insert("checked".to_string(), "false".to_string());
                m
            }
            NodeType::CodeBlock => {
                let mut m = HashMap::new();
                m.insert("language".to_string(), String::new());
                m
            }
            _ => HashMap::new(),
        }
    }
}

// ─── Char-index helpers ─────────────────────────────────────────

/// Get the char count of a string (NOT byte length).
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Slice a string by char indices (safe for all Unicode).
pub fn char_slice(s: &str, start: usize, end: usize) -> String {
    s.chars().skip(start).take(end - start).collect()
}

// ─── Node ───────────────────────────────────────────────────────

/// A document node. Either a text leaf or an element with children.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// Text content with inline marks.
    Text { text: String, marks: Vec<Mark> },
    /// An element node with a type, attributes, and children.
    Element {
        node_type: NodeType,
        attrs: HashMap<String, String>,
        content: Fragment,
        marks: Vec<Mark>,
    },
}

impl Node {
    /// Create a text node.
    pub fn text(text: &str) -> Self {
        Node::Text {
            text: text.to_string(),
            marks: vec![],
        }
    }

    /// Create a text node with marks.
    pub fn text_with_marks(text: &str, marks: Vec<Mark>) -> Self {
        let mut marks = marks;
        normalize_marks(&mut marks);
        Node::Text {
            text: text.to_string(),
            marks,
        }
    }

    /// Create an element node with no children and default attrs.
    pub fn element(node_type: NodeType) -> Self {
        let mut attrs = node_type.default_attrs();
        if node_type.is_commentable() {
            attrs.entry("blockId".to_string()).or_insert_with(generate_block_id);
        }
        Node::Element {
            node_type,
            attrs,
            content: Fragment::empty(),
            marks: vec![],
        }
    }

    /// Create an element node with children and default attrs.
    pub fn element_with_content(node_type: NodeType, content: Fragment) -> Self {
        let mut attrs = node_type.default_attrs();
        if node_type.is_commentable() {
            attrs.entry("blockId".to_string()).or_insert_with(generate_block_id);
        }
        Node::Element {
            node_type,
            attrs,
            content,
            marks: vec![],
        }
    }

    /// Create an element with explicit attributes (merged with defaults).
    pub fn element_with_attrs(
        node_type: NodeType,
        attrs: HashMap<String, String>,
        content: Fragment,
    ) -> Self {
        let mut merged = node_type.default_attrs();
        merged.extend(attrs);
        if node_type.is_commentable() {
            merged.entry("blockId".to_string()).or_insert_with(generate_block_id);
        }
        Node::Element {
            node_type,
            attrs: merged,
            content,
            marks: vec![],
        }
    }

    /// Get the block ID of this element node (if it has one).
    pub fn block_id(&self) -> Option<&str> {
        match self {
            Node::Element { attrs, .. } => attrs.get("blockId").map(|s| s.as_str()),
            _ => None,
        }
    }

    /// Find the block ID at a given model position in a document.
    /// Recursively walks the node tree to find the commentable block containing `pos`.
    pub fn block_id_at(&self, pos: usize) -> Option<String> {
        fn find_in_children(children: &[Node], pos: usize) -> Option<String> {
            let mut offset = 0;
            for child in children {
                let size = child.node_size();
                // Strict > on left: position at `offset` is the open boundary, not inside the child.
                if pos > offset && pos < offset + size {
                    if let Some(id) = child.block_id() {
                        return Some(id.to_string());
                    }
                    // Recurse into container elements (lists, blockquote, doc).
                    if let Node::Element { content, .. } = child {
                        let inner_pos = pos - offset - 1;
                        return find_in_children(&content.children, inner_pos);
                    }
                }
                offset += size;
            }
            None
        }

        let Node::Element { content, .. } = self else {
            return None;
        };
        // Doc-level: positions start at 0 (before content), first child content starts at 1.
        find_in_children(&content.children, pos)
    }

    /// Create a copy of this element with different content.
    pub fn copy_with_content(&self, content: Fragment) -> Self {
        match self {
            Node::Text { .. } => panic!("cannot copy_with_content on a text node"),
            Node::Element {
                node_type,
                attrs,
                marks,
                ..
            } => Node::Element {
                node_type: *node_type,
                attrs: attrs.clone(),
                content,
                marks: marks.clone(),
            },
        }
    }

    /// Get the node type (None for text nodes).
    pub fn node_type(&self) -> Option<NodeType> {
        match self {
            Node::Text { .. } => None,
            Node::Element { node_type, .. } => Some(*node_type),
        }
    }

    /// Whether this is a text node.
    pub fn is_text(&self) -> bool {
        matches!(self, Node::Text { .. })
    }

    /// Whether this is a leaf (text node or leaf element).
    pub fn is_leaf(&self) -> bool {
        match self {
            Node::Text { .. } => true,
            Node::Element { node_type, .. } => node_type.is_leaf(),
        }
    }

    /// Get the marks on this node.
    pub fn marks(&self) -> &[Mark] {
        match self {
            Node::Text { marks, .. } => marks,
            Node::Element { marks, .. } => marks,
        }
    }

    /// Get the text content of this node (recursively for elements).
    pub fn text_content(&self) -> String {
        match self {
            Node::Text { text, .. } => text.clone(),
            Node::Element { content, .. } => {
                content.children.iter().map(|c| c.text_content()).collect()
            }
        }
    }

    /// Get text content before a given model position within the same block.
    /// Returns None if position is out of bounds.
    pub fn text_before(&self, pos: usize) -> Option<String> {
        let Node::Element { content, .. } = self else {
            return None;
        };
        // Find which top-level block contains `pos`
        let mut offset = 0;
        for child in &content.children {
            let size = child.node_size();
            if pos >= offset && pos < offset + size {
                // pos is inside this child block
                let inner_pos = (pos - offset).saturating_sub(1);
                let full_text = child.text_content();
                let chars: Vec<char> = full_text.chars().collect();
                let clamped = inner_pos.min(chars.len());
                return Some(chars[..clamped].iter().collect());
            }
            offset += size;
        }
        None
    }

    /// The "size" of this node for position calculations.
    /// Uses char count for text (not byte length).
    /// - Text nodes: number of characters
    /// - Leaf elements (hr, br, image): 1
    /// - Non-leaf elements: content.size + 2 (open + close boundary)
    pub fn node_size(&self) -> usize {
        match self {
            Node::Text { text, .. } => char_len(text),
            Node::Element {
                node_type, content, ..
            } => {
                if node_type.is_leaf() {
                    1
                } else {
                    content.size() + 2
                }
            }
        }
    }

    /// The size of this node's content (0 for text/leaf nodes).
    pub fn content_size(&self) -> usize {
        match self {
            Node::Text { .. } => 0,
            Node::Element {
                node_type, content, ..
            } => {
                if node_type.is_leaf() {
                    0
                } else {
                    content.size()
                }
            }
        }
    }

    /// Get the child count (0 for text nodes).
    pub fn child_count(&self) -> usize {
        match self {
            Node::Text { .. } => 0,
            Node::Element { content, .. } => content.child_count(),
        }
    }

    /// Get a child by index.
    pub fn child(&self, index: usize) -> Option<&Node> {
        match self {
            Node::Text { .. } => None,
            Node::Element { content, .. } => content.child(index),
        }
    }

    /// Get the attributes (empty for text nodes).
    pub fn attrs(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::LazyLock<HashMap<String, String>> =
            std::sync::LazyLock::new(HashMap::new);
        match self {
            Node::Text { .. } => &EMPTY,
            Node::Element { attrs, .. } => attrs,
        }
    }

    /// Extract a Slice from a document between two positions.
    /// Positions are document-level (0-based within the doc content).
    pub fn slice(&self, from: usize, to: usize) -> Slice {
        let Node::Element { content, .. } = self else {
            return Slice::empty();
        };
        if from >= to {
            return Slice::empty();
        }
        let cut = content.cut(from, to);
        Slice::new(cut, 0, 0)
    }

    /// Create a simple document with a single empty paragraph.
    pub fn empty_doc() -> Self {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element(NodeType::Paragraph)]),
        )
    }
}

// ─── Fragment ───────────────────────────────────────────────────

/// An ordered sequence of child nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub children: Vec<Node>,
}

impl Fragment {
    pub fn empty() -> Self {
        Self { children: vec![] }
    }

    pub fn from(children: Vec<Node>) -> Self {
        let mut frag = Self { children };
        frag.normalize_text();
        frag
    }

    /// Total size of all children for position calculation.
    pub fn size(&self) -> usize {
        self.children.iter().map(|c| c.node_size()).sum()
    }

    /// Number of direct children.
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// Get a child by index.
    pub fn child(&self, index: usize) -> Option<&Node> {
        self.children.get(index)
    }

    /// Append a node, normalizing adjacent text.
    pub fn append(mut self, node: Node) -> Self {
        self.children.push(node);
        self.normalize_text();
        self
    }

    /// Append all children from another fragment.
    pub fn append_fragment(mut self, other: Fragment) -> Self {
        self.children.extend(other.children);
        self.normalize_text();
        self
    }

    /// Replace a child at the given index.
    pub fn replace_child(&self, index: usize, node: Node) -> Self {
        let mut children = self.children.clone();
        if index < children.len() {
            children[index] = node;
        }
        Fragment::from(children)
    }

    /// Cut a sub-fragment by position range (char-indexed).
    pub fn cut(&self, from: usize, to: usize) -> Self {
        if from >= to {
            return Fragment::empty();
        }

        let mut result = Vec::new();
        let mut pos = 0;

        for child in &self.children {
            let child_size = child.node_size();
            let child_end = pos + child_size;

            if child_end <= from {
                pos = child_end;
                continue;
            }
            if pos >= to {
                break;
            }

            match child {
                Node::Text { text, marks } => {
                    let start = if from > pos { from - pos } else { 0 };
                    let end = if to < child_end {
                        to - pos
                    } else {
                        char_len(text)
                    };
                    if start < end {
                        result.push(Node::Text {
                            text: char_slice(text, start, end),
                            marks: marks.clone(),
                        });
                    }
                }
                Node::Element {
                    node_type,
                    attrs,
                    content,
                    marks,
                } => {
                    if node_type.is_leaf() {
                        result.push(child.clone());
                    } else {
                        let inner_from = if from > pos + 1 { from - pos - 1 } else { 0 };
                        let inner_to = if to < child_end {
                            to - pos - 1
                        } else {
                            content.size()
                        };

                        if from <= pos && to >= child_end {
                            result.push(child.clone());
                        } else {
                            let cut_content = content.cut(inner_from, inner_to);
                            result.push(Node::Element {
                                node_type: *node_type,
                                attrs: attrs.clone(),
                                content: cut_content,
                                marks: marks.clone(),
                            });
                        }
                    }
                }
            }

            pos = child_end;
        }

        // Normalize the result (fixes adjacent text merging after cut)
        Fragment::from(result)
    }

    /// Merge adjacent text nodes with identical marks.
    fn normalize_text(&mut self) {
        let mut i = 0;
        while i + 1 < self.children.len() {
            if let (Node::Text { marks: m1, .. }, Node::Text { marks: m2, .. }) =
                (&self.children[i], &self.children[i + 1])
            {
                if m1 == m2 {
                    let next = self.children.remove(i + 1);
                    if let (Node::Text { text, .. }, Node::Text { text: next_text, .. }) =
                        (&mut self.children[i], next)
                    {
                        text.push_str(&next_text);
                    }
                    continue;
                }
            }
            i += 1;
        }
    }
}

// ─── Slice ──────────────────────────────────────────────────────

/// A piece of a document, used for clipboard and replace operations.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub content: Fragment,
    pub open_start: usize,
    pub open_end: usize,
}

impl Slice {
    pub fn new(content: Fragment, open_start: usize, open_end: usize) -> Self {
        Self {
            content,
            open_start,
            open_end,
        }
    }

    pub fn empty() -> Self {
        Self {
            content: Fragment::empty(),
            open_start: 0,
            open_end: 0,
        }
    }

    pub fn size(&self) -> usize {
        self.content.size()
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Text and Mark basics ──

    #[test]
    fn text_node_creation() {
        let node = Node::text("hello");
        assert!(node.is_text());
        assert!(node.is_leaf());
        assert_eq!(node.text_content(), "hello");
        assert_eq!(node.node_size(), 5);
        assert!(node.marks().is_empty());
    }

    #[test]
    fn text_node_with_marks() {
        let node = Node::text_with_marks("bold", vec![Mark::new(MarkType::Bold)]);
        assert_eq!(node.marks().len(), 1);
        assert_eq!(node.marks()[0].mark_type, MarkType::Bold);
    }

    #[test]
    fn text_node_unicode() {
        let node = Node::text("héllo 🌍");
        // 8 chars: h, é, l, l, o, space, 🌍 = 7? No:
        // h(1) é(1) l(1) l(1) o(1) ' '(1) 🌍(1) = 7 chars
        assert_eq!(node.node_size(), 7);
        assert_eq!(node.text_content(), "héllo 🌍");
    }

    #[test]
    fn text_node_emoji_size() {
        let node = Node::text("👨‍👩‍👧‍👦");
        // This is a ZWJ sequence: 7 chars (4 emoji + 3 ZWJ)
        let expected = "👨‍👩‍👧‍👦".chars().count();
        assert_eq!(node.node_size(), expected);
    }

    // ── Element basics ──

    #[test]
    fn element_node_creation() {
        let node = Node::element(NodeType::Paragraph);
        assert!(!node.is_text());
        assert!(!node.is_leaf());
        assert_eq!(node.node_type(), Some(NodeType::Paragraph));
        assert_eq!(node.node_size(), 2);
    }

    #[test]
    fn element_with_text_content() {
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("hello")]),
        );
        assert_eq!(para.text_content(), "hello");
        assert_eq!(para.node_size(), 7);
        assert_eq!(para.content_size(), 5);
        assert_eq!(para.child_count(), 1);
    }

    #[test]
    fn leaf_element_size() {
        let hr = Node::element(NodeType::HorizontalRule);
        assert!(hr.is_leaf());
        assert_eq!(hr.node_size(), 1);
        assert_eq!(hr.content_size(), 0);
    }

    #[test]
    fn doc_node_size() {
        let doc = Node::empty_doc();
        // doc: 2 (boundaries) + paragraph: 2 (boundaries) = 4
        assert_eq!(doc.node_size(), 4);
    }

    // ── Default attrs ──

    #[test]
    fn task_item_has_default_checked() {
        let task = Node::element(NodeType::TaskItem);
        assert_eq!(task.attrs().get("checked").unwrap(), "false");
    }

    #[test]
    fn heading_has_default_level() {
        let h = Node::element(NodeType::Heading);
        assert_eq!(h.attrs().get("level").unwrap(), "1");
    }

    #[test]
    fn element_with_attrs_merges_defaults() {
        let mut attrs = HashMap::new();
        attrs.insert("level".to_string(), "3".to_string());
        let h3 = Node::element_with_attrs(NodeType::Heading, attrs, Fragment::empty());
        assert_eq!(h3.attrs().get("level").unwrap(), "3");
    }

    // ── copy_with_content ──

    #[test]
    fn copy_with_content_preserves_attrs() {
        let mut attrs = HashMap::new();
        attrs.insert("level".to_string(), "2".to_string());
        let h2 = Node::element_with_attrs(
            NodeType::Heading,
            attrs,
            Fragment::from(vec![Node::text("old")]),
        );
        let h2_new = h2.copy_with_content(Fragment::from(vec![Node::text("new")]));
        assert_eq!(h2_new.attrs().get("level").unwrap(), "2");
        assert_eq!(h2_new.text_content(), "new");
    }

    // ── Mark ordering and dedup ──

    #[test]
    fn mark_sorting() {
        let mut marks = vec![
            Mark::new(MarkType::Code),
            Mark::new(MarkType::Bold),
            Mark::new(MarkType::Link),
        ];
        normalize_marks(&mut marks);
        assert_eq!(marks[0].mark_type, MarkType::Link);
        assert_eq!(marks[1].mark_type, MarkType::Bold);
        assert_eq!(marks[2].mark_type, MarkType::Code);
    }

    #[test]
    fn mark_dedup_identical() {
        let mut marks = vec![
            Mark::new(MarkType::Bold),
            Mark::new(MarkType::Bold),
            Mark::new(MarkType::Italic),
        ];
        normalize_marks(&mut marks);
        assert_eq!(marks.len(), 2);
    }

    #[test]
    fn mark_dedup_preserves_different_attrs() {
        let mut marks = vec![
            Mark::new(MarkType::Link).with_attr("href", "https://a.com"),
            Mark::new(MarkType::Link).with_attr("href", "https://b.com"),
        ];
        normalize_marks(&mut marks);
        // Two links with different hrefs should both be kept
        assert_eq!(marks.len(), 2);
    }

    #[test]
    fn mark_with_attrs() {
        let link = Mark::new(MarkType::Link)
            .with_attr("href", "https://example.com")
            .with_attr("target", "_blank");
        assert_eq!(link.attrs.get("href").unwrap(), "https://example.com");
    }

    #[test]
    fn mark_ord_consistent_with_eq() {
        let a = Mark::new(MarkType::Link).with_attr("href", "https://a.com");
        let b = Mark::new(MarkType::Link).with_attr("href", "https://b.com");
        // Different attrs -> not equal
        assert_ne!(a, b);
        // Ord should also distinguish them (not report Equal)
        assert_ne!(a.cmp(&b), std::cmp::Ordering::Equal);
    }

    // ── Mark compatibility ──

    #[test]
    fn marks_compatible_basic() {
        let marks = vec![Mark::new(MarkType::Bold), Mark::new(MarkType::Italic)];
        assert!(marks_compatible(&marks));
    }

    #[test]
    fn marks_incompatible_code_with_others() {
        let marks = vec![Mark::new(MarkType::Code), Mark::new(MarkType::Bold)];
        assert!(!marks_compatible(&marks));
    }

    #[test]
    fn marks_compatible_code_alone() {
        let marks = vec![Mark::new(MarkType::Code)];
        assert!(marks_compatible(&marks));
    }

    // ── Fragment operations ──

    #[test]
    fn fragment_size() {
        let frag = Fragment::from(vec![Node::text("hello"), Node::text(" world")]);
        assert_eq!(frag.child_count(), 1); // merged
        assert_eq!(frag.size(), 11);
    }

    #[test]
    fn fragment_no_merge_different_marks() {
        let frag = Fragment::from(vec![
            Node::text("normal"),
            Node::text_with_marks("bold", vec![Mark::new(MarkType::Bold)]),
        ]);
        assert_eq!(frag.child_count(), 2);
        assert_eq!(frag.size(), 10);
    }

    #[test]
    fn fragment_merge_same_marks() {
        let frag = Fragment::from(vec![
            Node::text_with_marks("hello ", vec![Mark::new(MarkType::Bold)]),
            Node::text_with_marks("world", vec![Mark::new(MarkType::Bold)]),
        ]);
        assert_eq!(frag.child_count(), 1);
        assert_eq!(frag.size(), 11);
        assert_eq!(frag.child(0).unwrap().text_content(), "hello world");
    }

    #[test]
    fn fragment_append_merges() {
        let frag = Fragment::from(vec![Node::text("hello")]).append(Node::text(" world"));
        assert_eq!(frag.child_count(), 1);
        assert_eq!(frag.child(0).unwrap().text_content(), "hello world");
    }

    #[test]
    fn fragment_append_no_merge_different_marks() {
        let frag = Fragment::from(vec![Node::text("hello")])
            .append(Node::text_with_marks(" world", vec![Mark::new(MarkType::Bold)]));
        assert_eq!(frag.child_count(), 2);
    }

    #[test]
    fn fragment_append_fragment_merges() {
        let a = Fragment::from(vec![Node::text("hello")]);
        let b = Fragment::from(vec![Node::text(" world")]);
        let merged = a.append_fragment(b);
        assert_eq!(merged.child_count(), 1);
        assert_eq!(merged.child(0).unwrap().text_content(), "hello world");
    }

    #[test]
    fn fragment_replace_child() {
        let frag = Fragment::from(vec![Node::text("aaa"), Node::text_with_marks("bbb", vec![Mark::new(MarkType::Bold)])]);
        let replaced = frag.replace_child(0, Node::text("ccc"));
        assert_eq!(replaced.child(0).unwrap().text_content(), "ccc");
        assert_eq!(replaced.child_count(), 2);
    }

    #[test]
    fn normalize_text_idempotent() {
        let frag = Fragment::from(vec![Node::text("a"), Node::text("b"), Node::text("c")]);
        assert_eq!(frag.child_count(), 1);
        assert_eq!(frag.child(0).unwrap().text_content(), "abc");
        let frag2 = Fragment::from(frag.children);
        assert_eq!(frag2.child_count(), 1);
        assert_eq!(frag2.child(0).unwrap().text_content(), "abc");
    }

    // ── Fragment.cut ──

    #[test]
    fn fragment_cut_text() {
        let frag = Fragment::from(vec![Node::text("hello world")]);
        let cut = frag.cut(0, 5);
        assert_eq!(cut.child_count(), 1);
        assert_eq!(cut.child(0).unwrap().text_content(), "hello");
    }

    #[test]
    fn fragment_cut_across_text_nodes() {
        let frag = Fragment::from(vec![
            Node::text("aaa"),
            Node::text_with_marks("bbb", vec![Mark::new(MarkType::Bold)]),
        ]);
        let cut = frag.cut(2, 5);
        assert_eq!(cut.size(), 3);
    }

    #[test]
    fn fragment_cut_unicode() {
        let frag = Fragment::from(vec![Node::text("héllo 🌍!")]);
        // h(0) é(1) l(2) l(3) o(4) ' '(5) 🌍(6) !(7)
        let cut = frag.cut(1, 6); // "éllo "
        assert_eq!(cut.child(0).unwrap().text_content(), "éllo ");
        assert_eq!(cut.size(), 5);
    }

    #[test]
    fn fragment_cut_emoji() {
        let frag = Fragment::from(vec![Node::text("a🌍b")]);
        // a(0) 🌍(1) b(2)
        let cut = frag.cut(0, 2); // "a🌍"
        assert_eq!(cut.child(0).unwrap().text_content(), "a🌍");
    }

    #[test]
    fn fragment_cut_partial_element() {
        // A paragraph containing "hello world"
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("hello world")]),
        );
        let frag = Fragment::from(vec![para]);
        // para: pos 0 (open), content 1..12, pos 12 (close) -> size 13
        // Cut positions 1..6 = inside the paragraph, "hello"
        let cut = frag.cut(1, 6);
        // Should produce a paragraph with "hello"
        assert_eq!(cut.child_count(), 1);
        let cut_para = cut.child(0).unwrap();
        assert_eq!(cut_para.node_type(), Some(NodeType::Paragraph));
        assert_eq!(cut_para.text_content(), "hello");
    }

    #[test]
    fn fragment_cut_produces_normalized_output() {
        // Two bold text nodes that will be adjacent after cut
        let frag = Fragment::from(vec![
            Node::text_with_marks("aaa", vec![Mark::new(MarkType::Bold)]),
            Node::text("xxx"),
            Node::text_with_marks("bbb", vec![Mark::new(MarkType::Bold)]),
        ]);
        // Cut to remove the middle "xxx" -- should merge the two bold runs
        // positions: aaa(0-3) xxx(3-6) bbb(6-9)
        // This specific cut won't merge because it produces [a, bbb] with different mark sets
        // Let's test a simpler case: cut all three
        let cut = frag.cut(0, 9);
        assert_eq!(cut.size(), 9);
        // Should be normalized (no adjacent text with same marks)
        // "aaa" bold, "xxx" plain, "bbb" bold -- these can't merge, correct
        assert_eq!(cut.child_count(), 3);
    }

    // ── Node type classification ──

    #[test]
    fn node_type_classification() {
        assert!(NodeType::HorizontalRule.is_leaf());
        assert!(NodeType::HardBreak.is_leaf());
        assert!(NodeType::Image.is_leaf());
        assert!(!NodeType::Paragraph.is_leaf());

        assert!(NodeType::Paragraph.is_block());
        assert!(NodeType::Heading.is_block());
        assert!(NodeType::Image.is_block()); // Image is a block-level atom
        assert!(!NodeType::Doc.is_block());

        assert!(NodeType::HardBreak.is_inline());
        assert!(!NodeType::Paragraph.is_inline());
        assert!(!NodeType::Image.is_inline()); // Image is NOT inline

        assert!(NodeType::CodeBlock.is_code());
        assert!(!NodeType::Paragraph.is_code());

        assert!(NodeType::HorizontalRule.is_atom());
        assert!(NodeType::Image.is_atom());
        assert!(!NodeType::Paragraph.is_atom());
    }

    // ── Document construction ──

    #[test]
    fn empty_doc() {
        let doc = Node::empty_doc();
        assert_eq!(doc.node_type(), Some(NodeType::Doc));
        assert_eq!(doc.child_count(), 1);
        assert_eq!(
            doc.child(0).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
    }

    #[test]
    fn doc_with_content() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hello")]),
                ),
                Node::element(NodeType::HorizontalRule),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("World")]),
                ),
            ]),
        );
        assert_eq!(doc.node_size(), 17);
        assert_eq!(doc.child_count(), 3);
        assert_eq!(doc.text_content(), "HelloWorld");
    }

    #[test]
    fn heading_with_level_attr() {
        let mut attrs = HashMap::new();
        attrs.insert("level".to_string(), "2".to_string());
        let h2 = Node::element_with_attrs(
            NodeType::Heading,
            attrs,
            Fragment::from(vec![Node::text("Title")]),
        );
        assert_eq!(h2.attrs().get("level").unwrap(), "2");
        assert_eq!(h2.text_content(), "Title");
    }

    #[test]
    fn nested_list() {
        let inner_list = Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("nested")]),
                )]),
            )]),
        );
        let outer_item = Node::element_with_content(
            NodeType::ListItem,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("parent")]),
                ),
                inner_list,
            ]),
        );
        let list = Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![outer_item]),
        );
        assert_eq!(list.text_content(), "parentnested");
        assert!(list.node_size() > 0);
    }

    // ── Slice ──

    #[test]
    fn slice_creation() {
        let slice = Slice::new(Fragment::from(vec![Node::text("hello")]), 0, 0);
        assert_eq!(slice.size(), 5);
    }

    #[test]
    fn slice_empty() {
        let slice = Slice::empty();
        assert_eq!(slice.size(), 0);
        assert_eq!(slice.open_start, 0);
        assert_eq!(slice.open_end, 0);
    }

    // ── Node::slice ──

    #[test]
    fn node_slice_within_block() {
        // Doc > Paragraph("Hello world")
        // Positions: Para(0..13), content 1..12, "Hello world" at 1..12
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        );
        // Select "lo wo" (positions 4..9)
        let slice = doc.slice(4, 9);
        assert!(!slice.content.children.is_empty());
        // Should contain partial paragraph with "lo wo"
        let text: String = slice.content.children.iter().map(|c| c.text_content()).collect();
        assert!(text.contains("lo wo"), "Expected 'lo wo', got: {text}");
    }

    #[test]
    fn node_slice_cross_block() {
        // Doc > [Paragraph("Hello"), Paragraph("World")]
        let doc = Node::element_with_content(
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
        );
        // Select from "llo" to "Wor" (positions 3..10)
        let slice = doc.slice(3, 10);
        assert_eq!(slice.content.children.len(), 2);
    }

    #[test]
    fn node_slice_with_marks() {
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
        // Select "Hello" (positions 1..6)
        let slice = doc.slice(1, 6);
        let text: String = slice.content.children.iter().map(|c| c.text_content()).collect();
        assert!(text.contains("Hello"), "Expected 'Hello', got: {text}");
    }

    #[test]
    fn node_slice_empty_range() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let slice = doc.slice(3, 3);
        assert!(slice.content.children.is_empty());
    }
}
