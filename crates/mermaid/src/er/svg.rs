//! SVG document assembly for ER (entity-relationship) diagrams. Consumes
//! the parsed `ErGraph` plus the boxgraph adapter's `Layout` (node
//! centers, edge polylines) and the per-entity title+attribute-grid
//! footprint sizes computed by `render_er`, and emits a single `<svg>`
//! string.
//!
//! Z-order matters (see task-7-brief.md for the normative structure this
//! file implements verbatim):
//! 1. `<svg>` open tag.
//! 2. `<defs>` — four crow's-foot markers (`mmd-er-one`, `mmd-er-zeroone`,
//!    `mmd-er-many`, `mmd-er-zeromany`).
//! 3. Relations (+ label mask rects + label texts).
//! 4. Entity boxes (title row + attribute grid).
//! 5. `</svg>`.

use crate::er::{Cardinality, ErGraph};
use crate::escape_xml;
use crate::layout::Layout;
use crate::measure;

/// Cardinality -> crow's-foot marker id (brief's normalization table).
fn marker_id(c: Cardinality) -> &'static str {
    match c {
        Cardinality::ExactlyOne => "mmd-er-one",
        Cardinality::ZeroOrOne => "mmd-er-zeroone",
        Cardinality::OneOrMore => "mmd-er-many",
        Cardinality::ZeroOrMore => "mmd-er-zeromany",
    }
}

pub(crate) fn emit(g: &ErGraph, l: &Layout, sizes: &[(f64, f64)]) -> String {
    let (w, h) = l.size;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(concat!(
        r#"<defs>"#,
        r#"<marker id="mmd-er-one" viewBox="0 0 14 14" markerWidth="12" markerHeight="12" refX="13" refY="7" orient="auto-start-reverse" stroke="currentColor" fill="none" stroke-width="1.2"><path d="M 9 2 V 12 M 12 2 V 12"/></marker>"#,
        r#"<marker id="mmd-er-zeroone" viewBox="0 0 14 14" markerWidth="12" markerHeight="12" refX="13" refY="7" orient="auto-start-reverse" stroke="currentColor" fill="none" stroke-width="1.2"><path d="M 12 2 V 12"/><circle cx="6" cy="7" r="3"/></marker>"#,
        r#"<marker id="mmd-er-many" viewBox="0 0 14 14" markerWidth="12" markerHeight="12" refX="13" refY="7" orient="auto-start-reverse" stroke="currentColor" fill="none" stroke-width="1.2"><path d="M 13 2 L 6 7 L 13 12 M 4 2 V 12"/></marker>"#,
        r#"<marker id="mmd-er-zeromany" viewBox="0 0 14 14" markerWidth="12" markerHeight="12" refX="13" refY="7" orient="auto-start-reverse" stroke="currentColor" fill="none" stroke-width="1.2"><path d="M 13 2 L 6 7 L 13 12"/><circle cx="3.5" cy="7" r="3"/></marker>"#,
        r#"</defs>"#,
    ));

    // 3. relations: marker-start/marker-end from card_from/card_to,
    // dashed only when non-identifying (markers themselves stay
    // solid-stroked, per their own `stroke` on the <marker> element).
    for ep in &l.edge_paths {
        let r = &g.relations[ep.edge];
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

        let mut attrs = format!(
            r#"stroke="currentColor" fill="none" marker-start="url(#{})" marker-end="url(#{})""#,
            marker_id(r.card_from),
            marker_id(r.card_to)
        );
        if !r.identifying {
            attrs.push_str(r#" stroke-dasharray="4 3""#);
        }
        out.push_str(&format!(r#"<path d="{d}" {attrs}/>"#));

        if let Some((lx, ly)) = ep.label_at {
            let (tw, th) = measure::text_size(&r.label);
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
                escape_xml(&r.label)
            ));
        }
    }

    // 4. entity boxes: title row (centered, bold) with a separator
    // <line> under it, then attribute rows as three left-aligned
    // columns (type / name / key) at per-entity computed offsets.
    let row_h = measure::LINE_H + 6.0;
    for (i, e) in g.entities.iter().enumerate() {
        let (cx, cy) = l.node_centers[i];
        let (bw, bh) = sizes[i];
        let box_left = cx - bw / 2.0;
        let box_top = cy - bh / 2.0;
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor"/>"#,
            box_left, box_top, bw, bh
        ));

        let mut y = box_top + 5.0;
        y += row_h;
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="600" fill="currentColor">{}</text>"#,
            cx,
            y - 6.0,
            escape_xml(&e.id)
        ));
        out.push_str(&format!(
            r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="currentColor"/>"#,
            box_left,
            y,
            box_left + bw,
            y
        ));

        // Per-entity column widths: max field width (+12 gap) over this
        // entity's attribute rows.
        let type_col_w = e
            .attributes
            .iter()
            .map(|a| measure::text_size(&a.ty).0)
            .fold(0.0_f64, f64::max)
            + 12.0;
        let name_col_w = e
            .attributes
            .iter()
            .map(|a| measure::text_size(&a.name).0)
            .fold(0.0_f64, f64::max)
            + 12.0;

        for a in &e.attributes {
            y += row_h;
            let text_y = y - 6.0;
            out.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" fill="currentColor">{}</text>"#,
                box_left + 8.0,
                text_y,
                escape_xml(&a.ty)
            ));
            out.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" fill="currentColor">{}</text>"#,
                box_left + 8.0 + type_col_w,
                text_y,
                escape_xml(&a.name)
            ));
            if let Some(key) = &a.key {
                out.push_str(&format!(
                    r#"<text x="{:.1}" y="{:.1}" fill="currentColor">{}</text>"#,
                    box_left + 8.0 + type_col_w + name_col_w,
                    text_y,
                    escape_xml(key)
                ));
            }
        }
    }

    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use crate::er::render_er;

    #[test]
    fn entities_and_relationship_render() {
        let svg = render_er("erDiagram\nCUSTOMER ||--o{ ORDER : places\nCUSTOMER {\nstring name\nint id PK\n}").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("CUSTOMER") && svg.contains("ORDER"));
        assert!(svg.contains("places"));
        assert!(svg.contains("url(#mmd-er-one)"));
        assert!(svg.contains("url(#mmd-er-zeromany)"));
        assert!(svg.contains(">PK<") || svg.contains("PK</"));
    }

    #[test]
    fn all_four_markers_exist_in_defs() {
        let svg = render_er("erDiagram\nA ||--|| B : x\nC |o--}| D : y\nE }o--o{ F : z").unwrap();
        for m in ["mmd-er-one", "mmd-er-zeroone", "mmd-er-many", "mmd-er-zeromany"] {
            assert!(svg.contains(&format!("id=\"{m}\"")), "{m} defined");
        }
    }

    #[test]
    fn non_identifying_dashed() {
        let svg = render_er("erDiagram\nA ||..|| B : weak").unwrap();
        assert!(svg.contains("stroke-dasharray"));
        let solid = render_er("erDiagram\nA ||--|| B : strong").unwrap();
        // markers use stroke but relation paths in the solid case carry no dasharray
        assert!(!solid.contains("stroke-dasharray"));
    }

    #[test]
    fn attributes_escaped() {
        let svg = render_er("erDiagram\nA {\nstring bad<script>x</script>\n}").unwrap();
        assert!(!svg.contains("<script>"));
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_er("erDiagram\nA {\nint code UK\n}").is_err());
    }
}
