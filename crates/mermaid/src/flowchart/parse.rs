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
    /// Open subgraphs: (subgraph index, opening line). Top of stack is
    /// the innermost currently-open subgraph.
    stack: Vec<(usize, usize)>,
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
        stack: Vec::new(),
    };
    let mut seen_header = false;
    for (idx, raw) in source.lines().enumerate() {
        p.line = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            // Mermaid's own docs use the `graph TD;` form (trailing `;`,
            // sometimes with statements chained after it on the same
            // line). Split the header line on `;` the same way a
            // statement line is split below: the first segment is the
            // header, any remaining non-empty segments are ordinary
            // statements on the same line.
            let mut segs = line.split(';');
            let header = segs.next().unwrap_or("").trim();
            p.parse_header(header)?;
            seen_header = true;
            for stmt in segs {
                let stmt = stmt.trim();
                if stmt.is_empty() {
                    continue;
                }
                p.parse_statement(stmt)?;
            }
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
    if let Some(&(idx, opening_line)) = p.stack.last() {
        return Err(ParseError {
            message: format!("unclosed subgraph `{}`", p.g.subgraphs[idx].id),
            line: Some(opening_line),
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
        let first = stmt.split_whitespace().next().unwrap_or("");
        match first {
            "subgraph" => return self.parse_subgraph_open(stmt),
            "end" if stmt == "end" => return self.parse_subgraph_end(),
            "classDef" => return self.parse_class_def(stmt),
            "class" => return self.parse_class_assign(stmt),
            "click" | "linkStyle" | "style" | "direction" => {
                return Err(self.err(format!("`{first}` statements are not supported")));
            }
            _ if stmt.starts_with("accTitle") || stmt.starts_with("accDescr") => {
                let kw = if stmt.starts_with("accTitle") { "accTitle" } else { "accDescr" };
                return Err(self.err(format!("`{kw}` statements are not supported")));
            }
            _ => {}
        }
        self.parse_chain(stmt)
    }

    fn parse_subgraph_open(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("subgraph").unwrap().trim();
        let (id, title) = if let Some(bracket_pos) = rest.find('[') {
            let id = rest[..bracket_pos].trim().to_string();
            let after = rest[bracket_pos + 1..].trim_end();
            let Some(body) = after.strip_suffix(']') else {
                return Err(self.err("unclosed `[` in subgraph title"));
            };
            let title = body.trim();
            let title = title
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(title);
            (id, title.to_string())
        } else {
            let whole = rest.trim().to_string();
            if whole.is_empty() {
                return Err(self.err("subgraph needs an id or title"));
            }
            (whole.clone(), whole)
        };
        if id.is_empty() {
            return Err(self.err("subgraph needs an id or title"));
        }
        let parent = self.stack.last().map(|&(i, _)| i);
        let idx = self.g.subgraphs.len();
        self.g.subgraphs.push(crate::flowchart::FlowSubgraph { id, title, parent });
        self.stack.push((idx, self.line));
        Ok(())
    }

    fn parse_subgraph_end(&mut self) -> Result<(), ParseError> {
        if self.stack.pop().is_none() {
            return Err(self.err("found `end` outside a subgraph"));
        }
        Ok(())
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
                    // Bail INSIDE the fan-out loop, not after it: an
                    // `a&a&...&a --> a&...&a` chain with N ids on each
                    // side pushes N*N edges before this loop would
                    // otherwise return, so at N=5000 (well under what a
                    // 20k-char source can spell) that's 25M `FlowEdge`
                    // allocations (~2.1GB RSS, measured) long before
                    // layout's `MAX_EDGES` validator ever runs. Checking
                    // after every push caps the work at MAX_EDGES + 1
                    // pushes regardless of how large the fan-out is.
                    if self.g.edges.len() > crate::layout::MAX_EDGES {
                        return Err(self.err(format!(
                            "diagram too large: too many edges (max {})",
                            crate::layout::MAX_EDGES
                        )));
                    }
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
        // char count == byte length ONLY because the predicate is
        // ASCII-only; do not relax without a byte-position scan.
        let id_len = r.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
        if id_len == 0 {
            return Err(self.err(format!("expected a node id, found {r:?}")));
        }
        let id: String = r[..id_len].to_string();
        let mut after = &r[id_len..];
        let shape_label = self.try_parse_bracket(&mut after)?;

        // Optional :::className suffix (after any bracket).
        let r2 = after.trim_start();
        let mut classes = Vec::new();
        let mut after2 = after;
        if let Some(rest_c) = r2.strip_prefix(":::") {
            let n = rest_c.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
            if n == 0 {
                return Err(self.err("expected a class name after `:::`"));
            }
            classes.push(rest_c[..n].to_string());
            after2 = &rest_c[n..];
        }
        *rest = after2;

        let idx = match self.ids.get(&id) {
            Some(&i) => i,
            None => {
                let i = self.g.nodes.len();
                self.g.nodes.push(FlowNode {
                    id: id.clone(),
                    label: id.clone(),
                    shape: ShapeKind::Rect,
                    classes: vec![],
                    subgraph: self.stack.last().map(|&(i, _)| i),
                });
                self.ids.insert(id, i);
                i
            }
        };
        if let Some((shape, label)) = shape_label {
            self.g.nodes[idx].shape = shape;
            self.g.nodes[idx].label = label;
        }
        self.g.nodes[idx].classes.extend(classes);
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

    /// The style allowlist is the CSS-injection boundary: only these
    /// properties, and only benign value characters, survive into the
    /// emitted `style` attribute. Everything else is dropped silently —
    /// styling is cosmetic and mermaid's style vocabulary is huge, so
    /// erroring here would be hostile to real-world diagrams.
    const STYLE_PROPS: &[&str] = &[
        "fill", "stroke", "stroke-width", "stroke-dasharray",
        "color", "font-weight", "font-style", "opacity",
    ];

    fn parse_class_def(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("classDef").unwrap().trim();
        let Some((name, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("classDef needs a name and styles"));
        };
        let mut kept = Vec::new();
        for pair in styles.split(',') {
            let Some((prop, value)) = pair.split_once(':') else { continue };
            let (prop, value) = (prop.trim(), value.trim());
            let value_ok = value.chars().all(|c| {
                c.is_ascii_alphanumeric() || " #.,%-".contains(c)
            });
            if Self::STYLE_PROPS.contains(&prop) && value_ok && !value.is_empty() {
                kept.push(format!("{prop}:{value}"));
            }
        }
        self.g.class_defs.push(crate::flowchart::ClassDef {
            name: name.to_string(),
            style: kept.join(";"),
        });
        Ok(())
    }

    /// `class n1,n2 name` — assigns an existing class name to a
    /// comma-separated list of already-defined node ids.
    fn parse_class_assign(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("class").unwrap().trim();
        // rsplit_once is char-boundary safe. (A manual `rfind + 1` slice
        // panics on multi-byte Unicode whitespace such as U+2003 em space:
        // rfind returns the START byte of the char, so `+ 1` lands inside
        // it.)
        let Some((node_list, class_name)) = rest.rsplit_once(char::is_whitespace) else {
            return Err(self.err("class needs a node list and a class name"));
        };
        let node_list = node_list.trim();
        let class_name = class_name.trim();
        if node_list.is_empty() || class_name.is_empty() {
            return Err(self.err("class needs a node list and a class name"));
        }
        for id in node_list.split(',') {
            let id = id.trim();
            let Some(&idx) = self.ids.get(id) else {
                return Err(self.err(format!("class refers to unknown node `{id}`")));
            };
            self.g.nodes[idx].classes.push(class_name.to_string());
        }
        Ok(())
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
    fn semicolon_terminated_header_alone() {
        let g = p("graph TD;");
        assert_eq!(g.direction, Direction::TB);
        assert!(g.nodes.is_empty());
    }

    #[test]
    fn semicolon_terminated_header_with_trailing_statement() {
        let g = p("graph LR; A-->B");
        assert_eq!(g.direction, Direction::LR);
        assert_eq!(g.edges.len(), 1);
        assert_eq!((g.edges[0].from, g.edges[0].to), (0, 1));
    }

    #[test]
    fn unknown_direction_still_errors_with_and_without_semicolon() {
        assert!(parse("graph XX").unwrap_err().message.contains("unknown direction"));
        let e = parse("graph XX;").unwrap_err();
        assert!(e.message.contains("unknown direction"), "got: {}", e.message);
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
    fn fanout_exceeding_edge_cap_errs_quickly() {
        // 200 ids on each side of `&` fan-out -> up to 200*200 = 40,000
        // candidate edge pushes if unchecked, but the in-loop cap check
        // must bail once `MAX_EDGES` (1000) is crossed, well before the
        // full cross product is built. The test proves "no blowup" simply
        // by completing in normal test time rather than by timing.
        let side: String = std::iter::repeat_n("a", 200).collect::<Vec<_>>().join("&");
        let src = format!("graph TD\n{side} --> {side}");
        let e = parse(&src).unwrap_err();
        assert!(e.message.contains("too many edges"), "got: {}", e.message);
        assert_eq!(e.line, Some(2));
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

    #[test]
    fn subgraph_membership_and_title() {
        let g = p("graph TD\nsubgraph one[Group One]\nA --> B\nend\nC --> A");
        assert_eq!(g.subgraphs.len(), 1);
        assert_eq!(g.subgraphs[0].title, "Group One");
        assert_eq!(g.nodes[0].subgraph, Some(0)); // A created inside
        assert_eq!(g.nodes[1].subgraph, Some(0)); // B created inside
        let c = g.nodes.iter().find(|n| n.id == "C").unwrap();
        assert_eq!(c.subgraph, None);
    }

    #[test]
    fn subgraph_without_bracket_title() {
        let g = p("graph TD\nsubgraph My Group\nA\nend");
        assert_eq!(g.subgraphs[0].title, "My Group");
        assert_eq!(g.subgraphs[0].id, "My Group");
    }

    #[test]
    fn nested_subgraphs() {
        let g = p("graph TD\nsubgraph outer\nsubgraph inner\nA\nend\nB\nend");
        assert_eq!(g.subgraphs.len(), 2);
        assert_eq!(g.subgraphs[1].parent, Some(0));
        assert_eq!(g.nodes[0].subgraph, Some(1)); // A in inner
        assert_eq!(g.nodes[1].subgraph, Some(0)); // B in outer
    }

    #[test]
    fn existing_node_does_not_move_into_subgraph() {
        let g = p("graph TD\nA\nsubgraph s\nA --> B\nend");
        assert_eq!(g.nodes[0].subgraph, None);
        assert_eq!(g.nodes[1].subgraph, Some(0));
    }

    #[test]
    fn end_without_subgraph_errors() {
        let e = parse("graph TD\nend").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn unclosed_subgraph_errors_at_opening_line() {
        let e = parse("graph TD\nA\nsubgraph s\nB").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("unclosed subgraph"));
    }

    #[test]
    fn class_def_and_assignment() {
        let g = p("graph TD\nA\nB\nclassDef hot fill:#f00,stroke-width:2px\nclass A,B hot");
        assert_eq!(g.class_defs.len(), 1);
        assert!(g.class_defs[0].style.contains("fill:#f00"));
        assert!(g.class_defs[0].style.contains("stroke-width:2px"));
        assert_eq!(g.nodes[0].classes, vec!["hot"]);
        assert_eq!(g.nodes[1].classes, vec!["hot"]);
    }

    #[test]
    fn inline_class_suffix() {
        let g = p("graph TD\nclassDef hot fill:#f00\nA[Hi]:::hot --> B");
        assert_eq!(g.nodes[0].classes, vec!["hot"]);
        assert!(g.nodes[1].classes.is_empty());
    }

    #[test]
    fn class_def_sanitizes_disallowed_properties() {
        let g = p("graph TD\nA\nclassDef bad background-image:url(x),fill:#0f0");
        // Disallowed property dropped; allowlisted one kept.
        assert!(!g.class_defs[0].style.contains("url"));
        assert!(g.class_defs[0].style.contains("fill:#0f0"));
    }

    #[test]
    fn class_def_rejects_hostile_value_chars() {
        let g = p("graph TD\nA\nclassDef x fill:#0f0;evil");
        // `;` splits statements, so `evil` becomes a separate (bare-node)
        // statement — fill survives, no injection into the style string.
        assert!(g.class_defs[0].style.contains("fill:#0f0"));
        assert!(!g.class_defs[0].style.contains("evil"));
    }

    #[test]
    fn out_of_scope_statements_error_with_line() {
        for stmt in ["click A callback", "linkStyle 0 stroke:red", "style A fill:#f00",
                     "accTitle: x", "accDescr: y", "direction LR"] {
            let src = format!("graph TD\nA\n{stmt}");
            let e = parse(&src).unwrap_err();
            assert_eq!(e.line, Some(3), "for {stmt}");
            let kw = stmt.split([' ', ':']).next().unwrap();
            assert!(e.message.contains(kw), "message names the keyword: {}", e.message);
        }
    }

    #[test]
    fn direction_inside_subgraph_errors() {
        let e = parse("graph TD\nsubgraph s\ndirection LR\nend").unwrap_err();
        assert_eq!(e.line, Some(3));
    }

    #[test]
    fn class_assign_multibyte_whitespace_no_panic() {
        // Multi-byte Unicode whitespace between node list and class name
        // must not panic (rfind returns the START byte of the char; naive
        // `+ 1` slicing lands mid-char). Ok or a clean line error are both
        // acceptable — panicking on untrusted input is not.
        for ws in ['\u{2003}' /* em space */, '\u{a0}' /* nbsp */] {
            let src = format!("graph TD\nA\nclass A{ws}hot");
            match parse(&src) {
                Ok(g) => {
                    let a = g.nodes.iter().find(|n| n.id == "A").unwrap();
                    assert_eq!(a.classes, vec!["hot"], "for {ws:?}");
                }
                Err(e) => assert!(e.line.is_some(), "clean error for {ws:?}"),
            }
        }
    }
}
