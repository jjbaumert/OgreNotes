// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Convert pasted dollar and TeX-delimited equations, preserving surrounding
//! formatting. Standalone display equations become blocks where the schema
//! allows them; equations amid prose remain inline. Code stays literal.

use super::model::{Fragment, Mark, MarkType, Node, NodeType, Slice};
use super::schema::{Schema, default_schema};

#[derive(Debug, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Math(&'a str),
    DisplayMath(&'a str),
    TexMath(&'a str),
    TexDisplayMath(&'a str),
}

/// Raw TeX pairs, used before Markdown can unescape their delimiters.
#[derive(Debug)]
pub(super) struct TexMathSpan<'a> {
    pub range: std::ops::Range<usize>,
    pub source: &'a str,
    pub display: bool,
}

pub(super) fn tex_math_spans(text: &str) -> Vec<TexMathSpan<'_>> {
    let mut spans = Vec::new();
    let mut opener = None;
    let mut slash_run = 0;
    for (offset, ch) in text.char_indices() {
        if ch == '\\' {
            slash_run += 1;
            continue;
        }
        if slash_run % 2 == 1 {
            match ch {
                '(' | '[' => opener = Some((offset - 1, ch == '[')),
                ')' | ']' => {
                    if let Some((start, display)) = opener
                        && display == (ch == ']')
                    {
                        spans.push(TexMathSpan {
                            range: start..offset + 1,
                            source: &text[start + 2..offset - 1],
                            display,
                        });
                        opener = None;
                    }
                }
                _ => {}
            }
        }
        slash_run = 0;
    }
    spans
}

fn url_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some((start, end)) = super::markdown::find_bare_url(text, from) {
        ranges.push(start..end);
        from = end;
    }
    ranges
}

fn inside_url(position: usize, ranges: &[std::ops::Range<usize>]) -> bool {
    ranges
        .partition_point(|range| range.start <= position)
        .checked_sub(1)
        .is_some_and(|index| ranges[index].contains(&position))
}

fn split_math(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut from = 0;
    let urls = url_ranges(text);
    for span in tex_math_spans(text) {
        if inside_url(span.range.start, &urls) {
            continue;
        }
        out.extend(split_dollar_math(&text[from..span.range.start]));
        let mode = if span.display {
            ogrenotes_math::Display::Block
        } else {
            ogrenotes_math::Display::Inline
        };
        out.push(if ogrenotes_math::to_mathml(span.source, mode).is_ok() {
            if span.display {
                Piece::TexDisplayMath(span.source)
            } else {
                Piece::TexMath(span.source)
            }
        } else {
            Piece::Text(&text[span.range.clone()])
        });
        from = span.range.end;
    }
    out.extend(split_dollar_math(&text[from..]));
    out
}

fn split_dollar_math(text: &str) -> Vec<Piece<'_>> {
    let urls = if text.contains('$') {
        url_ranges(text)
    } else {
        Vec::new()
    };
    let mut out = Vec::new();
    let mut literal_from = 0;
    let mut i = 0;
    while let Some(off) = text[i..].find('$') {
        let open = i + off;
        i = open + 1;
        if text[..open].ends_with(['\\', '$']) || inside_url(open, &urls) {
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextMath {
    Recognize,
    Preserve,
}

pub fn convert_math(slice: Slice, destination: NodeType) -> Slice {
    convert_slice(slice, destination, TextMath::Recognize)
}

/// Markdown already interpreted escapes, code and math. Keep its text literal
/// while placing existing display nodes in a schema-compatible destination.
pub(super) fn adapt_markdown_math(slice: Slice, destination: NodeType) -> Slice {
    convert_slice(slice, destination, TextMath::Preserve)
}

fn convert_slice(slice: Slice, destination: NodeType, text_math: TextMath) -> Slice {
    if !has_math_candidate(&slice.content, text_math) {
        return slice;
    }
    let schema = default_schema();
    Slice::new(
        convert_fragment(slice.content, destination, &schema, text_math),
        slice.open_start,
        slice.open_end,
    )
}

fn has_math_candidate(f: &Fragment, text_math: TextMath) -> bool {
    f.children.iter().any(|n| match n {
        Node::Text { text, .. } => text_math == TextMath::Recognize && text.contains(['$', '\\']),
        Node::Element {
            node_type, content, ..
        } => *node_type == NodeType::MathBlock || has_math_candidate(content, text_math),
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
    let text = text.trim();
    if !text.starts_with("$$") && !text.starts_with(r"\[") {
        return None;
    }
    match split_math(text).as_slice() {
        [Piece::DisplayMath(source) | Piece::TexDisplayMath(source)] => {
            Some(source.trim().to_string())
        }
        _ => None,
    }
}

fn convert_fragment(
    f: Fragment,
    parent: NodeType,
    schema: &Schema,
    text_math: TextMath,
) -> Fragment {
    let block_allowed = schema
        .node_spec(parent)
        .is_some_and(|s| s.valid_children.contains(&NodeType::MathBlock));
    // Native browser cuts can provide bare formatting spans without a paragraph.
    if text_math == TextMath::Recognize
        && block_allowed
        && let Some(source) = standalone_display(&f)
    {
        return Fragment::from(vec![math_node(NodeType::MathBlock, &source)]);
    }
    let mut out = Vec::with_capacity(f.children.len());
    let mut text = String::new();
    let mut segments = Vec::new();
    for node in f.children {
        if let Node::Text { text: t, marks } = &node
            && text_math == TextMath::Recognize
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
                    && text_math == TextMath::Recognize
                    && let Some(source) = standalone_display(&content)
                {
                    out.push(if block_allowed {
                        math_node(NodeType::MathBlock, &source)
                    } else {
                        Node::Element {
                            node_type,
                            attrs,
                            content: Fragment::from(vec![math_node(NodeType::MathInline, &source)]),
                            marks,
                        }
                    });
                } else {
                    out.push(Node::Element {
                        node_type,
                        attrs,
                        content: convert_fragment(content, node_type, schema, text_math),
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
            Piece::DisplayMath(source) | Piece::TexMath(source) | Piece::TexDisplayMath(source) => {
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
    fn tex_delimiters_keep_exact_sources_and_surrounding_unicode() {
        assert_eq!(
            split_math(r"😀 \(K_{P}\) and \[x^2\] fin"),
            vec![
                Piece::Text("😀 "),
                Piece::TexMath("K_{P}"),
                Piece::Text(" and "),
                Piece::TexDisplayMath("x^2"),
                Piece::Text(" fin"),
            ]
        );
        for text in [r"\\(x\\)", r"\(\unknowncommand\)", r"\(x", r"\[x\)"] {
            assert_eq!(split_math(text), vec![Piece::Text(text)], "{text}");
        }
        let spans = tex_math_spans(r"\(x\\) + y\)");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].source, r"x\\) + y");
    }

    #[test]
    fn full_academic_passage_converts_all_thirty_two_equations() {
        let text = include_str!("../../tests/fixtures/academic-math-paste.txt");
        let expected = tex_math_spans(text);
        assert_eq!(expected.len(), 32);
        let out = convert(Slice::new(Fragment::from(vec![Node::text(text)]), 0, 0));
        let sources: Vec<_> = out
            .content
            .children
            .iter()
            .filter(|node| node.node_type() == Some(NodeType::MathInline))
            .map(|node| node.attrs().get("source").unwrap().clone())
            .collect();
        assert_eq!(
            sources,
            expected
                .iter()
                .map(|span| span.source.trim().to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn tex_equation_across_marks_preserves_prose_and_code() {
        let bold = vec![Mark::new(MarkType::Bold)];
        let italic = vec![Mark::new(MarkType::Italic)];
        let code = vec![Mark::new(MarkType::Code)];
        let out = convert(Slice::new(
            Fragment::from(vec![
                Node::text_with_marks(r"😀 before ", bold.clone()),
                Node::text_with_marks(r"\", bold.clone()),
                Node::text_with_marks(r"(K_{P}\) after ", italic.clone()),
                Node::text_with_marks(r"\(literal\)", code.clone()),
            ]),
            0,
            0,
        ));
        let kids = &out.content.children;
        assert_eq!(kids[0], Node::text_with_marks("😀 before ", bold));
        assert_eq!(kids[1].node_type(), Some(NodeType::MathInline));
        assert_eq!(kids[1].attrs().get("source").unwrap(), "K_{P}");
        assert_eq!(kids[2], Node::text_with_marks(" after ", italic));
        assert_eq!(kids[3], Node::text_with_marks(r"\(literal\)", code));
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
