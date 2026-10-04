// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Convert pasted `$…$` and `$$…$$` into equations, preserving surrounding
//! formatting. Standalone display equations become blocks where the schema
//! allows them; equations amid prose remain inline. Code stays literal.

use super::model::{Fragment, Mark, MarkType, Node, NodeType, Slice};
use super::schema::{Schema, default_schema};

#[derive(Debug, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Math(&'a str),
    DisplayMath(&'a str),
}

fn split_math(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut literal_from = 0;
    let mut i = 0;
    while let Some(off) = text[i..].find('$') {
        let open = i + off;
        i = open + 1;
        if text[..open].ends_with(['\\', '$']) {
            continue;
        }
        let display = text[open..].starts_with("$$");
        let delimiter_len = if display { 2 } else { 1 };
        let from = open + delimiter_len;
        i = from;
        if text[from..].starts_with('$')
            || (!display && text[from..].chars().next().is_none_or(char::is_whitespace))
        {
            continue;
        }
        let Some(close) = closing_dollar(text, from, display) else {
            continue;
        };
        let source = &text[from..close];
        let mode = if display {
            ogrenotes_math::Display::Block
        } else {
            ogrenotes_math::Display::Inline
        };
        if ogrenotes_math::to_mathml(source, mode).is_err() {
            continue;
        }
        if open > literal_from {
            out.push(Piece::Text(&text[literal_from..open]));
        }
        out.push(if display {
            Piece::DisplayMath(source)
        } else {
            Piece::Math(source)
        });
        literal_from = close + delimiter_len;
        i = literal_from;
    }
    if literal_from < text.len() {
        out.push(Piece::Text(&text[literal_from..]));
    }
    out
}

fn closing_dollar(text: &str, from: usize, display: bool) -> Option<usize> {
    let mut j = from;
    while let Some(off) = text[j..].find('$') {
        let at = j + off;
        j = at + 1;
        let before = text[..at].chars().next_back();
        let after = text[at + 1..].chars().next();
        if before == Some('\\') {
            continue;
        }
        if display {
            if after == Some('$') && !text[at + 2..].starts_with('$') {
                return Some(at);
            }
        } else if after == Some('$') {
            // A display delimiter cannot close an inline equation.
            return None;
        } else if at != from
            && !before.is_some_and(char::is_whitespace)
            && !after.is_some_and(|c| c.is_ascii_digit())
        {
            return Some(at);
        }
    }
    None
}

pub fn convert_math(slice: Slice, destination: NodeType) -> Slice {
    if !has_math_candidate(&slice.content) {
        return slice;
    }
    let schema = default_schema();
    Slice::new(
        convert_fragment(slice.content, destination, &schema),
        slice.open_start,
        slice.open_end,
    )
}

fn has_math_candidate(f: &Fragment) -> bool {
    f.children.iter().any(|n| match n {
        Node::Text { text, .. } => text.contains('$'),
        Node::Element {
            node_type, content, ..
        } => *node_type == NodeType::MathBlock || has_math_candidate(content),
    })
}

fn math_node(node_type: NodeType, source: &str) -> Node {
    let attrs =
        std::collections::HashMap::from([("source".to_string(), source.trim().to_string())]);
    Node::element_with_attrs(node_type, attrs, Fragment::empty())
}

fn standalone_display(content: &Fragment) -> Option<String> {
    let mut text = String::new();
    for node in &content.children {
        match node {
            Node::Text { text: t, marks }
                if !marks.iter().any(|m| m.mark_type == MarkType::Code) =>
            {
                text.push_str(t)
            }
            Node::Element {
                node_type: NodeType::HardBreak,
                ..
            } => text.push('\n'),
            _ => return None,
        }
    }
    match split_math(text.trim()).as_slice() {
        [Piece::DisplayMath(source)] => Some(source.trim().to_string()),
        _ => None,
    }
}

fn convert_fragment(f: Fragment, parent: NodeType, schema: &Schema) -> Fragment {
    let block_allowed = schema
        .node_spec(parent)
        .is_some_and(|s| s.valid_children.contains(&NodeType::MathBlock));
    // Native browser cuts can provide bare formatting spans without a paragraph.
    if block_allowed && let Some(source) = standalone_display(&f) {
        return Fragment::from(vec![math_node(NodeType::MathBlock, &source)]);
    }
    let mut out = Vec::with_capacity(f.children.len());
    let mut text = String::new();
    let mut segments = Vec::new();
    for node in f.children {
        if let Node::Text { text: t, marks } = &node
            && !marks.iter().any(|m| m.mark_type == MarkType::Code)
        {
            segments.push((text.len(), marks.clone()));
            text.push_str(t);
            continue;
        }
        flush_text(&mut out, &mut text, &mut segments);
        match node {
            Node::Element {
                node_type,
                attrs,
                content,
                marks,
            } if node_type != NodeType::CodeBlock => {
                if node_type == NodeType::MathBlock && !block_allowed {
                    let equation = math_node(
                        NodeType::MathInline,
                        attrs.get("source").map(String::as_str).unwrap_or(""),
                    );
                    out.push(if parent.is_textblock() {
                        equation
                    } else {
                        Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![equation]),
                        )
                    });
                } else if node_type == NodeType::Paragraph
                    && block_allowed
                    && let Some(source) = standalone_display(&content)
                {
                    out.push(math_node(NodeType::MathBlock, &source));
                } else {
                    out.push(Node::Element {
                        node_type,
                        attrs,
                        content: convert_fragment(content, node_type, schema),
                        marks,
                    });
                }
            }
            other => out.push(other),
        }
    }
    flush_text(&mut out, &mut text, &mut segments);
    Fragment::from(out)
}

/// Scan across adjacent formatting spans, but never across code or inline atoms.
fn flush_text(out: &mut Vec<Node>, text: &mut String, segments: &mut Vec<(usize, Vec<Mark>)>) {
    let mut offset = 0;
    for piece in split_math(text) {
        match piece {
            Piece::Text(t) => {
                let end = offset + t.len();
                let first = segments
                    .partition_point(|(start, _)| *start <= offset)
                    .saturating_sub(1);
                for index in first..segments.len() {
                    let (start, marks) = &segments[index];
                    let from = offset.max(*start);
                    let to = end.min(
                        segments
                            .get(index + 1)
                            .map_or(text.len(), |(next, _)| *next),
                    );
                    if from < to {
                        out.push(Node::text_with_marks(&text[from..to], marks.clone()));
                    }
                    if to == end {
                        break;
                    }
                }
                offset = end;
            }
            Piece::Math(source) => {
                out.push(math_node(NodeType::MathInline, source));
                offset += source.len() + 2;
            }
            Piece::DisplayMath(source) => {
                out.push(math_node(NodeType::MathInline, source));
                offset += source.len() + 4;
            }
        }
    }
    text.clear();
    segments.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(slice: Slice) -> Slice {
        convert_math(slice, NodeType::Doc)
    }

    #[test]
    fn splits_equations_out_of_text() {
        assert_eq!(
            split_math("where $m(t) \\ge 0$ holds"),
            vec![
                Piece::Text("where "),
                Piece::Math("m(t) \\ge 0"),
                Piece::Text(" holds")
            ]
        );
        assert_eq!(split_math("$x^2$"), vec![Piece::Math("x^2")]);
        assert_eq!(
            split_math("$a$ and $b$"),
            vec![Piece::Math("a"), Piece::Text(" and "), Piece::Math("b")]
        );
        assert_eq!(split_math("$$ x^2 $$"), vec![Piece::DisplayMath(" x^2 ")]);
        assert_eq!(
            split_math("see $$x$$ and $y$"),
            vec![
                Piece::Text("see "),
                Piece::DisplayMath("x"),
                Piece::Text(" and "),
                Piece::Math("y")
            ]
        );
    }

    #[test]
    fn dollar_amounts_escapes_and_invalid_math_stay_text() {
        for s in [
            "costs $5 and $10",
            "a lone $ sign",
            "\\$x$ is escaped",
            "$ spaced$",
            "$trailing $",
            "$5$10",
            "$$",
            "$$$$",
            "$$$x$$$",
            "\\$$x$$",
            "$\\foo{$ does not parse",
            "$$\\foo{$$",
        ] {
            assert_eq!(split_math(s), vec![Piece::Text(s)], "{s}");
        }
    }

    #[test]
    fn converts_text_runs_but_not_code() {
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![
                Node::text("see $x^2$ "),
                Node::text_with_marks("$y$", vec![Mark::new(MarkType::Code)]),
            ]),
        );
        let code = Node::element_with_content(
            NodeType::CodeBlock,
            Fragment::from(vec![Node::text("echo $$a$$")]),
        );
        let out = convert(Slice::new(Fragment::from(vec![para, code]), 0, 0));
        let Node::Element { content, .. } = &out.content.children[0] else {
            panic!()
        };
        assert_eq!(content.children.len(), 4, "{content:?}");
        assert_eq!(content.children[1].node_type(), Some(NodeType::MathInline));
        assert!(matches!(&content.children[3], Node::Text { text, .. } if text == "$y$"));
        assert_eq!(out.content.children[1].text_content(), "echo $$a$$");
    }

    #[test]
    fn recognizes_display_math_across_marks_and_keeps_surrounding_marks() {
        let bold = vec![Mark::new(MarkType::Bold)];
        let italic = vec![Mark::new(MarkType::Italic)];
        let out = convert(Slice::new(
            Fragment::from(vec![
                Node::text_with_marks("😀 $$x", bold.clone()),
                Node::text_with_marks("^2$$ fin", italic.clone()),
            ]),
            0,
            0,
        ));
        let kids = &out.content.children;
        assert_eq!(kids.len(), 3);
        assert!(matches!(&kids[0], Node::Text { text, marks } if text == "😀 " && *marks == bold));
        assert_eq!(kids[1].node_type(), Some(NodeType::MathInline));
        assert_eq!(kids[1].attrs().get("source").unwrap(), "x^2");
        assert!(
            matches!(&kids[2], Node::Text { text, marks } if text == " fin" && *marks == italic)
        );
    }

    #[test]
    fn standalone_display_math_is_a_block_only_in_supported_parents() {
        let paragraph = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![
                Node::text(" $$\n"),
                Node::text_with_marks("x^2", vec![Mark::new(MarkType::Bold)]),
                Node::element(NodeType::HardBreak),
                Node::text("$$ "),
            ]),
        );
        let list = Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![paragraph.clone()]),
            )]),
        );
        let out = convert(Slice::new(Fragment::from(vec![paragraph, list]), 0, 0));
        assert_eq!(
            out.content.children[0].node_type(),
            Some(NodeType::MathBlock)
        );
        let item = out.content.children[1].child(0).unwrap();
        assert_eq!(
            item.child(0).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
        assert!(default_schema().validate(&out.content.children[1]).is_ok());
    }

    #[test]
    fn code_marks_and_atoms_break_math_runs() {
        let content = Fragment::from(vec![
            Node::text("$$x"),
            Node::text_with_marks("+y", vec![Mark::new(MarkType::Code)]),
            Node::text("$$"),
        ]);
        let out = convert(Slice::new(content, 0, 0));
        assert_eq!(out.content.children.len(), 3);
        assert!(
            out.content
                .children
                .iter()
                .all(|n| n.node_type() != Some(NodeType::MathInline))
        );
    }
}
