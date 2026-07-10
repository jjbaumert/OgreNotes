//! SVG document assembly for state diagrams. Consumes the parsed
//! `StateGraph` plus the boxgraph adapter's `Layout` (node centers, edge
//! polylines, cluster rects) and the per-node footprint sizes computed by
//! `render_state`, and emits a single `<svg>` string.
//!
//! Z-order matters (see task-3-brief.md for the normative structure this
//! file implements verbatim):
//! 1. `<svg>` open tag.
//! 2. `<defs>` — `mmd-arrow` marker.
//! 3. Composite (cluster) rects + title strips, parents first.
//! 4. Edges (+ label mask rects + label texts).
//! 5. Nodes by kind (Normal/Start/End/Choice/ForkJoin).
//! 6. Notes.
//! 7. `</svg>`.

use crate::escape_xml;
use crate::flowchart::{shapes, ShapeKind};
use crate::layout::Layout;
use crate::measure;
use crate::state::{StateGraph, StateKind};

/// Emits the label text for a node as centered tspans, mirroring
/// `flowchart::svg::emit`'s node-label block exactly (multi-line labels
/// via `measure::lines`, first line offset to vertically center the
/// block).
fn emit_label(out: &mut String, label: &str, cx: f64, cy: f64) {
    let lines = measure::lines(label);
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
}

pub(crate) fn emit(g: &StateGraph, l: &Layout, sizes: &[(f64, f64)]) -> String {
    let (w, h) = l.size;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(r#"<defs><marker id="mmd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#);

    // 3. composites (clusters), parents first (depth ascending) so
    // children paint on top — same idiom as flowchart::svg's subgraph
    // ordering.
    let mut order: Vec<usize> = (0..g.composites.len()).collect();
    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = g.composites[i].parent {
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
            escape_xml(&g.composites[i].display)
        ));
    }

    // 4. edges (+ label mask rects + label texts)
    for ep in &l.edge_paths {
        let t = &g.transitions[ep.edge];
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
        out.push_str(&format!(
            r#"<path d="{d}" stroke="currentColor" fill="none" marker-end="url(#mmd-arrow)"/>"#
        ));

        if let (Some(label), Some((lx, ly))) = (&t.label, ep.label_at) {
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

    // 5. nodes, by kind. Start/End/ForkJoin have no label; Choice's
    // diamond and Normal's rounded rect reuse flowchart's shape geometry
    // (`shapes::emit`) so the two families stay visually identical.
    for (i, n) in g.nodes.iter().enumerate() {
        let (cx, cy) = l.node_centers[i];
        let (nw, nh) = sizes[i];
        match n.kind {
            StateKind::Start => {
                out.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="8" fill="currentColor"/>"#
                ));
            }
            StateKind::End => {
                out.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="9" fill="none" stroke="currentColor"/>"#
                ));
                out.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="5" fill="currentColor"/>"#
                ));
            }
            StateKind::ForkJoin => {
                let (x, y) = (cx - nw / 2.0, cy - nh / 2.0);
                out.push_str(&format!(
                    r#"<rect x="{x:.1}" y="{y:.1}" width="{nw:.1}" height="{nh:.1}" fill="currentColor"/>"#
                ));
            }
            StateKind::Choice => {
                out.push_str(&shapes::emit(ShapeKind::Diamond, cx, cy, nw, nh));
                emit_label(&mut out, &n.display, cx, cy);
            }
            StateKind::Normal => {
                out.push_str(&shapes::emit(ShapeKind::Rounded, cx, cy, nw, nh));
                emit_label(&mut out, &n.display, cx, cy);
            }
        }
    }

    // 6. notes. Placed post-layout beside their state's laid-out center;
    // notes are not boxgraph nodes, so this is a best-effort overlay that
    // can overlap other elements (edges, neighboring nodes/notes) —
    // accepted for v1, per the brief.
    for note in &g.notes {
        let (cx, cy) = l.node_centers[note.state];
        let (nw, _nh) = sizes[note.state];
        let (tw, th) = measure::text_size(&note.text);
        let (bw, bh) = (tw + 16.0, th + 12.0);
        let x = if note.right {
            cx + nw / 2.0 + 12.0
        } else {
            cx - nw / 2.0 - 12.0 - bw
        };
        let y = cy - bh / 2.0;
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
        for (idx, line) in lines.iter().enumerate() {
            let dy = if idx == 0 {
                -((n_lines as f64 - 1.0) / 2.0) * measure::LINE_H + 4.0
            } else {
                measure::LINE_H
            };
            out.push_str(&format!(
                r#"<tspan x="{ncx:.1}" dy="{dy:.1}">{}</tspan>"#,
                escape_xml(line)
            ));
        }
        out.push_str("</text>");
    }

    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use crate::state::render_state;

    #[test]
    fn basic_machine_renders() {
        let svg = render_state("stateDiagram-v2\n[*] --> Idle\nIdle --> Busy: go\nBusy --> [*]").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Idle") && svg.contains("Busy"));
        assert!(svg.contains(">go<") || svg.contains("go</"));
        assert!(svg.contains("mmd-arrow"));
        // start = filled circle, end = ring (2+ circles total)
        assert!(svg.matches("<circle").count() >= 3);
    }

    #[test]
    fn composite_renders_cluster() {
        let svg = render_state("stateDiagram-v2\nstate Machine {\nA --> B\n}\nC --> A").unwrap();
        assert!(svg.contains("--mermaid-cluster-fill"));
        assert!(svg.contains("Machine"));
    }

    #[test]
    fn choice_renders_diamond() {
        let svg = render_state("stateDiagram-v2\nstate C <<choice>>\nA --> C\nC --> B: yes\nC --> D: no").unwrap();
        assert!(svg.contains("<polygon"));
    }

    #[test]
    fn fork_renders_bar() {
        let svg = render_state("stateDiagram-v2\nstate F <<fork>>\nA --> F\nF --> B\nF --> C").unwrap();
        assert!(svg.contains("fill=\"currentColor\""));
    }

    #[test]
    fn note_renders() {
        let svg = render_state("stateDiagram-v2\nA --> B\nnote right of A: important").unwrap();
        assert!(svg.contains("--mermaid-note-fill"));
        assert!(svg.contains("important"));
    }

    #[test]
    fn labels_escaped() {
        let svg = render_state("stateDiagram-v2\nstate \"<script>x</script>\" as s\ns --> t").unwrap();
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_state("stateDiagram-v2\n}").is_err());
    }
}
