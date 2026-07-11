//! SVG document assembly for flowcharts. Consumes the parsed `FlowGraph`
//! plus the layout engine's `Layout` (node centers, edge polylines,
//! cluster rects) and emits a single `<svg>` string.
//!
//! Z-order matters: defs -> cluster rects/titles (parents first) -> edges
//! (+ label masks + label text) -> nodes. See task-13-brief.md for the
//! normative attribute lists this file implements verbatim.

use crate::escape_xml;
use crate::flowchart::{shapes, EdgeKind, FlowGraph, FlowNode};
use crate::measure;
use crate::layout::Layout;

/// Resolves a node's style attr from its classes against the graph's
/// `class_defs`: first class (in the node's declared order) that has a
/// matching, non-empty `ClassDef.style` wins.
fn node_style<'a>(g: &'a FlowGraph, n: &FlowNode) -> Option<&'a str> {
    for cls in &n.classes {
        if let Some(def) = g.class_defs.iter().find(|d| &d.name == cls) {
            if !def.style.is_empty() {
                return Some(&def.style);
            }
        }
    }
    None
}

pub(crate) fn emit(g: &FlowGraph, l: &Layout) -> String {
    let (w, h) = l.size;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(r#"<defs><marker id="mmd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#);

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

    // 4. edges (+ label mask rects + label texts)
    for ep in &l.edge_paths {
        let e = &g.edges[ep.edge];
        let d: String = ep
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == 0 {
                    format!("M {:.1} {:.1}", p.0, p.1)
                } else {
                    format!(" L {:.1} {:.1}", p.0, p.1)
                }
            })
            .collect();
        let attrs = match e.kind {
            EdgeKind::Arrow => {
                r#"stroke="currentColor" fill="none" marker-end="url(#mmd-arrow)""#.to_string()
            }
            EdgeKind::Open => r#"stroke="currentColor" fill="none""#.to_string(),
            EdgeKind::Dotted => {
                r#"stroke="currentColor" fill="none" stroke-dasharray="3 3" marker-end="url(#mmd-arrow)""#
                    .to_string()
            }
            EdgeKind::Thick => {
                r#"stroke="currentColor" fill="none" stroke-width="2.5" marker-end="url(#mmd-arrow)""#
                    .to_string()
            }
        };
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

        match node_style(g, n) {
            Some(style) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(style))),
            None => out.push_str("<g>"),
        }
        out.push_str(&shapes::emit(n.shape, cx, cy, nw, nh));

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
    fn subgraph_renders_cluster_box_with_title() {
        let svg = render_flowchart("graph TD\nsubgraph grp[The Group]\nA --> B\nend\nC --> A").unwrap();
        assert!(svg.contains("stroke-dasharray=\"4 2\"")); // cluster rect
        assert!(svg.contains("The Group"));
    }

    #[test]
    fn class_def_styles_node_group() {
        let svg = render_flowchart("graph TD\nclassDef hot fill:#f00\nA[Hi]:::hot").unwrap();
        assert!(svg.contains("style=\"fill:#f00\""));
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
}
