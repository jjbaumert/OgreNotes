use super::model::Node;

/// A resolved position in the document tree.
/// Provides context about where a position falls: its depth,
/// the parent node at each level, and the offset within the current node.
#[derive(Debug, Clone)]
pub struct ResolvedPos {
    /// The absolute position in the document.
    pub pos: usize,
    /// Nesting depth (0 = document root).
    pub depth: usize,
    /// Path from root to the innermost containing element.
    /// Each entry records the child index and the absolute position
    /// where that element's content begins.
    /// Length = depth + 1 (entry 0 is for the doc root).
    path: Vec<PathEntry>,
}

#[derive(Debug, Clone)]
struct PathEntry {
    /// Index of this node among its parent's children.
    /// For depth 0 (the document root), this is always 0 and unused.
    index_in_parent: usize,
    /// Absolute position of the start of this node's content
    /// (i.e., right after the opening boundary).
    content_start: usize,
}

impl ResolvedPos {
    /// The absolute position where the content of the node at `depth` starts.
    pub fn start(&self, depth: usize) -> usize {
        self.path[depth].content_start
    }

    /// The absolute position where the content of the node at `depth` ends.
    pub fn end(&self, depth: usize, doc: &Node) -> usize {
        let node = self.node_at(depth, doc);
        self.path[depth].content_start + node.content_size()
    }

    /// The offset of this position within the innermost parent's content.
    pub fn parent_offset(&self) -> usize {
        self.pos - self.path[self.depth].content_start
    }

    /// The offset within a text node (0 if not in a text node).
    pub fn text_offset(&self, doc: &Node) -> usize {
        let parent = self.node_at(self.depth, doc);
        let offset = self.parent_offset();
        let mut pos = 0;
        for i in 0..parent.child_count() {
            let child = parent.child(i).unwrap();
            let child_size = child.node_size();
            if pos + child_size > offset {
                if child.is_text() {
                    return offset - pos;
                }
                return 0;
            }
            pos += child_size;
        }
        0
    }

    /// The index of the child in the parent that contains (or follows) this position.
    pub fn index(&self, doc: &Node) -> usize {
        let parent = self.node_at(self.depth, doc);
        let offset = self.parent_offset();
        let mut pos = 0;
        for i in 0..parent.child_count() {
            if pos >= offset {
                return i;
            }
            pos += parent.child(i).unwrap().node_size();
        }
        parent.child_count()
    }

    /// Get the node at the given depth by walking from the document root.
    pub fn node_at<'a>(&self, depth: usize, doc: &'a Node) -> &'a Node {
        if depth == 0 {
            return doc;
        }
        let mut current = doc;
        for d in 1..=depth {
            let idx = self.path[d].index_in_parent;
            current = current.child(idx).expect("invalid resolved position path");
        }
        current
    }

    /// The non-text element node directly after this position, if any.
    /// Returns `None` if the position is inside a text node or there is
    /// no element node at this boundary. Text nodes are never returned.
    pub fn node_after<'a>(&self, doc: &'a Node) -> Option<&'a Node> {
        let parent = self.node_at(self.depth, doc);
        let offset = self.parent_offset();
        let mut pos = 0;
        for i in 0..parent.child_count() {
            let child = parent.child(i).unwrap();
            let size = child.node_size();
            if pos + size > offset {
                // This child touches or crosses the offset
                if pos == offset && !child.is_text() {
                    return Some(child);
                }
                return None; // inside a text node or at a text boundary
            }
            pos += size;
        }
        None
    }

    /// The non-text element node directly before this position, if any.
    /// Returns `None` if the position is inside a text node or there is
    /// no element node ending at this boundary. Text nodes are never returned.
    pub fn node_before<'a>(&self, doc: &'a Node) -> Option<&'a Node> {
        let parent = self.node_at(self.depth, doc);
        let offset = self.parent_offset();
        let mut pos = 0;
        for i in 0..parent.child_count() {
            let child = parent.child(i).unwrap();
            let child_end = pos + child.node_size();
            if child_end == offset && !child.is_text() {
                return Some(child);
            }
            if child_end > offset {
                return None;
            }
            pos = child_end;
        }
        None
    }
}

/// Resolve an absolute position within a document to a `ResolvedPos`.
/// The position is relative to the document's content (0 = start of content).
/// The `doc` parameter must be a non-leaf Element node (typically `NodeType::Doc`).
/// Returns `None` if the position is out of bounds.
pub fn resolve(doc: &Node, pos: usize) -> Option<ResolvedPos> {
    debug_assert!(
        matches!(doc, Node::Element { node_type, .. } if !node_type.is_leaf()),
        "resolve() requires a non-leaf element node as the document root"
    );
    if pos > doc.content_size() {
        return None;
    }

    let mut path = Vec::new();

    // Start with the doc root: content_start = 0 (positions are relative to doc content)
    path.push(PathEntry {
        index_in_parent: 0,
        content_start: 0,
    });

    let mut current = doc;
    let mut remaining = pos;

    loop {
        // Walk the children of `current` to find which one contains `remaining`
        let content = match current {
            Node::Element { content, node_type, .. } if !node_type.is_leaf() => content,
            _ => break,
        };

        let mut offset = 0;
        let mut found_child = false;

        for (i, child) in content.children.iter().enumerate() {
            let child_size = child.node_size();

            if remaining < offset + child_size {
                // Position falls within this child
                match child {
                    Node::Text { .. } => {
                        // Inside a text node; the path stops at the current element
                        break;
                    }
                    Node::Element { node_type, .. } if node_type.is_leaf() => {
                        // At a leaf element; the path stops at the current element
                        break;
                    }
                    Node::Element { .. } => {
                        if remaining == offset {
                            // Right before this element's opening boundary;
                            // stay at the current depth
                            break;
                        }
                        // Inside this element: descend into it
                        let inner_content_start =
                            path.last().unwrap().content_start + offset + 1; // +1 for open boundary
                        path.push(PathEntry {
                            index_in_parent: i,
                            content_start: inner_content_start,
                        });
                        remaining = remaining - offset - 1; // -1 for open boundary
                        current = child;
                        found_child = true;
                        break;
                    }
                }
            }
            offset += child_size;
        }

        if !found_child {
            break;
        }
    }

    let depth = path.len() - 1;
    Some(ResolvedPos { pos, depth, path })
}

/// A range of sibling nodes within a parent, used for wrapping/lifting operations.
#[derive(Debug, Clone)]
pub struct NodeRange {
    /// Resolved start position.
    pub from: usize,
    /// Resolved end position.
    pub to: usize,
    /// Depth of the common parent.
    pub depth: usize,
    /// Start child index (inclusive).
    pub start_index: usize,
    /// End child index (exclusive).
    pub end_index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::*;

    fn make_simple_doc() -> Node {
        // doc > [paragraph("Hello"), paragraph("World")]
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

    // Position map for make_simple_doc():
    //  0: doc content start (before first para open boundary)
    //  1: first para content start (after open boundary)
    //  2-5: chars H, e, l, l (at offsets 1-4 in para)
    //  6: after "Hello" (para content end)
    //  7: after first para close boundary (between paras, doc level)
    //  8: second para content start
    //  9-12: chars W, o, r, l
    //  13: after "World"
    //  14: after second para close boundary (doc content end)

    #[test]
    fn resolve_at_doc_start() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 0).unwrap();
        assert_eq!(rp.pos, 0);
        assert_eq!(rp.depth, 0);
        assert_eq!(rp.parent_offset(), 0);
    }

    #[test]
    fn resolve_inside_first_paragraph() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 1).unwrap();
        assert_eq!(rp.pos, 1);
        assert_eq!(rp.depth, 1);
        assert_eq!(rp.parent_offset(), 0);
    }

    #[test]
    fn resolve_mid_text() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 3).unwrap();
        assert_eq!(rp.pos, 3);
        assert_eq!(rp.depth, 1);
        assert_eq!(rp.parent_offset(), 2);
        assert_eq!(rp.text_offset(&doc), 2);
    }

    #[test]
    fn resolve_end_of_first_paragraph() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 6).unwrap();
        assert_eq!(rp.pos, 6);
        assert_eq!(rp.depth, 1);
        assert_eq!(rp.parent_offset(), 5);
    }

    #[test]
    fn resolve_between_paragraphs() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 7).unwrap();
        assert_eq!(rp.pos, 7);
        assert_eq!(rp.depth, 0);
    }

    #[test]
    fn resolve_inside_second_paragraph() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 8).unwrap();
        assert_eq!(rp.pos, 8);
        assert_eq!(rp.depth, 1);
        assert_eq!(rp.parent_offset(), 0);
    }

    #[test]
    fn resolve_at_doc_end() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 14).unwrap();
        assert_eq!(rp.pos, 14);
        assert_eq!(rp.depth, 0);
    }

    #[test]
    fn resolve_out_of_bounds_returns_none() {
        let doc = make_simple_doc();
        assert!(resolve(&doc, 15).is_none());
        assert!(resolve(&doc, 100).is_none());
    }

    #[test]
    fn resolve_empty_doc() {
        let doc = Node::empty_doc();
        // doc content size = paragraph(2) = 2
        let rp = resolve(&doc, 0).unwrap();
        assert_eq!(rp.depth, 0);

        let rp = resolve(&doc, 1).unwrap();
        assert_eq!(rp.depth, 1); // inside paragraph

        let rp = resolve(&doc, 2).unwrap();
        assert_eq!(rp.depth, 0); // after paragraph
    }

    #[test]
    fn resolve_with_hr() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hi")]),
                ),
                Node::element(NodeType::HorizontalRule),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Lo")]),
                ),
            ]),
        );
        // para("Hi"): size=4, hr: size=1, para("Lo"): size=4 -> doc content=9
        assert_eq!(doc.content_size(), 9);

        // Position 4 = after first paragraph's close (doc level)
        let rp = resolve(&doc, 4).unwrap();
        assert_eq!(rp.depth, 0);

        // Position 5 = after hr (doc level)
        let rp = resolve(&doc, 5).unwrap();
        assert_eq!(rp.depth, 0);
    }

    #[test]
    fn resolve_nested_list() {
        // doc > bulletList > listItem > paragraph("item")
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
        // Sizes: text=4, para=6, li=8, ul=10, doc content=10
        assert_eq!(doc.content_size(), 10);

        // Position 4 = inside paragraph, before "i" (para content start)
        // doc(cs=0) > ul at offset 0, cs=0+0+1=1 > li at offset 0, cs=1+0+1=2 > para at offset 0, cs=2+0+1=3
        // pos 4: remaining after doc=4, after ul=3, after li=2, after para=1 -> inside text at offset 1
        // Wait, let me recalculate:
        // resolve(doc, 4): content_start=0, walk children: ul at offset 0, size=10, 4<10
        //   ul is element, 4 != 0, descend: content_start=0+0+1=1, remaining=4-0-1=3
        //   walk ul children: li at offset 0, size=8, 3<8
        //     li is element, 3 != 0, descend: content_start=1+0+1=2, remaining=3-0-1=2
        //     walk li children: para at offset 0, size=6, 2<6
        //       para is element, 2 != 0, descend: content_start=2+0+1=3, remaining=2-0-1=1
        //       walk para children: text "item" at offset 0, size=4, 1<4 -> text, stop
        //     depth=3 (doc=0, ul=1, li=2, para=3), parent_offset=pos-content_start=4-3=1
        let rp = resolve(&doc, 4).unwrap();
        assert_eq!(rp.depth, 3); // doc > ul > li > para
        assert_eq!(rp.parent_offset(), 1); // 1 char into "item"

        // Position 3 = paragraph content start
        let rp = resolve(&doc, 3).unwrap();
        assert_eq!(rp.depth, 3);
        assert_eq!(rp.parent_offset(), 0);

        // Position 7 = after "item" (para content end)
        let rp = resolve(&doc, 7).unwrap();
        assert_eq!(rp.depth, 3);
        assert_eq!(rp.parent_offset(), 4);
    }

    #[test]
    fn node_after_at_element_boundary() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 0).unwrap();
        let after = rp.node_after(&doc);
        assert!(after.is_some());
        assert_eq!(after.unwrap().node_type(), Some(NodeType::Paragraph));
    }

    #[test]
    fn node_before_at_element_boundary() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 7).unwrap();
        let before = rp.node_before(&doc);
        assert!(before.is_some());
        assert_eq!(before.unwrap().node_type(), Some(NodeType::Paragraph));
    }

    #[test]
    fn node_after_in_text_returns_none() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 3).unwrap();
        assert!(rp.node_after(&doc).is_none());
    }

    #[test]
    fn resolve_roundtrip_all_positions() {
        let doc = make_simple_doc();
        let size = doc.content_size();
        for pos in 0..=size {
            let rp = resolve(&doc, pos);
            assert!(rp.is_some(), "position {pos} should resolve");
            assert_eq!(rp.unwrap().pos, pos);
        }
    }

    #[test]
    fn resolve_unicode_positions() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("café 🌍")]),
            )]),
        );
        let text_len = "café 🌍".chars().count();
        assert_eq!(text_len, 6);
        assert_eq!(doc.content_size(), 8); // para(2) + text(6)

        for pos in 0..=8 {
            let rp = resolve(&doc, pos);
            assert!(rp.is_some(), "pos {pos} should resolve in unicode doc");
        }
        assert!(resolve(&doc, 9).is_none());
    }

    #[test]
    fn start_and_end() {
        let doc = make_simple_doc();
        let rp = resolve(&doc, 3).unwrap();
        assert_eq!(rp.start(0), 0);
        assert_eq!(rp.start(1), 1);
        assert_eq!(rp.end(0, &doc), 14);
        assert_eq!(rp.end(1, &doc), 6);
    }

    #[test]
    fn index_in_children() {
        let doc = make_simple_doc();
        // Position 0 = before first paragraph
        let rp = resolve(&doc, 0).unwrap();
        assert_eq!(rp.index(&doc), 0);

        // Position 7 = between paragraphs (after first para)
        let rp = resolve(&doc, 7).unwrap();
        assert_eq!(rp.index(&doc), 1);
    }

    // ── Additional tests from review ──

    #[test]
    fn node_after_with_leaf_hr() {
        // doc > [paragraph("Hi"), hr, paragraph("Lo")]
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hi")]),
                ),
                Node::element(NodeType::HorizontalRule),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Lo")]),
                ),
            ]),
        );
        // Position 4 = after first para, before hr (doc level)
        let rp = resolve(&doc, 4).unwrap();
        assert_eq!(rp.depth, 0);
        let after = rp.node_after(&doc);
        assert!(after.is_some());
        assert_eq!(after.unwrap().node_type(), Some(NodeType::HorizontalRule));
    }

    #[test]
    fn node_before_with_leaf_hr() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hi")]),
                ),
                Node::element(NodeType::HorizontalRule),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Lo")]),
                ),
            ]),
        );
        // Position 5 = after hr (doc level)
        let rp = resolve(&doc, 5).unwrap();
        assert_eq!(rp.depth, 0);
        let before = rp.node_before(&doc);
        assert!(before.is_some());
        assert_eq!(before.unwrap().node_type(), Some(NodeType::HorizontalRule));
    }

    #[test]
    fn node_after_text_then_element() {
        // This tests the critical bug fix: [Text("ab"), Paragraph(...)]
        // At offset 2 (after text), node_after should return the Paragraph
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text("ab"),
                    Node::element(NodeType::HardBreak),
                ]),
            )]),
        );
        // Inside the paragraph: text "ab" (size 2) + hard break (size 1)
        // Position 1 (para content start) + 2 (after "ab") = position 3
        let rp = resolve(&doc, 3).unwrap();
        assert_eq!(rp.depth, 1);
        assert_eq!(rp.parent_offset(), 2);
        let after = rp.node_after(&doc);
        assert!(after.is_some(), "should find HardBreak after text");
        assert_eq!(after.unwrap().node_type(), Some(NodeType::HardBreak));
    }

    #[test]
    fn resolve_at_nested_element_open_boundary() {
        // Test that position at the exact opening boundary of a nested element
        // stays at the parent depth (doesn't descend)
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Blockquote,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("quoted")]),
                    )]),
                ),
            ]),
        );
        // Position 0 = before blockquote (doc level)
        let rp = resolve(&doc, 0).unwrap();
        assert_eq!(rp.depth, 0);
        assert_eq!(rp.parent_offset(), 0);

        // Position 1 = inside blockquote, before paragraph (blockquote level)
        let rp = resolve(&doc, 1).unwrap();
        assert_eq!(rp.depth, 1);
        assert_eq!(rp.parent_offset(), 0);

        // Position 2 = inside paragraph (paragraph level)
        let rp = resolve(&doc, 2).unwrap();
        assert_eq!(rp.depth, 2);
        assert_eq!(rp.parent_offset(), 0);
    }

    #[test]
    fn end_at_deep_nesting() {
        // doc > blockquote > paragraph("deep")
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("deep")]),
                )]),
            )]),
        );
        // Resolve inside "deep" (position 3 = para content start)
        let rp = resolve(&doc, 3).unwrap();
        assert_eq!(rp.depth, 2);
        assert_eq!(rp.start(0), 0); // doc content start
        assert_eq!(rp.start(1), 1); // blockquote content start
        assert_eq!(rp.start(2), 2); // paragraph content start
        assert_eq!(rp.end(0, &doc), 8); // doc content end
        assert_eq!(rp.end(1, &doc), 7); // blockquote content end
        assert_eq!(rp.end(2, &doc), 6); // paragraph content end (4 chars)
    }

    #[test]
    fn index_at_content_end() {
        let doc = make_simple_doc();
        // Position 14 = doc content end
        let rp = resolve(&doc, 14).unwrap();
        assert_eq!(rp.depth, 0);
        // index() should return child_count (past-end index)
        assert_eq!(rp.index(&doc), 2);
    }
}
