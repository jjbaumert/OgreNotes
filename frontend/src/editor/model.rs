// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::collections::HashMap;

/// Generate a random block ID (10 alphanumeric chars).
pub fn generate_block_id() -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut id = String::with_capacity(10);

    #[cfg(target_arch = "wasm32")]
    {
        for _ in 0..10 {
            let idx = (js_sys::Math::random() * CHARS.len() as f64) as usize;
            id.push(CHARS[idx % CHARS.len()] as char);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seed = RandomState::new().build_hasher().finish()
            .wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed));
        let mut state = seed;
        for _ in 0..10 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            id.push(CHARS[(state >> 33) as usize % CHARS.len()] as char);
        }
    }

    id
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
    // #143: appended (8/9) so existing discriminants — and any wire encoding
    // that relies on them — are unchanged. They sort innermost, which is the
    // right nesting for sub/superscript.
    Subscript = 8,
    Superscript = 9,
    // #148: user @-mention. Renders as a mention chip (see the
    // `.mention` CSS class); the `user_id` attribute carries the
    // mentioned user's id. Discriminant appended so no existing
    // wire encoding shifts.
    Mention = 10,
}

impl MarkType {
    /// #143: the mutually-exclusive partner, if any. A character can't be both
    /// subscript and superscript, so applying one strips the other.
    pub fn exclusive_partner(&self) -> Option<MarkType> {
        match self {
            MarkType::Subscript => Some(MarkType::Superscript),
            MarkType::Superscript => Some(MarkType::Subscript),
            _ => None,
        }
    }
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
    Table,
    TableRow,
    TableCell,
    TableHeader,
    /// Phase 5 M-P6 — sandboxed third-party embed (YouTube / Vimeo
    /// / Figma / Loom / CodeSandbox / generic per-workspace
    /// allowlisted host). Block-level leaf atom; the renderer
    /// emits a sandboxed iframe. Mirrors the same-named variant
    /// in `crates/collab/src/schema.rs` — both must list it in
    /// Doc.valid_children for the cross-schema test.
    Embed,
    /// #136 — inline calendar block; container of `CalendarEvent`
    /// children. See `design/live-app-blocks.md`. Mirrors the same-
    /// named variant in `crates/collab/src/schema.rs`.
    Calendar,
    /// #136 — one event inside a `Calendar`. Block-level leaf atom
    /// carrying `color`, `allDay`, date/timestamp pair, and
    /// `content`. See `design/live-app-blocks.md`.
    CalendarEvent,
    /// #137 — Kanban board. Container of columns. See
    /// `design/live-app-blocks.md`.
    Kanban,
    /// #137 — one column inside a `Kanban` container of cards.
    KanbanColumn,
    /// #137 — one card inside a `KanbanColumn`. Leaf.
    KanbanCard,
    /// #148 slice 6 — inline user @-mention leaf atom. Replaces
    /// the pre-existing text + `MarkType::Mention` shape.
    /// Attributes: `user_id` (opaque server-side id) and
    /// `display` (rendered name snapshot at insert time). Inline
    /// atom; deletes as one keystroke. Mirrors the same-named
    /// variant in `crates/collab/src/schema.rs`.
    Mention,
    /// Mermaid diagram block. Block-level leaf atom; source stored in
    /// the `source` attribute; rendered to SVG by `ogrenotes-mermaid`.
    /// Mirrors the same-named variant in `crates/collab/src/schema.rs`.
    Mermaid,
}

impl NodeType {
    /// Whether this is a leaf node (no children).
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
                | NodeType::Mermaid
        )
    }

    /// Whether this is an inline node. Mention is inline (lives
    /// inside a Paragraph's text stream, next to characters);
    /// Image is block-level (sits between paragraphs).
    pub fn is_inline(&self) -> bool {
        matches!(self, NodeType::HardBreak | NodeType::Mention)
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
        matches!(
            self,
            NodeType::HorizontalRule
                | NodeType::Image
                | NodeType::Embed
                | NodeType::Calendar
                | NodeType::CalendarEvent
                | NodeType::Kanban
                | NodeType::KanbanCard
                | NodeType::Mention
                | NodeType::Mermaid
        )
    }

    /// Whether this node type is a commentable block (the user
    /// can attach a comment thread to it).
    ///
    /// **Exhaustive on `NodeType`** — adding a variant to the enum
    /// is a compile error here until you decide. Distinct from
    /// `needs_block_id` below: that's structural identity for the
    /// yrs-bridge; this is "the comments UI lets users attach a
    /// thread here." Conflating the two cost a multi-hour
    /// debugging session (commit `d92dac4`) — every container needs
    /// structural identity, only a subset of nodes is a useful
    /// comment target. Keep them separate.
    pub fn is_commentable(&self) -> bool {
        match self {
            NodeType::Paragraph
            | NodeType::Heading
            | NodeType::ListItem
            | NodeType::TaskItem
            | NodeType::CodeBlock
            | NodeType::Blockquote
            | NodeType::Table
            | NodeType::TableCell
            | NodeType::TableHeader
            | NodeType::Calendar
            | NodeType::Kanban
            | NodeType::KanbanCard => true,
            NodeType::Doc
            | NodeType::BulletList
            | NodeType::OrderedList
            | NodeType::TaskList
            | NodeType::TableRow
            | NodeType::HorizontalRule
            | NodeType::HardBreak
            | NodeType::Image
            | NodeType::Embed
            | NodeType::CalendarEvent
            | NodeType::KanbanColumn
            | NodeType::Mention
            | NodeType::Mermaid => false,
        }
    }

    /// Whether this node type carries a stable structural blockId
    /// used by the yrs-bridge's `find_match` to align the editor
    /// model against the yrs CRDT state on every transaction.
    ///
    /// **Exhaustive on `NodeType`** — adding a variant is a compile
    /// error here. The right answer for any new Element type is
    /// almost certainly `true`; only `Doc` opts out because it's
    /// the singleton root, never matched by the bridge, and giving
    /// it an id makes equality across independently-constructed
    /// Docs fail spuriously.
    ///
    /// Pre-d92dac4 this rule was implicit (rode along on
    /// `is_commentable`) and four container types — BulletList,
    /// OrderedList, TaskList, TableRow — fell through the gap,
    /// triggering a `find_match` miss on every keystroke and a
    /// ~60 KB-per-update pathology that broke persistence. Make
    /// the rule exhaustive so the next added NodeType cannot
    /// repeat the failure.
    pub fn needs_block_id(&self) -> bool {
        match self {
            NodeType::Doc => false,
            NodeType::Paragraph
            | NodeType::Heading
            | NodeType::BulletList
            | NodeType::OrderedList
            | NodeType::ListItem
            | NodeType::TaskList
            | NodeType::TaskItem
            | NodeType::Blockquote
            | NodeType::CodeBlock
            | NodeType::Table
            | NodeType::TableRow
            | NodeType::TableCell
            | NodeType::TableHeader
            | NodeType::HorizontalRule
            | NodeType::HardBreak
            | NodeType::Image
            | NodeType::Embed
            | NodeType::Calendar
            | NodeType::CalendarEvent
            | NodeType::Kanban
            | NodeType::KanbanColumn
            | NodeType::KanbanCard
            | NodeType::Mention
            | NodeType::Mermaid => true,
        }
    }

    /// Whether this node type contains inline content (text).
    /// Paragraph, Heading, and CodeBlock hold text directly.
    /// Container blocks (lists, blockquote, table) hold other blocks, not text.
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
            NodeType::TableCell | NodeType::TableHeader => {
                let mut m = HashMap::new();
                m.insert("colspan".to_string(), "1".to_string());
                m.insert("rowspan".to_string(), "1".to_string());
                m
            }
            // #136 — cursor + timezone are filled in by the insert
            // handler (which knows today's date and the user's TZ);
            // view defaults to month.
            NodeType::Calendar => {
                let mut m = HashMap::new();
                m.insert("view".to_string(), "month".to_string());
                m.insert("timezone".to_string(), "UTC".to_string());
                m
            }
            NodeType::CalendarEvent => {
                let mut m = HashMap::new();
                m.insert("color".to_string(), "blue".to_string());
                m.insert("allDay".to_string(), "true".to_string());
                m.insert("content".to_string(), String::new());
                m
            }
            NodeType::KanbanColumn => {
                let mut m = HashMap::new();
                m.insert("title".to_string(), "New column".to_string());
                m
            }
            NodeType::KanbanCard => {
                let mut m = HashMap::new();
                m.insert("title".to_string(), "New card".to_string());
                m.insert("content".to_string(), String::new());
                m.insert("color".to_string(), "blue".to_string());
                m
            }
            NodeType::Mention => {
                let mut m = HashMap::new();
                m.insert("user_id".to_string(), String::new());
                m.insert("display".to_string(), String::new());
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
    ///
    /// Every Element except the top-level `Doc` gets a blockId.
    /// The bridge's `find_match` (`yrs_bridge.rs::find_match`)
    /// matches model nodes to yrs blocks by blockId, and without a
    /// stable identity on non-commentable containers (BulletList,
    /// TableRow, etc.) every edit re-inserted them from scratch,
    /// producing ~60 KB yrs updates per keystroke and a 56% match
    /// rate on real docs. `is_commentable` remains the gate for
    /// comment-UI rendering; this attribute is structural identity
    /// only.
    ///
    /// Doc is excluded because it's the singleton root — it has no
    /// parent to disambiguate against, the bridge never matches it,
    /// and giving it a blockId makes equality across independently-
    /// constructed Docs fail spuriously (see the
    /// `read_doc_from_ydoc` round-trip and concurrent-edit tests).
    pub fn element(node_type: NodeType) -> Self {
        let mut attrs = node_type.default_attrs();
        if node_type.needs_block_id() {
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
    /// See `Node::element` for the blockId rationale.
    pub fn element_with_content(node_type: NodeType, content: Fragment) -> Self {
        let mut attrs = node_type.default_attrs();
        if node_type.needs_block_id() {
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
    /// See `Node::element` for the blockId rationale.
    pub fn element_with_attrs(
        node_type: NodeType,
        attrs: HashMap<String, String>,
        content: Fragment,
    ) -> Self {
        let mut merged = node_type.default_attrs();
        merged.extend(attrs);
        if node_type.needs_block_id() {
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
                    // Descend FIRST and prefer the innermost (most specific)
                    // block id — e.g. a table cell's paragraph rather than
                    // the table itself, so a comment anchors to the cell, not
                    // the whole table (#116). This mirrors `find_block_at`,
                    // which the comment-anchor offsets are already computed
                    // against; before this, the two disagreed inside tables.
                    if let Node::Element { content, .. } = child {
                        let inner_pos = pos - offset - 1;
                        if let Some(inner) = find_in_children(&content.children, inner_pos) {
                            return Some(inner);
                        }
                    }
                    // No deeper block id — this child is the innermost block
                    // (or a non-id leaf, in which case there's nothing here).
                    return child.block_id().map(|id| id.to_string());
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

    /// Find a block by its `blockId` attribute and return its content_start position.
    /// Walks the document tree. Returns `None` if not found.
    pub fn find_block_content_start(&self, block_id: &str) -> Option<usize> {
        fn walk(children: &[Node], block_id: &str, mut offset: usize) -> Option<usize> {
            for child in children {
                let child_size = child.node_size();
                if let Node::Element { content, .. } = child {
                    let content_start = offset + 1;
                    if child.block_id() == Some(block_id) {
                        return Some(content_start);
                    }
                    // Recurse into containers
                    if let Some(found) = walk(&content.children, block_id, content_start) {
                        return Some(found);
                    }
                }
                offset += child_size;
            }
            None
        }

        let Node::Element { content, .. } = self else {
            return None;
        };
        walk(&content.children, block_id, 0)
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
    ///
    /// #148 slice 6: a `NodeType::Mention` leaf contributes its
    /// `display` attribute so downstream text extractors (LLM
    /// prompts, plain-text export, search index) see "@alice"
    /// rather than a silent gap. This matches the pre-existing
    /// text+MarkType::Mention output byte-for-byte.
    pub fn text_content(&self) -> String {
        match self {
            Node::Text { text, .. } => text.clone(),
            Node::Element {
                node_type: NodeType::Mention,
                attrs,
                content: _,
                marks: _,
            } => attrs.get("display").cloned().unwrap_or_default(),
            Node::Element { content, .. } => {
                content.children.iter().map(|c| c.text_content()).collect()
            }
        }
    }

    /// Compute a hash of the full document structure including node types, marks, and attributes.
    /// Used for change detection in collab sync (unlike text_content which ignores formatting).
    ///
    /// Attribute hashing is order-independent (keys are sorted first) so a
    /// freshly-decoded copy of a logically-identical document hashes the same.
    /// `attrs` is a `HashMap`, whose iteration order differs between independently
    /// constructed instances; feeding pairs in raw iteration order made the hash
    /// unstable across the CRDT decode round-trip for any node carrying ≥2 attrs
    /// (calendar/kanban blocks, multi-attr marks). That defeated the collab
    /// send-effect's `hash == prev_hash` change guard, so every echoed frame
    /// looked like a fresh edit and re-sent — an infinite send→echo→apply loop
    /// that also spammed `history.replaceState`.
    pub fn structural_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash_structure(&mut hasher);
        hasher.finish()
    }

    fn hash_structure(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        // Hash an attribute map order-independently: sort by key so the
        // digest is identical regardless of `HashMap` iteration order,
        // which varies between independently-constructed instances.
        fn hash_attrs_sorted(
            attrs: &HashMap<String, String>,
            hasher: &mut impl std::hash::Hasher,
        ) {
            use std::hash::Hash;
            attrs.len().hash(hasher);
            let mut entries: Vec<(&String, &String)> = attrs.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (k, v) in entries {
                k.hash(hasher);
                v.hash(hasher);
            }
        }
        match self {
            Node::Text { text, marks } => {
                0u8.hash(hasher); // tag for Text
                text.hash(hasher);
                marks.len().hash(hasher);
                for mark in marks {
                    (mark.mark_type as u8).hash(hasher);
                    hash_attrs_sorted(&mark.attrs, hasher);
                }
            }
            Node::Element { node_type, attrs, content, marks } => {
                1u8.hash(hasher); // tag for Element
                (*node_type as u8).hash(hasher);
                hash_attrs_sorted(attrs, hasher);
                marks.len().hash(hasher);
                for mark in marks {
                    (mark.mark_type as u8).hash(hasher);
                }
                content.children.len().hash(hasher);
                for child in &content.children {
                    child.hash_structure(hasher);
                }
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
        let size = self.size();
        let from = from.min(size);
        let to = to.min(size);
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

// ─── Document Normalization ────────────────────────────────────

/// Normalize a document tree to fix common structural violations:
/// - Empty lists (no children) → removed
/// - Empty list items → get an empty Paragraph
/// - Orphaned list items under Doc → unwrapped to their children
/// - Bare text under Doc → wrapped in Paragraph
///
/// A `Table` whose existence defines a spreadsheet sheet: it carries a
/// `sheetName` (or `ssv` version) attr, set by the spreadsheet persist
/// layer. Such a table must survive normalization even when empty (#128),
/// or trimming an empty sheet to a 0-row table would silently delete the
/// sheet. Document-mode tables carry neither attr.
fn is_spreadsheet_table(node: &Node) -> bool {
    matches!(
        node,
        Node::Element { attrs, .. }
            if attrs.contains_key("sheetName") || attrs.contains_key("ssv")
    )
}

/// Idempotent: calling on an already-valid document returns it unchanged.
pub fn normalize_doc(doc: &Node) -> Node {
    let Node::Element { content, .. } = doc else { return doc.clone() };
    let children: Vec<Node> = content.children.iter()
        .flat_map(|child| normalize_node(child, NodeType::Doc))
        .collect();
    let children = if children.is_empty() {
        vec![Node::element(NodeType::Paragraph)]
    } else {
        children
    };
    doc.copy_with_content(Fragment::from(children))
}

fn normalize_node(node: &Node, parent_type: NodeType) -> Vec<Node> {
    match node {
        Node::Text { .. } => {
            if parent_type == NodeType::Doc {
                vec![Node::element_with_content(
                    NodeType::Paragraph, Fragment::from(vec![node.clone()]),
                )]
            } else {
                vec![node.clone()]
            }
        }
        Node::Element { node_type, content, .. } => {
            // Orphaned structural nodes under Doc → unwrap to their children
            if matches!(node_type,
                NodeType::ListItem | NodeType::TaskItem
                | NodeType::TableRow | NodeType::TableCell | NodeType::TableHeader
            ) && parent_type == NodeType::Doc
            {
                return content.children.iter()
                    .flat_map(|c| normalize_node(c, NodeType::Doc))
                    .collect();
            }

            // Block elements inside textblocks (e.g., Paragraph containing Paragraph)
            // are invalid HTML and break contenteditable. Split them out as siblings.
            if node_type.is_textblock() {
                let has_block_children = content.children.iter().any(|c| {
                    matches!(c, Node::Element { node_type: nt, .. } if nt.is_block() && !nt.is_inline())
                });
                if has_block_children {
                    let mut result = Vec::new();
                    let mut inline_buf: Vec<Node> = Vec::new();

                    for child in &content.children {
                        let is_block_child = matches!(child, Node::Element { node_type: nt, .. } if nt.is_block() && !nt.is_inline());
                        if is_block_child {
                            // Flush buffered inline content as a paragraph
                            if !inline_buf.is_empty() {
                                result.push(node.copy_with_content(Fragment::from(inline_buf.drain(..).collect::<Vec<_>>())));
                            }
                            // Add the block child directly (it becomes a sibling)
                            result.extend(normalize_node(child, parent_type));
                        } else {
                            inline_buf.push(child.clone());
                        }
                    }
                    // Flush remaining inline content
                    if !inline_buf.is_empty() {
                        result.push(node.copy_with_content(Fragment::from(inline_buf)));
                    }
                    // If nothing produced, add an empty paragraph
                    if result.is_empty() {
                        result.push(Node::element(NodeType::Paragraph));
                    }
                    return result;
                }
            }

            // Recurse into children
            let children: Vec<Node> = content.children.iter()
                .flat_map(|c| normalize_node(c, *node_type))
                .collect();

            match node_type {
                // Empty lists → remove entirely
                NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList => {
                    if children.is_empty() {
                        vec![]
                    } else {
                        vec![node.copy_with_content(Fragment::from(children))]
                    }
                }
                // Empty tables → remove, EXCEPT a spreadsheet table: a
                // `sheetName` (or `ssv`) attr means the table's existence
                // defines a sheet, so it must survive even when trimmed to
                // zero rows (#128 persists only the used bounding box, so
                // an empty sheet is a 0-row table). Document-mode tables
                // carry neither attr and are still removed when empty.
                NodeType::Table => {
                    if children.is_empty() && !is_spreadsheet_table(node) {
                        vec![]
                    } else {
                        vec![node.copy_with_content(Fragment::from(children))]
                    }
                }
                // Empty table rows → remove
                NodeType::TableRow => {
                    if children.is_empty() {
                        vec![]
                    } else {
                        vec![node.copy_with_content(Fragment::from(children))]
                    }
                }
                // Empty list items / table cells → add an empty Paragraph
                NodeType::ListItem | NodeType::TaskItem
                | NodeType::TableCell | NodeType::TableHeader => {
                    if children.is_empty() {
                        vec![node.copy_with_content(Fragment::from(vec![
                            Node::element(NodeType::Paragraph),
                        ]))]
                    } else {
                        vec![node.copy_with_content(Fragment::from(children))]
                    }
                }
                // Everything else → just propagate normalized children
                _ => vec![node.copy_with_content(Fragment::from(children))],
            }
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

    // ── block_id_at: innermost anchoring (#116) ──

    fn para(text: &str) -> Node {
        Node::element_with_content(NodeType::Paragraph, Fragment::from(vec![Node::text(text)]))
    }

    #[test]
    fn block_id_at_returns_paragraph_for_plain_text() {
        // Top-level paragraph: anchor is the paragraph itself (unchanged).
        let p = para("hello");
        let p_id = p.block_id().unwrap().to_string();
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![p]));
        // Para open@0, content@1, "hello"@1..6; cursor at 3.
        assert_eq!(doc.block_id_at(3).as_deref(), Some(p_id.as_str()));
    }

    #[test]
    fn block_id_at_anchors_to_cell_not_table() {
        // #116: a comment in a table cell must anchor to the cell's content
        // (its paragraph), NOT the whole table.
        let c0 = para("a");
        let c0_id = c0.block_id().unwrap().to_string();
        let c1 = para("b");
        let c1_id = c1.block_id().unwrap().to_string();
        let cell0 = Node::element_with_content(NodeType::TableCell, Fragment::from(vec![c0]));
        let cell1 = Node::element_with_content(NodeType::TableCell, Fragment::from(vec![c1]));
        let row = Node::element_with_content(
            NodeType::TableRow,
            Fragment::from(vec![cell0, cell1]),
        );
        let table = Node::element_with_content(NodeType::Table, Fragment::from(vec![row]));
        let table_id = table.block_id().unwrap().to_string();
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![table]));

        // Positions: Table@0(open) Row@1 Cell0@2 Para@3 content@4 "a"@4..5.
        // Cell1 sits after cell0 (cell0 size = 2+(2+1)=5): Cell1@7 Para@8
        // content@9 "b"@9..10.
        let in_cell0 = doc.block_id_at(4);
        let in_cell1 = doc.block_id_at(9);
        assert_eq!(in_cell0.as_deref(), Some(c0_id.as_str()), "cell 0 -> its paragraph");
        assert_eq!(in_cell1.as_deref(), Some(c1_id.as_str()), "cell 1 -> its paragraph");
        // Distinct cells get distinct anchors, and never the table's id.
        assert_ne!(in_cell0, in_cell1, "different cells must have different anchors");
        assert_ne!(in_cell0.as_deref(), Some(table_id.as_str()));
        assert_ne!(in_cell1.as_deref(), Some(table_id.as_str()));
    }

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

    // ── structural_hash attribute-order independence ──

    /// A node's structural hash must not depend on the iteration order
    /// of its attribute map. `HashMap` iteration order differs between
    /// independently-constructed instances, so a doc decoded fresh from
    /// the CRDT must still hash identically to the local copy — otherwise
    /// the collab send-effect's `hash == prev_hash` guard fails on every
    /// echoed frame and re-sends forever (the calendar History-API storm).
    #[test]
    fn structural_hash_is_attr_order_independent() {
        // Enough keys that two independently-seeded HashMaps iterating in
        // the same relative order by chance is astronomically unlikely.
        let keys: Vec<(String, String)> = (0..16)
            .map(|i| (format!("attr{i}"), format!("val{i}")))
            .collect();

        let mut forward = HashMap::new();
        for (k, v) in keys.iter() {
            forward.insert(k.clone(), v.clone());
        }
        let mut reverse = HashMap::new();
        for (k, v) in keys.iter().rev() {
            reverse.insert(k.clone(), v.clone());
        }

        let node_a = Node::Element {
            node_type: NodeType::Calendar,
            attrs: forward,
            content: Fragment::empty(),
            marks: vec![],
        };
        let node_b = Node::Element {
            node_type: NodeType::Calendar,
            attrs: reverse,
            content: Fragment::empty(),
            marks: vec![],
        };

        assert_eq!(node_a.structural_hash(), node_b.structural_hash());
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

        // #148 slice 6 — Mention: inline leaf atom.
        assert!(NodeType::Mention.is_leaf());
        assert!(NodeType::Mention.is_atom());
        assert!(NodeType::Mention.is_inline());
        assert!(!NodeType::Mention.is_block());
        assert!(!NodeType::Mention.is_commentable());
        assert!(NodeType::Mention.needs_block_id());
        assert_eq!(NodeType::Mention.default_attrs().len(), 2);
        assert!(NodeType::Mention.default_attrs().contains_key("user_id"));
        assert!(NodeType::Mention.default_attrs().contains_key("display"));
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

    // ── normalize_doc ──

    #[test]
    fn normalize_preserves_valid_doc() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hello")]),
                ),
                Node::element_with_content(
                    NodeType::BulletList,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("item")]),
                        )]),
                    )]),
                ),
            ]),
        );
        let normalized = normalize_doc(&doc);
        assert_eq!(normalized.text_content(), doc.text_content());
        assert_eq!(normalized.child_count(), 2);
    }

    #[test]
    fn normalize_removes_empty_list() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("before")]),
                ),
                Node::Element {
                    node_type: NodeType::BulletList,
                    attrs: HashMap::new(),
                    content: Fragment::empty(),
                    marks: vec![],
                },
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("after")]),
                ),
            ]),
        );
        let normalized = normalize_doc(&doc);
        assert_eq!(normalized.child_count(), 2);
        assert_eq!(normalized.child(0).unwrap().text_content(), "before");
        assert_eq!(normalized.child(1).unwrap().text_content(), "after");
    }

    #[test]
    fn normalize_keeps_empty_spreadsheet_table_with_sheet_name() {
        // #128: an empty sheet trims to a 0-row Table; its `sheetName`
        // attr means its existence defines a sheet, so normalize must
        // NOT remove it (else the sheet vanishes on round-trip).
        let mut attrs = HashMap::new();
        attrs.insert("sheetName".to_string(), "Sheet1".to_string());
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::Element {
                node_type: NodeType::Table,
                attrs,
                content: Fragment::empty(),
                marks: vec![],
            }]),
        );
        let normalized = normalize_doc(&doc);
        assert_eq!(normalized.child_count(), 1);
        assert_eq!(
            normalized.child(0).unwrap().node_type(),
            Some(NodeType::Table)
        );
    }

    #[test]
    fn normalize_removes_empty_document_mode_table() {
        // Counterpart: an empty Table with NO sheetName/ssv is a
        // document-mode table and is still removed.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("keep")]),
                ),
                Node::Element {
                    node_type: NodeType::Table,
                    attrs: HashMap::new(),
                    content: Fragment::empty(),
                    marks: vec![],
                },
            ]),
        );
        let normalized = normalize_doc(&doc);
        assert_eq!(normalized.child_count(), 1);
        assert_eq!(normalized.child(0).unwrap().text_content(), "keep");
    }

    #[test]
    fn normalize_adds_paragraph_to_empty_list_item() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::Element {
                    node_type: NodeType::ListItem,
                    attrs: HashMap::new(),
                    content: Fragment::empty(),
                    marks: vec![],
                }]),
            )]),
        );
        let normalized = normalize_doc(&doc);
        let list = normalized.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        let item = list.child(0).unwrap();
        assert_eq!(item.node_type(), Some(NodeType::ListItem));
        let para = item.child(0).unwrap();
        assert_eq!(para.node_type(), Some(NodeType::Paragraph));
    }

    #[test]
    fn normalize_unwraps_orphaned_list_item() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("orphan")]),
                )]),
            )]),
        );
        let normalized = normalize_doc(&doc);
        let first = normalized.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Paragraph));
        assert_eq!(first.text_content(), "orphan");
    }

    #[test]
    fn normalize_wraps_bare_text_under_doc() {
        let doc = Node::Element {
            node_type: NodeType::Doc,
            attrs: HashMap::new(),
            content: Fragment::from(vec![Node::text("bare")]),
            marks: vec![],
        };
        let normalized = normalize_doc(&doc);
        let first = normalized.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Paragraph));
        assert_eq!(first.text_content(), "bare");
    }

    #[test]
    fn normalize_is_idempotent() {
        // An already-corrupted doc
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::Element {
                    node_type: NodeType::BulletList,
                    attrs: HashMap::new(),
                    content: Fragment::empty(),
                    marks: vec![],
                },
                Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("orphan")]),
                    )]),
                ),
            ]),
        );
        let first = normalize_doc(&doc);
        let second = normalize_doc(&first);
        assert_eq!(first, second);
    }

    #[test]
    fn normalize_empty_doc_gets_paragraph() {
        let doc = Node::element_with_content(NodeType::Doc, Fragment::empty());
        let normalized = normalize_doc(&doc);
        assert_eq!(normalized.child_count(), 1);
        assert_eq!(normalized.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
    }

    #[test]
    fn normalize_removes_list_whose_items_are_all_empty_after_normalization() {
        // A list where the only item contains an empty nested list (which gets removed),
        // leaving the item empty, which gets a paragraph, so the outer list survives.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::Element {
                        node_type: NodeType::BulletList,
                        attrs: HashMap::new(),
                        content: Fragment::empty(),
                        marks: vec![],
                    }]),
                )]),
            )]),
        );
        let normalized = normalize_doc(&doc);
        // Inner empty list removed → ListItem becomes empty → gets Paragraph
        let list = normalized.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        let item = list.child(0).unwrap();
        assert_eq!(item.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
    }

    #[test]
    fn normalize_splits_nested_paragraph_out_of_textblock() {
        // A paragraph containing text + a nested paragraph (invalid HTML).
        // Should be split into separate sibling paragraphs.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::Element {
                node_type: NodeType::Paragraph,
                attrs: HashMap::new(),
                content: Fragment::from(vec![
                    Node::text("before"),
                    Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("nested")]),
                    ),
                    Node::text("after"),
                ]),
                marks: vec![],
            }]),
        );
        let normalized = normalize_doc(&doc);
        // Should produce 3 paragraphs: "before", "nested", "after"
        assert_eq!(normalized.child_count(), 3,
            "nested paragraph should be split out, got {} children: {:?}",
            normalized.child_count(), normalized);
        assert_eq!(normalized.child(0).unwrap().text_content(), "before");
        assert_eq!(normalized.child(1).unwrap().text_content(), "nested");
        assert_eq!(normalized.child(2).unwrap().text_content(), "after");
    }

    #[test]
    fn normalize_splits_nested_paragraph_with_marks() {
        // Paragraph with bold text + nested paragraph (the exact bug from the report)
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::Element {
                node_type: NodeType::Paragraph,
                attrs: HashMap::new(),
                content: Fragment::from(vec![
                    Node::text("a"),
                    Node::text_with_marks("s", vec![Mark::new(MarkType::Bold)]),
                    Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("ss")]),
                    ),
                ]),
                marks: vec![],
            }]),
        );
        let normalized = normalize_doc(&doc);
        assert_eq!(normalized.child_count(), 2);
        // First para: "a" + bold "s"
        let first = normalized.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Paragraph));
        assert_eq!(first.text_content(), "as");
        // Second para: "ss"
        let second = normalized.child(1).unwrap();
        assert_eq!(second.node_type(), Some(NodeType::Paragraph));
        assert_eq!(second.text_content(), "ss");
    }
}
