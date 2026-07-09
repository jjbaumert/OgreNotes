//! Flowchart parser. Line-oriented; each line splits on `;` into
//! statements; chain statements are scanned left-to-right with
//! longest-first token matching (std only, no regex).

use crate::flowchart::{EdgeKind, FlowEdge, FlowGraph, FlowNode, ShapeKind};
use crate::layout::Direction;
use crate::ParseError;
use std::collections::HashMap;

struct Parser {
    g: FlowGraph,
    ids: HashMap<String, usize>,
    line: usize, // 1-based, for errors
}

pub(crate) fn parse(source: &str) -> Result<FlowGraph, ParseError> {
    let mut p = Parser {
        g: FlowGraph {
            direction: Direction::TB,
            nodes: vec![],
            edges: vec![],
            subgraphs: vec![],
            class_defs: vec![],
        },
        ids: HashMap::new(),
        line: 0,
    };
    let mut seen_header = false;
    for (idx, raw) in source.lines().enumerate() {
        p.line = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            p.parse_header(line)?;
            seen_header = true;
            continue;
        }
        for stmt in line.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            p.parse_statement(stmt)?;
        }
    }
    if !seen_header {
        return Err(ParseError {
            message: "flowchart must start with `graph` or `flowchart`".into(),
            line: Some(1),
        });
    }
    Ok(p.g)
}

impl Parser {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { message: msg.into(), line: Some(self.line) }
    }

    fn parse_header(&mut self, line: &str) -> Result<(), ParseError> {
        let mut toks = line.split_whitespace();
        match toks.next() {
            Some("graph") | Some("flowchart") => {}
            _ => return Err(self.err("flowchart must start with `graph` or `flowchart`")),
        }
        self.g.direction = match toks.next() {
            None | Some("TD") | Some("TB") => Direction::TB,
            Some("BT") => Direction::BT,
            Some("LR") => Direction::LR,
            Some("RL") => Direction::RL,
            Some(other) => return Err(self.err(format!("unknown direction {other:?}"))),
        };
        Ok(())
    }

    fn parse_statement(&mut self, stmt: &str) -> Result<(), ParseError> {
        // Task 11 adds keyword statements here (subgraph/end/classDef/
        // class + explicit out-of-scope errors). Task 10: chains only.
        self.parse_chain(stmt)
    }

    /// `noderef (edgeop noderef)*` where noderef = group ('&' group)*.
    fn parse_chain(&mut self, stmt: &str) -> Result<(), ParseError> {
        let mut rest = stmt;
        let mut lhs = self.parse_node_group(&mut rest)?;
        loop {
            let r = rest.trim_start();
            if r.is_empty() {
                return Ok(());
            }
            rest = r;
            let (kind, label) = self.parse_edge_op(&mut rest)?;
            let rhs = self.parse_node_group(&mut rest)?;
            for &f in &lhs {
                for &t in &rhs {
                    self.g.edges.push(FlowEdge { from: f, to: t, kind, label: label.clone() });
                }
            }
            lhs = rhs;
        }
    }

    /// One or more `id bracket?` joined by `&`.
    fn parse_node_group(&mut self, rest: &mut &str) -> Result<Vec<usize>, ParseError> {
        let mut out = vec![self.parse_node_ref(rest)?];
        loop {
            let r = rest.trim_start();
            if let Some(after) = r.strip_prefix('&') {
                *rest = after.trim_start();
                out.push(self.parse_node_ref(rest)?);
            } else {
                *rest = r;
                return Ok(out);
            }
        }
    }

    fn parse_node_ref(&mut self, rest: &mut &str) -> Result<usize, ParseError> {
        let r = rest.trim_start();
        let id_len = r.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
        if id_len == 0 {
            return Err(self.err(format!("expected a node id, found {r:?}")));
        }
        let id: String = r[..id_len].to_string();
        let mut after = &r[id_len..];
        let shape_label = self.try_parse_bracket(&mut after)?;
        *rest = after;
        let idx = match self.ids.get(&id) {
            Some(&i) => i,
            None => {
                let i = self.g.nodes.len();
                self.g.nodes.push(FlowNode {
                    id: id.clone(),
                    label: id.clone(),
                    shape: ShapeKind::Rect,
                    classes: vec![],
                    subgraph: None, // Task 11 sets from subgraph stack
                });
                self.ids.insert(id, i);
                i
            }
        };
        if let Some((shape, label)) = shape_label {
            self.g.nodes[idx].shape = shape;
            self.g.nodes[idx].label = label;
        }
        Ok(idx)
    }

    /// Longest-first bracket match. Returns None if no opener follows.
    fn try_parse_bracket(
        &self,
        rest: &mut &str,
    ) -> Result<Option<(ShapeKind, String)>, ParseError> {
        // (opener, &[(closer, shape)]) — closers tried longest-first.
        const TABLE: &[(&str, &[(&str, ShapeKind)])] = &[
            ("(((", &[(")))", ShapeKind::DoubleCircle)]),
            ("((", &[("))", ShapeKind::Circle)]),
            ("([", &[("])", ShapeKind::Stadium)]),
            ("[(", &[(")]", ShapeKind::Cylinder)]),
            ("[/", &[("/]", ShapeKind::Parallelogram), ("\\]", ShapeKind::Trapezoid)]),
            ("[\\", &[("\\]", ShapeKind::ParallelogramRev), ("/]", ShapeKind::TrapezoidRev)]),
            ("{{", &[("}}", ShapeKind::Hexagon)]),
            ("{", &[("}", ShapeKind::Diamond)]),
            ("[", &[("]", ShapeKind::Rect)]),
            ("(", &[(")", ShapeKind::Rounded)]),
            (">", &[("]", ShapeKind::Flag)]),
        ];
        for (opener, closers) in TABLE {
            if let Some(body_start) = rest.strip_prefix(opener) {
                // Quoted label: `"..."` may contain characters that look
                // like closers (e.g. `A["has [brackets] inside"]`), so the
                // earliest-occurrence scan below would misfire. Handle the
                // quote specially: the label runs to the next `"`, and the
                // real closer must immediately follow (after whitespace).
                if let Some(after_open_quote) = body_start.trim_start().strip_prefix('"') {
                    if let Some(qi) = after_open_quote.find('"') {
                        let label = &after_open_quote[..qi];
                        let after_quote = after_open_quote[qi + 1..].trim_start();
                        for (closer, shape) in *closers {
                            if let Some(rem) = after_quote.strip_prefix(closer) {
                                *rest = rem;
                                return Ok(Some((*shape, label.to_string())));
                            }
                        }
                        return Err(
                            self.err(format!("unclosed {opener:?} bracket after quoted label"))
                        );
                    }
                }
                // Find the EARLIEST closer occurrence among this opener's
                // closers; the closer that matches at that position wins.
                let mut best: Option<(usize, &str, ShapeKind)> = None;
                for (closer, shape) in *closers {
                    if let Some(i) = body_start.find(closer) {
                        if best.map_or(true, |(bi, _, _)| i < bi) {
                            best = Some((i, closer, *shape));
                        }
                    }
                }
                let Some((i, closer, shape)) = best else {
                    return Err(self.err(format!("unclosed {opener:?} bracket")));
                };
                let body = &body_start[..i];
                *rest = &body_start[i + closer.len()..];
                let label = body.trim();
                let label = label
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(label);
                return Ok(Some((shape, label.to_string())));
            }
        }
        Ok(None)
    }

    /// Edge operator with optional label (inline or |pipe| form).
    fn parse_edge_op(&mut self, rest: &mut &str) -> Result<(EdgeKind, Option<String>), ParseError> {
        let r = rest.trim_start();
        // Inline label forms first: `-- text -->`, `-. text .->`, `== text ==>`.
        for (open, close, kind) in [
            ("--", "-->", EdgeKind::Arrow),
            ("-.", ".->", EdgeKind::Dotted),
            ("==", "==>", EdgeKind::Thick),
        ] {
            if let Some(after_open) = r.strip_prefix(open) {
                // Inline form requires a space after the opener (else it's
                // the plain operator like `-->` sharing the prefix).
                if after_open.starts_with(' ') {
                    if let Some(i) = after_open.find(close) {
                        let label = after_open[..i].trim().to_string();
                        *rest = &after_open[i + close.len()..];
                        return Ok((kind, Some(label)));
                    }
                }
            }
        }
        // Plain operators, longest first.
        for (op, kind) in [
            ("-.->", EdgeKind::Dotted),
            ("-.-", EdgeKind::Dotted),
            ("==>", EdgeKind::Thick),
            ("===", EdgeKind::Thick),
            ("-->", EdgeKind::Arrow),
            ("---", EdgeKind::Open),
        ] {
            if let Some(after) = r.strip_prefix(op) {
                let mut rest2 = after;
                // Optional |label|.
                let label = {
                    let r2 = rest2.trim_start();
                    if let Some(after_pipe) = r2.strip_prefix('|') {
                        let Some(i) = after_pipe.find('|') else {
                            return Err(self.err("unclosed `|` edge label"));
                        };
                        let l = after_pipe[..i].trim().to_string();
                        rest2 = &after_pipe[i + 1..];
                        Some(l)
                    } else {
                        None
                    }
                };
                *rest = rest2;
                return Ok((kind, label));
            }
        }
        Err(self.err(format!("expected an edge (e.g. `-->`), found {r:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flowchart::{EdgeKind, ShapeKind};
    use crate::layout::Direction;

    fn p(src: &str) -> crate::flowchart::FlowGraph {
        parse(src).expect("parse ok")
    }

    #[test]
    fn header_directions() {
        assert_eq!(p("graph TD\nA").direction, Direction::TB);
        assert_eq!(p("graph TB\nA").direction, Direction::TB);
        assert_eq!(p("flowchart LR\nA").direction, Direction::LR);
        assert_eq!(p("graph RL\nA").direction, Direction::RL);
        assert_eq!(p("graph BT\nA").direction, Direction::BT);
        assert_eq!(p("graph\nA").direction, Direction::TB); // default
    }

    #[test]
    fn missing_header_is_line_error() {
        let e = parse("A --> B").unwrap_err();
        assert_eq!(e.line, Some(1));
    }

    #[test]
    fn all_shapes_parse() {
        let cases = [
            ("A[text]", ShapeKind::Rect),
            ("A(text)", ShapeKind::Rounded),
            ("A([text])", ShapeKind::Stadium),
            ("A((text))", ShapeKind::Circle),
            ("A(((text)))", ShapeKind::DoubleCircle),
            ("A{text}", ShapeKind::Diamond),
            ("A{{text}}", ShapeKind::Hexagon),
            ("A[/text/]", ShapeKind::Parallelogram),
            ("A[\\text\\]", ShapeKind::ParallelogramRev),
            ("A[/text\\]", ShapeKind::Trapezoid),
            ("A[\\text/]", ShapeKind::TrapezoidRev),
            ("A[(text)]", ShapeKind::Cylinder),
            ("A>text]", ShapeKind::Flag),
        ];
        for (src, want) in cases {
            let g = p(&format!("graph TD\n{src}"));
            assert_eq!(g.nodes[0].shape, want, "for {src}");
            assert_eq!(g.nodes[0].label, "text", "for {src}");
        }
    }

    #[test]
    fn bare_id_defaults_rect_with_id_label() {
        let g = p("graph TD\nfoo_1");
        assert_eq!(g.nodes[0].id, "foo_1");
        assert_eq!(g.nodes[0].label, "foo_1");
        assert_eq!(g.nodes[0].shape, ShapeKind::Rect);
    }

    #[test]
    fn quoted_label_strips_quotes() {
        let g = p("graph TD\nA[\"has [brackets] inside\"]");
        assert_eq!(g.nodes[0].label, "has [brackets] inside");
    }

    #[test]
    fn edge_kinds() {
        let cases = [
            ("A --> B", EdgeKind::Arrow),
            ("A --- B", EdgeKind::Open),
            ("A -.-> B", EdgeKind::Dotted),
            ("A ==> B", EdgeKind::Thick),
        ];
        for (src, want) in cases {
            let g = p(&format!("graph TD\n{src}"));
            assert_eq!(g.edges[0].kind, want, "for {src}");
        }
    }

    #[test]
    fn pipe_label() {
        let g = p("graph TD\nA -->|yes| B");
        assert_eq!(g.edges[0].label.as_deref(), Some("yes"));
    }

    #[test]
    fn inline_label() {
        let g = p("graph TD\nA -- no --> B");
        assert_eq!(g.edges[0].label.as_deref(), Some("no"));
        let g = p("graph TD\nA -. maybe .-> B");
        assert_eq!(g.edges[0].label.as_deref(), Some("maybe"));
        assert_eq!(g.edges[0].kind, EdgeKind::Dotted);
    }

    #[test]
    fn chains_create_all_edges() {
        let g = p("graph TD\nA --> B --> C --> D");
        assert_eq!(g.edges.len(), 3);
        assert_eq!(g.nodes.len(), 4);
        assert_eq!((g.edges[1].from, g.edges[1].to), (1, 2));
    }

    #[test]
    fn ampersand_fanout() {
        let g = p("graph TD\nA & B --> C");
        assert_eq!(g.edges.len(), 2);
        let g = p("graph TD\nA --> B & C");
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn inline_shape_in_chain() {
        let g = p("graph TD\nA[Start] --> B{Choice}");
        assert_eq!(g.nodes[1].shape, ShapeKind::Diamond);
    }

    #[test]
    fn later_bracket_updates_earlier_bare_ref() {
        let g = p("graph TD\nA --> B\nB{Decide}");
        assert_eq!(g.nodes[1].shape, ShapeKind::Diamond);
        assert_eq!(g.nodes[1].label, "Decide");
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn comments_blanks_and_semicolons() {
        let g = p("graph TD\n%% a comment\n\nA --> B; B --> C;");
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn self_loop_parses() {
        let g = p("graph TD\nA --> A");
        assert_eq!((g.edges[0].from, g.edges[0].to), (0, 0));
    }

    #[test]
    fn unclosed_bracket_is_line_error() {
        let e = parse("graph TD\nA[oops").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn garbage_after_node_is_line_error() {
        let e = parse("graph TD\nA[ok] ???").unwrap_err();
        assert_eq!(e.line, Some(2));
    }
}
