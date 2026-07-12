//! SVG document assembly for class diagrams. Consumes the parsed
//! `ClassGraph` plus the boxgraph adapter's `Layout` (node centers, edge
//! polylines) and the per-class compartment-box footprint sizes computed
//! by `render_class`, and emits a single `<svg>` string.
//!
//! Z-order matters (see task-5-brief.md for the normative structure this
//! file implements verbatim):
//! 1. `<svg>` open tag.
//! 2. `<defs>` — four relation markers (`mmd-tri-hollow`,
//!    `mmd-diamond-filled`, `mmd-diamond-hollow`, `mmd-open`).
//! 3. Relations (+ label mask rects + label texts + multiplicities).
//! 4. Class compartment boxes (name / attributes / methods).
//! 5. `</svg>`.

use crate::class::{ClassGraph, RelKind};
use crate::escape_xml;
use crate::layout::Layout;
use crate::measure;

/// Position of a relation-end multiplicity label: 14px along the path
/// inward from the endpoint, offset 10px perpendicular, computed from
/// the first (resp. last) segment's unit vector. Zero-length segments
/// (and paths too short to have a segment at all) fall back to the
/// endpoint itself.
fn mult_pos(points: &[(f64, f64)], at_start: bool) -> (f64, f64) {
    if points.len() < 2 {
        return points.first().copied().unwrap_or((0.0, 0.0));
    }
    let (p0, p1) = if at_start {
        (points[0], points[1])
    } else {
        let n = points.len();
        (points[n - 1], points[n - 2])
    };
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return p0;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    (p0.0 + ux * 14.0 + px * 10.0, p0.1 + uy * 14.0 + py * 10.0)
}

/// Post-layout note-box geometry: an attached note sits to the right of its
/// class; floating notes tile in a row below the diagram. Notes only extend
/// the canvas right/down, so no coordinate re-homing is needed.
fn note_rects(g: &ClassGraph, l: &Layout, sizes: &[(f64, f64)], base_h: f64) -> Vec<(f64, f64, f64, f64)> {
    let mut float_x = 20.0;
    let mut out = Vec::with_capacity(g.notes.len());
    for note in &g.notes {
        let (tw, th) = measure::text_size(&note.text);
        let (bw, bh) = (tw + 16.0, th + 12.0);
        let (x, y) = match note.target {
            Some(ci) => {
                let (cx, cy) = l.node_centers[ci];
                let (nw, _) = sizes[ci];
                (cx + nw / 2.0 + 16.0, cy - bh / 2.0)
            }
            None => {
                let x = float_x;
                float_x += bw + 16.0;
                (x, base_h + 20.0)
            }
        };
        out.push((x, y, bw, bh));
    }
    out
}

pub(crate) fn emit(g: &ClassGraph, l: &Layout, sizes: &[(f64, f64)]) -> String {
    let (base_w, base_h) = l.size;
    let notes = note_rects(g, l, sizes, base_h);
    let (mut w, mut h) = (base_w, base_h);
    for &(x, y, bw, bh) in &notes {
        w = w.max(x + bw + 12.0);
        h = h.max(y + bh + 12.0);
    }
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(concat!(
        r#"<defs>"#,
        r#"<marker id="mmd-tri-hollow" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="10" markerHeight="10" orient="auto-start-reverse"><path d="M 1 1 L 11 6 L 1 11 z" fill="var(--surface, #fff)" stroke="currentColor" stroke-width="1"/></marker>"#,
        r#"<marker id="mmd-diamond-filled" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="10" markerHeight="10" orient="auto-start-reverse"><path d="M 1 6 L 6 2 L 11 6 L 6 10 z" fill="currentColor"/></marker>"#,
        r#"<marker id="mmd-diamond-hollow" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="10" markerHeight="10" orient="auto-start-reverse"><path d="M 1 6 L 6 2 L 11 6 L 6 10 z" fill="var(--surface, #fff)" stroke="currentColor" stroke-width="1"/></marker>"#,
        r#"<marker id="mmd-open" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="10" markerHeight="10" orient="auto-start-reverse"><path d="M 2 2 L 11 6 L 2 10" stroke="currentColor" stroke-width="1.2" fill="none"/></marker>"#,
        r#"</defs>"#,
    ));

    // 2. namespace boxes (behind everything): dashed rect + title.
    for (i, title) in g.namespaces.iter().enumerate() {
        let r = &l.cluster_rects[i];
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--mermaid-cluster-fill, #7773)" stroke="currentColor" stroke-dasharray="4 2" rx="4"/>"#,
            r.x, r.y, r.w, r.h
        ));
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="currentColor" font-weight="600">{}</text>"#,
            r.x + r.w / 2.0,
            r.y + 14.0,
            escape_xml(title)
        ));
    }

    // 3. relations (+ label mask rects + label texts + multiplicities).
    // `Relation.to` is always the marker end and `EdgePath.points` was
    // built from -> to (route.rs restores true direction for internally
    // reversed edges), so the arrowhead lands on the path's natural end
    // with no re-reversal needed here.
    for ep in &l.edge_paths {
        let r = &g.relations[ep.edge];
        // Boxgraph diagrams lay out top-to-bottom, so edges curve along
        // the vertical flow axis.
        let d = crate::curved_path(&ep.points, true);

        let dashed =
            matches!(r.kind, RelKind::Realization | RelKind::Dependency | RelKind::DashedLink);
        let marker: Option<&str> = match r.kind {
            RelKind::Inheritance | RelKind::Realization => Some("mmd-tri-hollow"),
            RelKind::Composition => Some("mmd-diamond-filled"),
            RelKind::Aggregation => Some("mmd-diamond-hollow"),
            RelKind::Dependency => Some("mmd-open"),
            RelKind::DashedLink => None,
            RelKind::Association => {
                if r.arrow {
                    Some("mmd-open")
                } else {
                    None
                }
            }
        };
        let mut attrs = String::from(r#"stroke="currentColor" fill="none""#);
        if dashed {
            attrs.push_str(r#" stroke-dasharray="4 3""#);
        }
        if let Some(m) = marker {
            attrs.push_str(&format!(r#" marker-end="url(#{m})""#));
        }
        // Bidirectional relations (`<-->`, `<..>`) carry a second open
        // arrowhead on the `from` end. `orient="auto-start-reverse"` flips
        // the marker so it points outward from the start node.
        if r.back_arrow {
            attrs.push_str(r#" marker-start="url(#mmd-open)""#);
        }
        if let Some(style) = &r.style {
            attrs.push_str(&format!(r#" style="{}""#, escape_xml(style)));
        }
        out.push_str(&format!(r#"<path d="{d}" {attrs}/>"#));

        if let (Some(label), Some((lx, ly))) = (&r.label, ep.label_at) {
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

        if let Some(m) = &r.m_from {
            let (x, y) = mult_pos(&ep.points, true);
            out.push_str(&format!(
                r#"<text x="{x:.1}" y="{y:.1}" fill="currentColor">{}</text>"#,
                escape_xml(m)
            ));
        }
        if let Some(m) = &r.m_to {
            let (x, y) = mult_pos(&ep.points, false);
            out.push_str(&format!(
                r#"<text x="{x:.1}" y="{y:.1}" fill="currentColor">{}</text>"#,
                escape_xml(m)
            ));
        }
    }

    // 4. class compartment boxes: name (centered, bold; annotation line
    // centered italic «...» above it) / attributes / methods (both
    // left-aligned at box_left+8). A separator `<line>` precedes a
    // compartment only when that compartment is non-empty.
    for (i, c) in g.classes.iter().enumerate() {
        let (cx, cy) = l.node_centers[i];
        let (bw, bh) = sizes[i];
        let box_left = cx - bw / 2.0;
        let box_top = cy - bh / 2.0;
        let resolved = crate::style::resolve(&c.classes, c.style.as_deref(), &g.class_defs);
        match &resolved {
            Some(s) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(s))),
            None => out.push_str("<g>"),
        }
        let box_style = match &resolved {
            Some(s) => format!(r#" style="{}""#, escape_xml(s)),
            None => String::new(),
        };
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor" rx="4"{box_style}/>"#,
            box_left, box_top, bw, bh
        ));

        let row_h = measure::LINE_H + 4.0;
        let mut y = box_top + 8.0;

        if let Some(ann) = &c.annotation {
            y += row_h;
            out.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-style="italic" fill="currentColor">«{}»</text>"#,
                cx,
                y - 4.0,
                escape_xml(ann)
            ));
        }
        y += row_h;
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="600" fill="currentColor">{}</text>"#,
            cx,
            y - 4.0,
            escape_xml(c.display.as_deref().unwrap_or(&c.id))
        ));

        if !c.attributes.is_empty() {
            let sep_y = y + 4.0;
            out.push_str(&format!(
                r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="currentColor"/>"#,
                box_left,
                sep_y,
                box_left + bw,
                sep_y
            ));
            y += 8.0;
            for a in &c.attributes {
                y += row_h;
                out.push_str(&format!(
                    r#"<text x="{:.1}" y="{:.1}" fill="currentColor">{}</text>"#,
                    box_left + 8.0,
                    y - 4.0,
                    escape_xml(a)
                ));
            }
        }

        if !c.methods.is_empty() {
            let sep_y = y + 4.0;
            out.push_str(&format!(
                r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="currentColor"/>"#,
                box_left,
                sep_y,
                box_left + bw,
                sep_y
            ));
            y += 8.0;
            for m in &c.methods {
                y += row_h;
                out.push_str(&format!(
                    r#"<text x="{:.1}" y="{:.1}" fill="currentColor">{}</text>"#,
                    box_left + 8.0,
                    y - 4.0,
                    escape_xml(m)
                ));
            }
        }
        out.push_str("</g>");
    }

    // 5. notes (post-layout overlay): a dotted connector for attached notes,
    // then the note box + centered text.
    for (i, note) in g.notes.iter().enumerate() {
        let (x, y, bw, bh) = notes[i];
        if let Some(ci) = note.target {
            let (cx, cy) = l.node_centers[ci];
            let (nw, _) = sizes[ci];
            out.push_str(&format!(
                r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="currentColor" stroke-dasharray="4 2"/>"#,
                cx + nw / 2.0,
                cy,
                x,
                y + bh / 2.0
            ));
        }
        out.push_str(&format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{bw:.1}" height="{bh:.1}" fill="var(--mermaid-note-fill, #fff5ad)" stroke="currentColor" rx="2"/>"#
        ));
        let ncx = x + bw / 2.0;
        let ncy = y + bh / 2.0;
        let lines = measure::lines(&note.text);
        let n_lines = lines.len();
        out.push_str(&format!(
            r#"<text x="{ncx:.1}" y="{ncy:.1}" text-anchor="middle" fill="var(--mermaid-note-text, #333)">"#
        ));
        for (li, line) in lines.iter().enumerate() {
            let dy = if li == 0 {
                -((n_lines as f64 - 1.0) / 2.0) * measure::LINE_H + 4.0
            } else {
                measure::LINE_H
            };
            out.push_str(&format!(r#"<tspan x="{ncx:.1}" dy="{dy:.1}">{}</tspan>"#, escape_xml(line)));
        }
        out.push_str("</text>");
    }

    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use crate::class::render_class;

    #[test]
    fn class_with_members_renders_compartments() {
        let svg = render_class("classDiagram\nclass Animal {\n<<abstract>>\n+String name\n+speak() String\n}").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Animal"));
        assert!(svg.contains("«abstract»"));
        assert!(svg.contains("+String name"));
        assert!(svg.contains("+speak() String"));
        assert!(svg.matches("<line").count() >= 2); // separators
    }

    #[test]
    fn marker_per_kind() {
        let svg = render_class("classDiagram\nA <|-- B\nC <|.. D\nE *-- F\nG o-- H\nI --> J\nK ..> L").unwrap();
        assert!(svg.contains("url(#mmd-tri-hollow)"));
        assert!(svg.contains("url(#mmd-diamond-filled)"));
        assert!(svg.contains("url(#mmd-diamond-hollow)"));
        assert!(svg.contains("url(#mmd-open)"));
        assert!(svg.contains("stroke-dasharray")); // realization + dependency
    }

    #[test]
    fn plain_association_no_marker() {
        let svg = render_class("classDiagram\nA -- B").unwrap();
        assert_eq!(svg.matches("marker-end").count(), 0);
    }

    #[test]
    fn multiplicities_and_label_render() {
        let svg = render_class("classDiagram\nCustomer \"1\" --> \"0..*\" Order : places").unwrap();
        assert!(svg.contains(">1<") || svg.contains(">1</"));
        assert!(svg.contains("0..*"));
        assert!(svg.contains("places"));
    }

    #[test]
    fn members_escaped() {
        let svg = render_class("classDiagram\nclass X {\n+bad <script>alert(1)</script>\n}").unwrap();
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_class("classDiagram\nnamespace N {").is_err());
    }

    #[test]
    fn dashed_link_renders_dashed_without_marker() {
        let svg = render_class("classDiagram\nA .. B").unwrap();
        assert!(svg.contains("stroke-dasharray"));
        assert_eq!(svg.matches("marker-end").count(), 0);
    }

    #[test]
    fn class_label_renders_as_title() {
        let svg = render_class("classDiagram\nclass A[\"Account Holder\"]\nA : +id int").unwrap();
        assert!(svg.contains("Account Holder"));
        // The label replaces the raw id in the title row.
        assert!(!svg.contains(">A<"));
    }

    #[test]
    fn class_style_applied_to_box() {
        let svg = crate::render("classDiagram\nclassDef hot fill:#f00\nclass A\nA:::hot").svg.unwrap();
        // resolved style on the box rect + group wrapper for text color.
        assert!(svg.contains("fill:#f00;color:#fff"), "styled box: {svg}");
        // unstyled diagram unchanged: no stray style= on the rect.
        let plain = crate::render("classDiagram\nclass B").svg.unwrap();
        assert!(!plain.contains("<g style="), "unstyled must not wrap: {plain}");
    }

    #[test]
    fn linkstyle_colours_edge() {
        let svg = crate::render("classDiagram\nA --> B\nlinkStyle 0 stroke:#f00").svg.unwrap();
        assert!(svg.contains("stroke:#f00"), "edge style: {svg}");
    }

    #[test]
    fn combined_new_features_render() {
        // End to end: no-space inheritance, a class label, a dashed link,
        // a member block, and a `..>` dependency all in one diagram.
        let src = "classDiagram-v2\n\
                   class Animal[\"Animal (base)\"]\n\
                   Animal <|--Duck\n\
                   Animal <|--Fish\n\
                   Duck ..> Water : swims in\n\
                   Duck .. Fish\n\
                   class Duck {\n\
                   +String beakColor\n\
                   +swim()\n\
                   +quack()\n\
                   }";
        let svg = render_class(src).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Animal (base)")); // label as title
        assert!(svg.contains("+String beakColor"));
        assert!(svg.contains("url(#mmd-tri-hollow)")); // inheritance
        assert!(svg.contains("stroke-dasharray")); // dependency + dashed link
    }
}
