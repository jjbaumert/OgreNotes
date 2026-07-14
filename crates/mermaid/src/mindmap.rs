//! Mermaid `mindmap`: parser + left-to-right tidy-tree SVG renderer.
//!
//! Hierarchy comes from indentation: a line is a child of the nearest
//! previous line with less indentation. Node shapes reuse the flowchart
//! shape geometry. Mermaid draws mindmaps radially; we render a horizontal
//! tree (root at the left, children fanning right), which conveys the same
//! structure and is easy to read.

use crate::flowchart::{shapes, ShapeKind};
use crate::{escape_xml, measure, ParseError};

const MAX_NODES: usize = 300;

#[derive(Debug, Clone)]
pub(crate) struct MindNode {
    pub label: String,
    pub shape: ShapeKind,
    pub depth: usize,
    pub children: Vec<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Mindmap {
    pub nodes: Vec<MindNode>, // node 0 is the root
}

/// Classifies a node from its optional `id`-prefix + shape brackets,
/// returning the shape and inner label. `root((Root))` -> circle "Root";
/// `[square]` -> rect "square"; plain text (no bracket immediately after
/// an id) is a rounded node labelled with the whole text.
fn parse_node(s: &str) -> (ShapeKind, &str) {
    // An id may precede a shape bracket (`root((x))`), like flowchart node
    // ids. Only strip it when a bracket follows immediately — otherwise the
    // text (e.g. `Long history`) is a plain label.
    let id_len = s.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').count();
    let after = &s[id_len..];
    let body = if after.starts_with(['(', '[', '{', ')']) { after } else { s };

    let pairs: &[(&str, &str, ShapeKind)] = &[
        ("((", "))", ShapeKind::Circle),
        ("))", "((", ShapeKind::Rounded), // bang
        ("{{", "}}", ShapeKind::Hexagon),
        (")", "(", ShapeKind::Rounded), // cloud
        ("(", ")", ShapeKind::Rounded),
        ("[", "]", ShapeKind::Rect),
    ];
    for (open, close, shape) in pairs {
        if body.len() > open.len() + close.len()
            && body.starts_with(open)
            && body.ends_with(close)
        {
            return (*shape, body[open.len()..body.len() - close.len()].trim());
        }
    }
    (ShapeKind::Rounded, s)
}

/// Leading-whitespace width (tabs count as one), used only to compare
/// relative indentation between lines.
fn indent_of(raw: &str) -> usize {
    raw.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

pub(crate) fn parse(source: &str) -> Result<Mindmap, ParseError> {
    let mut nodes: Vec<MindNode> = Vec::new();
    // Stack of (indent, node_index) for the current ancestor chain.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with("%%") {
            continue;
        }
        if !seen_header {
            if trimmed != "mindmap" {
                return Err(ParseError {
                    message: "mindmap must start with `mindmap`".into(),
                    line: Some(line_no),
                });
            }
            seen_header = true;
            continue;
        }
        // Decorators that annotate the previous node; ignored.
        if trimmed.starts_with("::icon(") || trimmed.starts_with(":::") {
            continue;
        }

        let indent = indent_of(raw);
        let (shape, label) = parse_node(trimmed);
        // Parent = nearest node on the stack with strictly smaller indent.
        while let Some(&(top_indent, _)) = stack.last() {
            if top_indent >= indent {
                stack.pop();
            } else {
                break;
            }
        }
        let parent = stack.last().map(|&(_, i)| i);
        if parent.is_none() && !nodes.is_empty() {
            return Err(ParseError {
                message: "mindmap has more than one root node".into(),
                line: Some(line_no),
            });
        }
        if nodes.len() >= MAX_NODES {
            return Err(ParseError {
                message: format!("mindmap too large: more than {MAX_NODES} nodes"),
                line: Some(line_no),
            });
        }
        let depth = parent.map_or(0, |p| nodes[p].depth + 1);
        let ni = nodes.len();
        nodes.push(MindNode { label: label.to_string(), shape, depth, children: Vec::new() });
        if let Some(p) = parent {
            nodes[p].children.push(ni);
        }
        stack.push((indent, ni));
    }

    if !seen_header {
        return Err(ParseError { message: "mindmap must start with `mindmap`".into(), line: None });
    }
    if nodes.is_empty() {
        return Err(ParseError { message: "mindmap has no root node".into(), line: None });
    }
    Ok(Mindmap { nodes })
}

const MARGIN: f64 = 16.0;
const X_GAP: f64 = 180.0;
const Y_GAP: f64 = 40.0;
const NODE_H: f64 = 30.0;

/// Assigns each node a vertical slot: leaves take successive slots,
/// internal nodes centre on their children. Iterative-safe recursion
/// (depth bounded by `MAX_NODES`).
fn assign_slots(nodes: &[MindNode], node: usize, next_leaf: &mut f64, ys: &mut [f64]) {
    if nodes[node].children.is_empty() {
        ys[node] = *next_leaf;
        *next_leaf += 1.0;
    } else {
        for &c in &nodes[node].children {
            assign_slots(nodes, c, next_leaf, ys);
        }
        let first = ys[nodes[node].children[0]];
        let last = ys[*nodes[node].children.last().unwrap()];
        ys[node] = (first + last) / 2.0;
    }
}

pub(crate) fn render_svg(m: &Mindmap) -> String {
    let n = m.nodes.len();
    let mut slots = vec![0.0f64; n];
    let mut next = 0.0;
    assign_slots(&m.nodes, 0, &mut next, &mut slots);

    // Per-node box size from its label; column left edge from its depth.
    // Circle/double-circle nodes (e.g. `root((mindmap))`) must be sized to
    // circumscribe their text — a rectangle-sized box would clip the label —
    // so they use the shape-aware sizing; other shapes keep the row height.
    let sizes: Vec<(f64, f64)> = m
        .nodes
        .iter()
        .map(|node| {
            let (tw, th) = measure::text_size(&node.label);
            match node.shape {
                ShapeKind::Circle | ShapeKind::DoubleCircle => shapes::size_for(node.shape, tw, th),
                _ => ((tw + 28.0).max(48.0), NODE_H),
            }
        })
        .collect();
    let col_left = |depth: usize| MARGIN + depth as f64 * X_GAP;
    let cx = |i: usize| col_left(m.nodes[i].depth) + sizes[i].0 / 2.0;
    let cy_raw = |i: usize| MARGIN + NODE_H / 2.0 + slots[i] * Y_GAP;

    // A node taller than the row height (a big root circle) can reach above the
    // top margin; shift everything down so the highest node clears the margin.
    let y_top = (0..n).map(|i| cy_raw(i) - sizes[i].1 / 2.0).fold(f64::INFINITY, f64::min);
    let y_shift = (MARGIN - y_top).max(0.0);
    let cy = |i: usize| cy_raw(i) + y_shift;

    // Canvas width = the furthest right edge over ALL nodes (a wide label
    // on a shallow node can reach past the deepest column), plus a margin.
    let w = (0..n)
        .map(|i| col_left(m.nodes[i].depth) + sizes[i].0)
        .fold(0.0_f64, f64::max)
        + MARGIN;
    // Height from the lowest node extent (tall circles included), not just rows.
    let h = (0..n).map(|i| cy(i) + sizes[i].1 / 2.0).fold(0.0_f64, f64::max) + MARGIN;
    let _ = next; // row count no longer bounds the height directly

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:13px">"#
    );

    // Edges (under nodes): parent's right edge -> child's left edge,
    // curved along the horizontal flow.
    for (i, node) in m.nodes.iter().enumerate() {
        let px = col_left(node.depth) + sizes[i].0;
        let py = cy(i);
        for &c in &node.children {
            let clx = col_left(m.nodes[c].depth);
            let d = crate::curved_path(&[(px, py), (clx, cy(c))], false);
            svg.push_str(&format!(
                r#"<path d="{d}" stroke="currentColor" fill="none" opacity="0.5"/>"#
            ));
        }
    }

    // Nodes.
    for (i, node) in m.nodes.iter().enumerate() {
        let (nw, nh) = sizes[i];
        svg.push_str("<g>");
        svg.push_str(&shapes::emit(node.shape, cx(i), cy(i), nw, nh, None));
        svg.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">{}</text>"#,
            cx(i),
            cy(i) + 4.0,
            escape_xml(&node.label),
        ));
        svg.push_str("</g>");
    }

    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> Mindmap {
        parse(src).expect("parse ok")
    }





    #[test]
    fn header_required() {
        assert!(parse("root").is_err());
        assert!(parse("mindmap\n  root").is_ok());
    }

    #[test]
    fn indentation_builds_tree() {
        // Explicit spaces (no `\` line-continuation, which would strip the
        // leading indentation this test depends on).
        let g = p("mindmap\n  root((Root))\n    Origins\n      Long history\n    Research\n    Tools\n      Pen\n      Mermaid");
        // root has 3 children: Origins, Research, Tools
        assert_eq!(g.nodes[0].label, "Root");
        assert_eq!(g.nodes[0].shape, ShapeKind::Circle);
        assert_eq!(g.nodes[0].children.len(), 3);
        // Origins has one child (Long history) at depth 2.
        let origins = g.nodes[0].children[0];
        assert_eq!(g.nodes[origins].label, "Origins");
        assert_eq!(g.nodes[origins].children.len(), 1);
        assert_eq!(g.nodes[g.nodes[origins].children[0]].depth, 2);
        // Tools has two children.
        let tools = g.nodes[0].children[2];
        assert_eq!(g.nodes[tools].label, "Tools");
        assert_eq!(g.nodes[tools].children.len(), 2);
    }

    #[test]
    fn root_circle_is_sized_to_fit_its_label() {
        // Regression: a `((wide label))` root used to render in a tiny circle
        // whose radius came from the fixed row height, so the text overflowed.
        // The circle must now circumscribe the label.
        let svg = render_svg(&p("mindmap\n  root((Understanding))\n    A\n    B"));
        let label_w = measure::text_size("Understanding").0;
        // Pull the first <circle r="..."> radius out of the SVG.
        let r: f64 = svg
            .split(r#"<circle"#)
            .nth(1)
            .and_then(|s| s.split(r#"r="#).nth(1))
            .and_then(|s| s.trim_start_matches('"').split('"').next())
            .and_then(|s| s.parse().ok())
            .expect("a circle with a radius");
        assert!(2.0 * r >= label_w, "circle diameter {} must cover label width {label_w}", 2.0 * r);
    }

    #[test]
    fn shapes_parse() {
        let g = p("mindmap\n  r\n    [square]\n    (round)\n    ((circle))\n    {{hex}}\n    ))bang((\n    )cloud(");
        let kids: Vec<(ShapeKind, &str)> =
            g.nodes[0].children.iter().map(|&c| (g.nodes[c].shape, g.nodes[c].label.as_str())).collect();
        assert_eq!(kids[0], (ShapeKind::Rect, "square"));
        assert_eq!(kids[1], (ShapeKind::Rounded, "round"));
        assert_eq!(kids[2], (ShapeKind::Circle, "circle"));
        assert_eq!(kids[3], (ShapeKind::Hexagon, "hex"));
        assert_eq!(kids[4].1, "bang");
        assert_eq!(kids[5].1, "cloud");
    }

    #[test]
    fn icon_and_class_decorators_ignored() {
        let g = p("mindmap\n  root\n    Origins\n    ::icon(fa fa-book)\n    :::urgent\n    Research");
        // Only Origins + Research are real children; decorators skipped.
        assert_eq!(g.nodes[0].children.len(), 2);
        assert_eq!(g.nodes[g.nodes[0].children[1]].label, "Research");
    }

    #[test]
    fn two_roots_error() {
        let e = parse("mindmap\nroot1\nroot2").unwrap_err();
        assert!(e.message.contains("more than one root"), "got: {}", e.message);
    }

    #[test]
    fn empty_errors() {
        assert!(parse("mindmap").unwrap_err().message.contains("no root"));
    }

    #[test]
    fn renders_nodes_and_edges() {
        let g = p("mindmap\n  root((Ideas))\n    A\n    B\n      C");
        let svg = render_svg(&g);
        assert!(svg.starts_with("<svg") && svg.contains("</svg>"));
        assert!(svg.contains("Ideas") && svg.contains("C"));
        assert!(svg.contains("<path"), "tree edges");
        assert_eq!(svg.matches("<text").count(), 4, "one label per node");
    }

    #[test]
    fn wide_shallow_node_widens_canvas() {
        let width = |src: &str| {
            let svg = render_svg(&p(src));
            svg.split("width=\"").nth(1).unwrap().split('"').next().unwrap().parse::<f64>().unwrap()
        };
        // Same tree shape; only the ROOT (depth 0) label width differs.
        // Under the old "deepest column only" width these would be equal and
        // the wide root would clip.
        let narrow = width("mindmap\n  R\n    child");
        let wide = width("mindmap\n  ((A very very very long root label))\n    child");
        assert!(wide > narrow, "wide root ({wide}) must widen the canvas vs narrow ({narrow})");
    }

    #[test]
    fn markup_escaped() {
        let g = p("mindmap\n  root\n    [<x>]");
        let svg = render_svg(&g);
        assert!(!svg.contains("<x>"));
        assert!(svg.contains("&lt;x&gt;"));
    }

    #[test]
    fn node_cap_enforced() {
        let mut src = String::from("mindmap\n  root\n");
        for i in 0..=MAX_NODES {
            src.push_str(&format!("    n{i}\n"));
        }
        assert!(parse(&src).unwrap_err().message.contains("too large"));
    }
}
