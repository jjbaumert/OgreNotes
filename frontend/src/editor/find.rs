// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #147: in-document find. Locates every occurrence of a query string and
//! returns its model `(from, to)` position range so the find/replace bar can
//! select it (navigation) and replace it through the editor's transaction
//! API (collab-safe).
//!
//! Matching is case-insensitive and scoped to a single textblock — a match
//! never spans block boundaries or inline atoms (images, hard breaks). It
//! also doesn't cross inline mark boundaries within a block *separately*
//! either; runs are concatenated per textblock, so "He**llo**" (partly bold)
//! is still found.

use super::model::Node;

/// Lowercase a char to a single char, preserving 1:1 char↔position mapping
/// (full `to_lowercase()` can expand to multiple chars and desync the map).
fn lc(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// All `(from, to)` model ranges where `query` occurs in `doc`, in document
/// order, non-overlapping, case-insensitive. Empty query → no matches.
pub fn find_matches(doc: &Node, query: &str) -> Vec<(usize, usize)> {
    let q: Vec<char> = query.chars().map(lc).collect();
    if q.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    collect(doc, 0, &q, &mut out);
    out
}

/// Walk `node`'s content starting at model position `content_start`.
fn collect(node: &Node, content_start: usize, q: &[char], out: &mut Vec<(usize, usize)>) {
    if node.node_type().map(|t| t.is_textblock()).unwrap_or(false) {
        search_textblock(node, content_start, q, out);
        return;
    }
    // A container: each child occupies [pos, pos + node_size); an element
    // child's own content starts one past its open token.
    let mut pos = content_start;
    for i in 0..node.child_count() {
        let child = node.child(i).expect("child in range");
        if !child.is_text() {
            collect(child, pos + 1, q, out);
        }
        pos += child.node_size();
    }
}

/// Concatenate a textblock's inline text (with per-char model positions),
/// breaking the run at inline atoms, and record matches.
fn search_textblock(block: &Node, content_start: usize, q: &[char], out: &mut Vec<(usize, usize)>) {
    let mut chars: Vec<char> = Vec::new();
    let mut posmap: Vec<usize> = Vec::new();
    let mut pos = content_start;
    for i in 0..block.child_count() {
        let child = block.child(i).expect("child in range");
        if child.is_text() {
            for ch in child.text_content().chars() {
                chars.push(ch);
                posmap.push(pos);
                pos += 1;
            }
        } else {
            search_run(&chars, &posmap, q, out);
            chars.clear();
            posmap.clear();
            pos += child.node_size();
        }
    }
    search_run(&chars, &posmap, q, out);
}

fn search_run(chars: &[char], posmap: &[usize], q: &[char], out: &mut Vec<(usize, usize)>) {
    let qlen = q.len();
    if chars.len() < qlen {
        return;
    }
    let run: Vec<char> = chars.iter().map(|&c| lc(c)).collect();
    let mut i = 0;
    while i + qlen <= run.len() {
        if run[i..i + qlen] == *q {
            out.push((posmap[i], posmap[i + qlen - 1] + 1));
            i += qlen; // non-overlapping
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::{Fragment, Node, NodeType};

    fn para_doc(text: &str) -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text(text)]),
            )]),
        )
    }

    #[test]
    fn finds_single_match_with_model_positions() {
        // "Hello world": H=1 e=2 l=3 l=4 o=5 ' '=6 ... so "lo" = (4,6).
        let doc = para_doc("Hello world");
        assert_eq!(find_matches(&doc, "lo"), vec![(4, 6)]);
    }

    #[test]
    fn finds_multiple_non_overlapping_matches() {
        // "aaaa": positions 1,2,3,4. "aa" → (1,3) then (3,5), non-overlapping.
        let doc = para_doc("aaaa");
        assert_eq!(find_matches(&doc, "aa"), vec![(1, 3), (3, 5)]);
    }

    #[test]
    fn is_case_insensitive() {
        let doc = para_doc("Hello HELLO hello");
        let m = find_matches(&doc, "hello");
        assert_eq!(m.len(), 3, "all three casings match");
    }

    #[test]
    fn empty_query_finds_nothing() {
        let doc = para_doc("anything");
        assert!(find_matches(&doc, "").is_empty());
    }

    #[test]
    fn finds_across_inline_mark_boundary() {
        // "He" then "llo" as two text nodes (a mark boundary) — the run is
        // concatenated, so "Hello" is still found at (1,6).
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("He"), Node::text("llo")]),
            )]),
        );
        assert_eq!(find_matches(&doc, "hello"), vec![(1, 6)]);
    }

    #[test]
    fn matches_in_separate_blocks_carry_distinct_positions() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("cat")]),
                ),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("cat")]),
                ),
            ]),
        );
        let m = find_matches(&doc, "cat");
        assert_eq!(m.len(), 2);
        // First block "cat" at content_start 1 → (1,4). Second block opens
        // after the first (size 3+2=5), at pos 5, content 6 → (6,9).
        assert_eq!(m[0], (1, 4));
        assert_eq!(m[1], (6, 9));
    }
}
