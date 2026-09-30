// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Pasted `$…$` becomes an inline equation.
//!
//! The `$…$` input rule only fires on typing, so text pasted from anywhere
//! (a web page, a chat, markdown source) kept its dollar signs. This pass
//! runs over every pasted slice and applies the same Pandoc-style guards
//! as the input rule — the opening `$` is not followed by a space, the
//! closing one is not preceded by a space or followed by a digit, and `\$`
//! is literal — then converts only a source that actually parses, so
//! `$5 and $10` or a stray dollar stays plain text. Code (code blocks and
//! code-marked text) is never touched.

use super::model::{Fragment, MarkType, Node, NodeType, Slice};

/// A piece of a text run: literal text, or an equation's LaTeX source.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Piece<'a> {
    Text(&'a str),
    Math(&'a str),
}

/// Split `text` around its `$…$` equations (sources that parse).
pub(crate) fn split_inline_math(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut literal_from = 0;
    let mut i = 0;
    while let Some(off) = text[i..].find('$') {
        let open = i + off;
        i = open + 1;
        let before = text[..open].chars().next_back();
        let after = text[open + 1..].chars().next();
        if matches!(before, Some('\\' | '$')) || after.is_none_or(|c| c.is_whitespace() || c == '$') {
            continue;
        }
        let Some(close) = closing_dollar(text, open + 1) else { continue };
        let source = &text[open + 1..close];
        if ogrenotes_math::to_mathml(source, ogrenotes_math::Display::Inline).is_err() {
            continue;
        }
        if open > literal_from {
            out.push(Piece::Text(&text[literal_from..open]));
        }
        out.push(Piece::Math(source));
        literal_from = close + 1;
        i = close + 1;
    }
    if literal_from < text.len() {
        out.push(Piece::Text(&text[literal_from..]));
    }
    out
}

/// The first `$` at or after `from` that can close an equation: not
/// escaped, not preceded by whitespace, not followed by a digit or `$`.
fn closing_dollar(text: &str, from: usize) -> Option<usize> {
    let mut j = from;
    while let Some(off) = text[j..].find('$') {
        let at = j + off;
        j = at + 1;
        let before = text[..at].chars().next_back();
        let after = text[at + 1..].chars().next();
        if at == from || before.is_some_and(|c| c.is_whitespace() || c == '\\') {
            continue;
        }
        if after.is_some_and(|c| c.is_ascii_digit() || c == '$') {
            continue;
        }
        return Some(at);
    }
    None
}

/// Replace `$…$` in every non-code text run of `slice` with inline
/// equation nodes.
pub fn convert_inline_math(slice: Slice) -> Slice {
    if !contains_dollar(&slice.content) {
        return slice;
    }
    Slice::new(convert_fragment(slice.content), slice.open_start, slice.open_end)
}

fn contains_dollar(f: &Fragment) -> bool {
    f.children.iter().any(|n| match n {
        Node::Text { text, .. } => text.contains('$'),
        Node::Element { content, .. } => contains_dollar(content),
    })
}

fn convert_fragment(f: Fragment) -> Fragment {
    let mut out = Vec::with_capacity(f.children.len());
    for node in f.children {
        match node {
            Node::Text { text, marks } if !marks.iter().any(|m| m.mark_type == MarkType::Code) && text.contains('$') => {
                for piece in split_inline_math(&text) {
                    match piece {
                        Piece::Text(t) => out.push(Node::Text { text: t.to_string(), marks: marks.clone() }),
                        Piece::Math(src) => {
                            let mut attrs = std::collections::HashMap::new();
                            attrs.insert("source".to_string(), src.to_string());
                            out.push(Node::element_with_attrs(NodeType::MathInline, attrs, Fragment::empty()));
                        }
                    }
                }
            }
            Node::Element { node_type, attrs, content, marks } if node_type != NodeType::CodeBlock => {
                out.push(Node::Element { node_type, attrs, content: convert_fragment(content), marks });
            }
            other => out.push(other),
        }
    }
    Fragment::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::Mark;

    #[test]
    fn splits_equations_out_of_text() {
        assert_eq!(
            split_inline_math("where $m(t) \\ge 0$ holds"),
            vec![Piece::Text("where "), Piece::Math("m(t) \\ge 0"), Piece::Text(" holds")]
        );
        assert_eq!(split_inline_math("$x^2$"), vec![Piece::Math("x^2")]);
        assert_eq!(
            split_inline_math("$a$ and $b$"),
            vec![Piece::Math("a"), Piece::Text(" and "), Piece::Math("b")]
        );
    }

    #[test]
    fn dollar_amounts_and_escapes_stay_text() {
        for s in [
            "costs $5 and $10",
            "a lone $ sign",
            "\\$x$ is escaped",
            "$ spaced$",
            "$trailing $",
            "$5$10",
            "$$",
            "$\\foo{$ does not parse",
        ] {
            assert_eq!(split_inline_math(s), vec![Piece::Text(s)], "{s}");
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
        let code = Node::element_with_content(NodeType::CodeBlock, Fragment::from(vec![Node::text("echo $a$")]));
        let out = convert_inline_math(Slice::new(Fragment::from(vec![para, code]), 0, 0));
        let p = &out.content.children[0];
        let Node::Element { content, .. } = p else { panic!() };
        assert_eq!(content.children.len(), 4, "{content:?}");
        assert_eq!(content.children[1].node_type(), Some(NodeType::MathInline));
        assert!(matches!(&content.children[3], Node::Text { text, .. } if text == "$y$"));
        assert_eq!(out.content.children[1].text_content(), "echo $a$");
    }

    #[test]
    fn marks_carry_over_to_the_surrounding_text() {
        let bold = vec![Mark::new(MarkType::Bold)];
        let out = convert_inline_math(Slice::new(Fragment::from(vec![Node::text_with_marks("a $x$ b", bold.clone())]), 0, 0));
        let kids = &out.content.children;
        assert_eq!(kids.len(), 3);
        assert!(matches!(&kids[0], Node::Text { marks, .. } if *marks == bold));
        assert!(matches!(&kids[2], Node::Text { marks, .. } if *marks == bold));
    }
}
