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

const MARGIN: f64 = 30.0;
const RING: f64 = 165.0; // radius added per depth level
const NODE_H: f64 = 28.0;
/// Root node fill (Mermaid draws the root as a large filled dark circle).
const ROOT_FILL: &str = "#2a2ad6";
// Text colors kept as consts so `#` never follows `"` in a raw string literal.
const TXT_WHITE: &str = "#fff";
const TXT_DARK: &str = "#222";
/// Per top-level-branch colors; the whole subtree inherits its branch's color.
const BRANCH_COLORS: &[&str] =
    &["#e6e64d", "#7ed321", "#9d6ef0", "#4dd0e1", "#ec4899", "#f5a623", "#4a90e2", "#b06ef0"];

/// Leaf count of the subtree at `node` (memoized), used for angular allocation.
fn leaf_count(nodes: &[MindNode], node: usize, memo: &mut [f64]) -> f64 {
    if memo[node] > 0.0 {
        return memo[node];
    }
    let lc = if nodes[node].children.is_empty() {
        1.0
    } else {
        nodes[node].children.iter().map(|&c| leaf_count(nodes, c, memo)).sum()
    };
    memo[node] = lc;
    lc
}

/// Radial placement: each node sits at `depth * RING` along the bisector of its
/// angular sector; a node's children split its sector in proportion to their
/// leaf counts. Positions are relative to the (0,0) root center.
fn assign_radial(
    nodes: &[MindNode],
    node: usize,
    a0: f64,
    a1: f64,
    depth: usize,
    pos: &mut [(f64, f64)],
    memo: &mut [f64],
) {
    let angle = (a0 + a1) / 2.0;
    let r = depth as f64 * RING;
    pos[node] = (r * angle.cos(), r * angle.sin());
    let total: f64 = nodes[node].children.iter().map(|&c| leaf_count(nodes, c, memo)).sum();
    let mut cur = a0;
    for &c in &nodes[node].children {
        let span = (a1 - a0) * leaf_count(nodes, c, memo) / total.max(1.0);
        assign_radial(nodes, c, cur, cur + span, depth + 1, pos, memo);
        cur += span;
    }
}

pub(crate) fn render_svg(m: &Mindmap) -> String {
    use std::f64::consts::PI;
    let n = m.nodes.len();
    let mut memo = vec![0.0f64; n];
    let mut pos = vec![(0.0f64, 0.0f64); n];
    assign_radial(&m.nodes, 0, -PI, PI, 0, &mut pos, &mut memo);

    // Each node's top-level branch (root-child ancestor) drives its color.
    let mut branch = vec![usize::MAX; n];
    for (k, &child) in m.nodes[0].children.iter().enumerate() {
        let mut stack = vec![child];
        while let Some(u) = stack.pop() {
            branch[u] = k;
            stack.extend(m.nodes[u].children.iter().copied());
        }
    }
    let color_of = |i: usize| -> &'static str {
        match branch[i] {
            usize::MAX => ROOT_FILL,
            k => BRANCH_COLORS[k % BRANCH_COLORS.len()],
        }
    };

    // Per-node half-extents (root circle circumscribes its label).
    let sizes: Vec<(f64, f64)> = m
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let (tw, th) = measure::text_size(&node.label);
            if i == 0 {
                shapes::size_for(ShapeKind::Circle, tw, th)
            } else {
                (tw + 18.0, NODE_H)
            }
        })
        .collect();

    // Shift relative positions into a positive canvas with a margin.
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for i in 0..n {
        minx = minx.min(pos[i].0 - sizes[i].0 / 2.0);
        miny = miny.min(pos[i].1 - sizes[i].1 / 2.0);
        maxx = maxx.max(pos[i].0 + sizes[i].0 / 2.0);
        maxy = maxy.max(pos[i].1 + sizes[i].1 / 2.0);
    }
    let (ox, oy) = (MARGIN - minx, MARGIN - miny);
    let at = |i: usize| (pos[i].0 + ox, pos[i].1 + oy);
    let w = maxx - minx + 2.0 * MARGIN;
    let h = maxy - miny + 2.0 * MARGIN;

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:13px">"#
    );

    // Edges (under nodes): thick, branch-colored curves radiating outward.
    for (i, node) in m.nodes.iter().enumerate() {
        let (px, py) = at(i);
        for &c in &node.children {
            let (ccx, ccy) = at(c);
            let d = crate::curved_path(&[(px, py), (ccx, ccy)], false);
            svg.push_str(&format!(
                r#"<path d="{d}" stroke="{}" stroke-width="5" stroke-linecap="round" fill="none" opacity="0.85"/>"#,
                color_of(c),
            ));
        }
    }

    // Nodes: root = big filled circle; level-1 = filled rounded rect; deeper
    // leaves = text with a branch-colored underline.
    for (i, node) in m.nodes.iter().enumerate() {
        let (x, y) = at(i);
        let (tw, _) = measure::text_size(&node.label);
        if i == 0 {
            let r = sizes[i].0 / 2.0;
            svg.push_str(&format!(
                r#"<circle cx="{x:.1}" cy="{y:.1}" r="{r:.1}" fill="{ROOT_FILL}" stroke="{ROOT_FILL}"/>"#
            ));
            svg.push_str(&format!(
                r#"<text x="{x:.1}" y="{:.1}" text-anchor="middle" fill="{TXT_WHITE}" font-weight="600">{}</text>"#,
                y + 4.0,
                escape_xml(&node.label),
            ));
        } else if node.depth == 1 {
            let (bw, bh) = (tw + 18.0, NODE_H);
            // Honor the declared shape's corner style (`[square]` = sharp).
            let rx = if node.shape == ShapeKind::Rect { 1.0 } else { 6.0 };
            svg.push_str(&format!(
                r#"<rect x="{:.1}" y="{:.1}" width="{bw:.1}" height="{bh:.1}" rx="{rx}" fill="{}"/>"#,
                x - bw / 2.0,
                y - bh / 2.0,
                color_of(i),
            ));
            svg.push_str(&format!(
                r#"<text x="{x:.1}" y="{:.1}" text-anchor="middle" fill="{TXT_DARK}">{}</text>"#,
                y + 4.0,
                escape_xml(&node.label),
            ));
        } else {
            // leaf: label with a colored underline in its branch color.
            svg.push_str(&format!(
                r#"<text x="{x:.1}" y="{:.1}" text-anchor="middle" fill="{TXT_DARK}">{}</text>"#,
                y + 4.0,
                escape_xml(&node.label),
            ));
            svg.push_str(&format!(
                r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{}" stroke-width="2.5"/>"#,
                x - tw / 2.0 - 2.0,
                y + 10.0,
                x + tw / 2.0 + 2.0,
                y + 10.0,
                color_of(i),
            ));
        }
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
    fn radial_layout_colors_branches_and_styles_nodes() {
        let svg = render_svg(&p("mindmap\n  root((r))\n    A\n      a1\n    B\n      b1"));
        // Root is a filled circle in ROOT_FILL.
        assert!(svg.contains(&format!(r#"fill="{ROOT_FILL}""#)), "root fill: {svg}");
        // The two top-level branches use distinct palette colors...
        assert!(svg.contains(BRANCH_COLORS[0]) && svg.contains(BRANCH_COLORS[1]), "branch colors: {svg}");
        // ...on thick edges, and leaves get an underline.
        assert!(svg.contains(r#"stroke-width="5""#), "thick edges: {svg}");
        assert!(svg.contains("<line"), "leaf underline: {svg}");
        // Radial: not every node shares one x (the old tree put a column per depth).
        let xs: Vec<&str> = svg.split("<circle cx=\"").skip(1).map(|s| s.split('"').next().unwrap()).collect();
        assert!(!xs.is_empty());
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
