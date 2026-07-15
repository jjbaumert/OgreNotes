//! SVG document assembly for flowcharts. Consumes the parsed `FlowGraph`
//! plus the layout engine's `Layout` (node centers, edge polylines,
//! cluster rects) and emits a single `<svg>` string.
//!
//! Z-order matters: defs -> cluster rects/titles (parents first) -> edges
//! (+ label masks + label text) -> nodes. See task-13-brief.md for the
//! normative attribute lists this file implements verbatim.

use crate::escape_xml;
use crate::flowchart::{shapes, EdgeKind, FlowGraph, Head};
use crate::measure;
use crate::layout::{Direction, Layout};

/// Horizontal (or vertical, in LR/RL flow) bow between paired edges so
/// opposing/parallel edges don't overlap.
const PARALLEL_BOW: f64 = 20.0;
/// Tangential spread applied to a diamond's edge anchors when two edges share
/// the same neighbour, so a decision loop's out/back edges don't collide.
const DIAMOND_PORT_SPREAD: f64 = 14.0;

/// A curve between two endpoints bowed off its midpoint (perpendicular to the
/// flow axis) so parallel edges between the same node pair separate.
fn bowed_path(points: &[(f64, f64)], off: f64, vertical: bool) -> String {
    if points.len() < 2 || off == 0.0 {
        return crate::curved_path(points, vertical);
    }
    let start = points[0];
    let end = *points.last().unwrap();
    let mx = (start.0 + end.0) / 2.0;
    let my = (start.1 + end.1) / 2.0;
    // In top-down flow bow horizontally; in left-right flow bow vertically.
    let mid = if vertical { (mx + off, my) } else { (mx, my + off) };
    crate::curved_path(&[start, mid, end], vertical)
}

pub(crate) fn emit(g: &FlowGraph, l: &Layout) -> String {
    let (w, h) = l.size;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(concat!(
        "<defs>",
        r#"<marker id="mmd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="12" markerHeight="12" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker>"#,
        r#"<marker id="mmd-circle" viewBox="0 0 10 10" refX="5" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><circle cx="5" cy="5" r="3.5" fill="currentColor"/></marker>"#,
        r#"<marker id="mmd-cross" viewBox="0 0 10 10" refX="5" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 1 1 L 9 9 M 9 1 L 1 9" stroke="currentColor" stroke-width="1.5" fill="none"/></marker>"#,
        "</defs>",
    ));

    // 3. clusters, parents first (depth ascending) so children paint on top.
    let mut order: Vec<usize> = (0..g.subgraphs.len()).collect();
    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = g.subgraphs[i].parent {
            d += 1;
            i = p;
        }
        d
    };
    order.sort_by_key(|&i| depth(i));
    for i in order {
        let r = &l.cluster_rects[i];
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--mermaid-cluster-fill, #7773)" stroke="currentColor" stroke-dasharray="4 2" rx="4"/>"#,
            r.x, r.y, r.w, r.h
        ));
        let title_x = r.x + r.w / 2.0;
        let title_y = r.y + 14.0;
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="currentColor" font-weight="600">{}</text>"#,
            title_x,
            title_y,
            escape_xml(&g.subgraphs[i].title)
        ));
    }

    // Per-node geometry (center + half-extents + shape) for shape-aware edge
    // anchoring. The layout clips edges to each node's bounding BOX; for a
    // diamond that snaps branch/return edges to the bottom vertex, so we
    // re-anchor diamond endpoints to the appropriate face midpoint instead.
    let geom: Vec<(f64, f64, f64, f64, crate::flowchart::ShapeKind)> = g
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let (tw, th) = measure::text_size(&n.label);
            let (nw, nh) = shapes::size_for(n.shape, tw, th);
            let (cx, cy) = l.node_centers[i];
            (cx, cy, nw / 2.0, nh / 2.0, n.shape)
        })
        .collect();
    // Intersection of the ray from a diamond's center toward `(tx, ty)` with
    // its rhombus outline (|x|/hw + |y|/hh = 1). A near-vertical edge lands on
    // the top/bottom vertex; a diagonal branch lands on a lower face — matching
    // Mermaid's polygon clip instead of snapping to the bounding-box vertex.
    let diamond_clip = |gi: &(f64, f64, f64, f64, crate::flowchart::ShapeKind), tx: f64, ty: f64| {
        let (dx, dy) = (tx - gi.0, ty - gi.1);
        let t = 1.0 / (dx.abs() / gi.2 + dy.abs() / gi.3).max(1e-6);
        (gi.0 + dx * t, gi.1 + dy * t)
    };

    // 4. edges (+ label mask rects + label texts)
    // Detect node pairs joined by more than one edge (parallel or opposing,
    // e.g. a `B-->D` / `D-->B` feedback loop) so we can bow them apart instead
    // of stacking them into one overlapping line.
    let mut multi: std::collections::HashMap<(usize, usize), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, ep) in l.edge_paths.iter().enumerate() {
        let e = &g.edges[ep.edge];
        if e.from != e.to {
            multi.entry((e.from.min(e.to), e.from.max(e.to))).or_default().push(i);
        }
    }
    for (i, ep) in l.edge_paths.iter().enumerate() {
        let e = &g.edges[ep.edge];
        if e.kind == EdgeKind::Invisible {
            continue; // participates in layout; draws nothing
        }
        let vertical = matches!(g.direction, Direction::TB | Direction::BT);
        // Re-anchor endpoints that land on a diamond to the correct face
        // midpoint (branch edges leave the two lower faces symmetrically rather
        // than piling onto the bottom vertex).
        // Opposing/parallel edges to the same neighbour get a tangential spread
        // on the diamond so they attach at distinct points (not one shared
        // vertex) — this, with the bow below, separates a B-->D / D-->B loop.
        let spread = match multi.get(&(e.from.min(e.to), e.from.max(e.to))) {
            Some(grp) if grp.len() > 1 => {
                let pos = grp.iter().position(|&x| x == i).unwrap_or(0);
                (pos as f64 - (grp.len() as f64 - 1.0) / 2.0) * DIAMOND_PORT_SPREAD
            }
            _ => 0.0,
        };
        let mut pts = ep.points.clone();
        if pts.len() >= 2 {
            let (tc_x, tc_y) = (geom[e.to].0, geom[e.to].1);
            if geom[e.from].4 == crate::flowchart::ShapeKind::Diamond {
                let (ax, ay) = diamond_clip(&geom[e.from], tc_x, tc_y);
                pts[0] = (ax + spread, ay);
            }
            let last = pts.len() - 1;
            let (sc_x, sc_y) = (geom[e.from].0, geom[e.from].1);
            if geom[e.to].4 == crate::flowchart::ShapeKind::Diamond {
                let (ax, ay) = diamond_clip(&geom[e.to], sc_x, sc_y);
                pts[last] = (ax + spread, ay);
            }
        }
        // Bow apart only genuine multi-edges between the same pair, and only
        // when the routing is simple (<= 3 points) — long routed edges keep
        // their waypoints untouched to avoid regressing complex layouts.
        let d = match multi.get(&(e.from.min(e.to), e.from.max(e.to))) {
            Some(grp) if grp.len() > 1 && pts.len() <= 3 => {
                let pos = grp.iter().position(|&x| x == i).unwrap_or(0);
                let off = (pos as f64 - (grp.len() as f64 - 1.0) / 2.0) * PARALLEL_BOW;
                bowed_path(&pts, off, vertical)
            }
            _ => crate::curved_path(&pts, vertical),
        };
        // Line style from the kind family; heads from the edge ends.
        let line_attrs = match e.kind {
            EdgeKind::Arrow | EdgeKind::Open | EdgeKind::Invisible => "",
            EdgeKind::Dotted => r#" stroke-dasharray="3 3""#,
            EdgeKind::Thick => r#" stroke-width="2.5""#,
        };
        let marker = |h: Head| match h {
            Head::None => None,
            Head::Arrow => Some("mmd-arrow"),
            Head::Circle => Some("mmd-circle"),
            Head::Cross => Some("mmd-cross"),
        };
        let mut attrs = format!(r#"stroke="currentColor" fill="none"{line_attrs}"#);
        if let Some(m) = marker(e.from_head) {
            attrs.push_str(&format!(r#" marker-start="url(#{m})""#));
        }
        if let Some(m) = marker(e.to_head) {
            attrs.push_str(&format!(r#" marker-end="url(#{m})""#));
        }
        // `linkStyle <index>` (or `linkStyle default`) overrides.
        if let Some(style) = e.style.as_deref().or(g.default_link_style.as_deref()) {
            if !style.is_empty() {
                attrs.push_str(&format!(r#" style="{}""#, escape_xml(style)));
            }
        }
        out.push_str(&format!(r#"<path d="{d}" {attrs}/>"#));

        if let (Some(label), Some((lx, ly))) = (&e.label, ep.label_at) {
            let (tw, th) = measure::text_size(label);
            let (mw, mh) = (tw + 4.0, th + 4.0);
            out.push_str(&format!(
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--surface, #fff)"/>"#,
                lx - mw / 2.0,
                ly - mh / 2.0,
                mw,
                mh
            ));
            out.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">{}</text>"#,
                lx,
                ly + 4.0,
                escape_xml(label)
            ));
        }
    }

    // 5. nodes
    for (i, n) in g.nodes.iter().enumerate() {
        let (cx, cy) = l.node_centers[i];
        let (tw, th) = measure::text_size(&n.label);
        let (nw, nh) = shapes::size_for(n.shape, tw, th);

        // Inline `style <id>` layers on top of any class style.
        let combined = crate::style::resolve(&n.classes, n.style.as_deref(), &g.class_defs);
        match &combined {
            Some(style) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(style))),
            None => out.push_str("<g>"),
        }
        // Style also goes ON the shape element: its own fill/stroke
        // presentation attributes would otherwise beat the inherited group
        // style, so `style A fill:#f00` wouldn't recolour the node.
        out.push_str(&shapes::emit(n.shape, cx, cy, nw, nh, combined.as_deref()));

        let lines = measure::lines(&n.label);
        let n_lines = lines.len();
        out.push_str(&format!(
            r#"<text x="{cx:.1}" y="{cy:.1}" text-anchor="middle" fill="currentColor">"#
        ));
        for (idx, line) in lines.iter().enumerate() {
            let dy = if idx == 0 {
                -((n_lines as f64 - 1.0) / 2.0) * measure::LINE_H + 4.0
            } else {
                measure::LINE_H
            };
            out.push_str(&format!(
                r#"<tspan x="{cx:.1}" dy="{dy:.1}">{}</tspan>"#,
                escape_xml(line)
            ));
        }
        out.push_str("</text>");
        out.push_str("</g>");
    }

    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use crate::flowchart::render_flowchart;



    #[test]
    fn simple_chain_renders() {
        let svg = render_flowchart("graph TD\nA[Start] --> B{Choice} -->|yes| C(End)").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("viewBox"));
        assert!(svg.contains("Start") && svg.contains("Choice") && svg.contains("End"));
        assert!(svg.contains("mmd-arrow"));
        assert!(svg.contains("yes"));
        assert!(svg.contains("<polygon")); // the diamond
    }

    #[test]
    fn decision_diamond_is_square() {
        // Defect 1: the rhombus bounding box must be square regardless of label
        // aspect (Mermaid sizes it as a rotated square).
        let svg = render_flowchart("graph TD\nB{Is it working?} --> C[x]").unwrap();
        let poly = svg.split("<polygon points=\"").nth(1).unwrap().split('"').next().unwrap();
        let xs: Vec<f64> = poly.split([' ', ',']).step_by(2).map(|s| s.parse().unwrap()).collect();
        let ys: Vec<f64> =
            poly.split([' ', ',']).skip(1).step_by(2).map(|s| s.parse().unwrap()).collect();
        let w = xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
        let h = ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min);
        assert!((w - h).abs() / w < 0.02, "diamond not square: {w} x {h}");
    }

    #[test]
    fn decision_branch_edges_leave_distinct_faces() {
        // Defect 4/3: the two branch edges from a decision node (and a back
        // edge) must not share one attachment point on the diamond.
        let svg =
            render_flowchart("graph TD\nB{q} -->|Yes| C[Ship]\nB -->|No| D[Debug]\nD --> B").unwrap();
        // Collect the start point of every edge path (`<path d="M x y ...`).
        let starts: Vec<(String, String)> = svg
            .split("<path d=\"M ")
            .skip(1)
            .map(|s| {
                let mut it = s.split_whitespace();
                (it.next().unwrap().to_string(), it.next().unwrap().to_string())
            })
            .collect();
        // The two branch starts (Yes, No) must differ — they leave opposite faces.
        assert!(starts.len() >= 3, "expected edge paths: {starts:?}");
        assert_ne!(starts[1], starts[2], "Yes/No branches share an anchor: {starts:?}");
    }

    #[test]
    fn styled_nodes_and_links_render() {
        let svg = render_flowchart(
            "graph TD\nA[Hot]-->B[Cold]\nstyle A fill:#f00,stroke:#900\nlinkStyle 0 stroke:green",
        )
        .unwrap();
        assert!(svg.contains("fill:#f00;stroke:#900;color:#fff"), "node style: {svg}");
        assert!(svg.contains("stroke:green"), "link style: {svg}");
    }

    #[test]
    fn edges_render_as_smooth_curves() {
        // Edge paths use cubic Béziers (`C`), not straight `L` segments.
        // Scope to the body after `</defs>` so the marker definitions
        // (which use `M`/`L`) aren't mistaken for edges; edges are the
        // arrowed paths (`marker-end`).
        let svg = render_flowchart("graph TD\nA-->B-->C").unwrap();
        let body = svg.split_once("</defs>").map(|(_, b)| b).unwrap_or(&svg);
        let edges: Vec<&str> = body
            .split("<path d=\"")
            .skip(1)
            .filter(|s| s.contains("marker-end"))
            .collect();
        assert_eq!(edges.len(), 2, "two edges expected");
        for p in &edges {
            let d = &p[..p.find('"').unwrap_or(p.len())];
            assert!(d.contains(" C "), "edge not curved: {d}");
        }
    }

    #[test]
    fn subgraph_renders_cluster_box_with_title() {
        let svg = render_flowchart("graph TD\nsubgraph grp[The Group]\nA --> B\nend\nC --> A").unwrap();
        assert!(svg.contains("stroke-dasharray=\"4 2\"")); // cluster rect
        assert!(svg.contains("The Group"));
    }

    #[test]
    fn class_def_styles_node_group() {
        let svg = render_flowchart("graph TD\nclassDef hot fill:#f00\nA[Hi]:::hot").unwrap();
        assert!(svg.contains("style=\"fill:#f00;color:#fff\""));
    }

    #[test]
    fn labels_are_escaped() {
        let svg = render_flowchart("graph TD\nA[\"<script>alert(1)</script>\"]").unwrap();
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn dotted_and_thick_edge_styles() {
        let svg = render_flowchart("graph TD\nA -.-> B\nA ==> C").unwrap();
        assert!(svg.contains("stroke-dasharray=\"3 3\""));
        assert!(svg.contains("stroke-width=\"2.5\""));
    }

    #[test]
    fn open_edge_has_no_arrowhead() {
        let svg = render_flowchart("graph TD\nA --- B").unwrap();
        // The marker is defined in defs but referenced by no edge.
        assert_eq!(svg.matches("marker-end").count(), 0);
    }

    #[test]
    fn multiline_label_gets_tspans() {
        let svg = render_flowchart("graph TD\nA[line one<br/>line two]").unwrap();
        assert!(svg.matches("<tspan").count() >= 2);
        assert!(svg.contains("line one") && svg.contains("line two"));
    }

    #[test]
    fn lr_direction_wider_than_tall() {
        let svg = render_flowchart("graph LR\nA --> B --> C --> D").unwrap();
        let vb: Vec<f64> = svg.split("viewBox=\"").nth(1).unwrap()
            .split('"').next().unwrap()
            .split(' ').map(|v| v.parse().unwrap()).collect();
        assert!(vb[2] > vb[3], "LR chain should be wider than tall: {vb:?}");
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_flowchart("graph TD\nA[unclosed").is_err());
    }

    #[test]
    fn too_large_graph_errors() {
        let mut src = String::from("graph TD\n");
        for i in 0..=crate::layout::MAX_NODES {
            src.push_str(&format!("n{i}\n"));
        }
        let e = render_flowchart(&src).unwrap_err();
        assert!(e.message.contains("too large"));
    }

    #[test]
    fn circle_and_cross_heads_render_their_markers() {
        let svg = render_flowchart("graph TD\nA --o B\nB --x C").unwrap();
        assert!(svg.contains(r##"marker-end="url(#mmd-circle)""##), "{svg}");
        assert!(svg.contains(r##"marker-end="url(#mmd-cross)""##), "{svg}");
        assert!(svg.contains(r#"<marker id="mmd-circle""#));
        assert!(svg.contains(r#"<marker id="mmd-cross""#));
    }

    #[test]
    fn bidirectional_edge_gets_marker_start_and_end() {
        let svg = render_flowchart("graph TD\nA <--> B").unwrap();
        assert!(svg.contains(r##"marker-start="url(#mmd-arrow)""##), "{svg}");
        assert!(svg.contains(r##"marker-end="url(#mmd-arrow)""##), "{svg}");
    }

    #[test]
    fn invisible_edge_emits_no_path_but_constrains_layout() {
        let svg = render_flowchart("graph TD\nA ~~~ B").unwrap();
        // Both nodes render; the edge draws nothing. (Can't assert on
        // `<path` — the always-present marker DEFS contain paths. Edge
        // paths are the only elements carrying this exact attr prefix;
        // the marker defs order their attributes differently.)
        assert!(svg.contains(">A<") && svg.contains(">B<"), "{svg}");
        assert!(
            !svg.contains(r#"stroke="currentColor" fill="none""#),
            "invisible edge drew a path: {svg}"
        );
    }

    #[test]
    fn plain_dotted_edge_has_no_arrowhead() {
        // mermaid semantics fix: `-.-` is open (the old table always
        // arrowed dotted edges).
        let svg = render_flowchart("graph TD\nA -.- B").unwrap();
        assert_eq!(svg.matches("marker-end").count(), 0, "{svg}");
        assert!(svg.contains("stroke-dasharray=\"3 3\""));
    }

    #[test]
    fn thick_open_edge_has_no_arrowhead() {
        // `===` is an open thick link — line style without any head.
        let svg = render_flowchart("graph TD\nA === B").unwrap();
        assert_eq!(svg.matches("marker-end").count(), 0, "{svg}");
        assert_eq!(svg.matches("marker-start").count(), 0, "{svg}");
        assert!(svg.contains("stroke-width=\"2.5\""));
    }

    #[test]
    fn class_def_default_styles_every_unclassed_node() {
        let svg = render_flowchart("graph TD\nclassDef default fill:#f9f\nA --> B").unwrap();
        // Each of the two unclassed nodes carries the style twice: on the
        // `<g>` (so `color:` reaches the label) AND on the shape element
        // (so `fill`/`stroke` actually override the shape's own attrs).
        assert_eq!(svg.matches("style=\"fill:#f9f;color:#000\"").count(), 4, "{svg}");
        // The shape element itself must carry it (the group alone can't
        // override the shape's `fill` presentation attribute).
        assert!(svg.contains("stroke-width=\"1\" style=\"fill:#f9f;color:#000\""), "style on shape: {svg}");
    }
}
