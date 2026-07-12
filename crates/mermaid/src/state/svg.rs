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
use crate::state::{StateGraph, StateKind, StateNote};

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

/// Geometry of one note's box — shared by the canvas-extent pre-pass
/// and the emission loop so the two cannot drift.
fn note_rect(l: &Layout, sizes: &[(f64, f64)], note: &StateNote) -> (f64, f64, f64, f64) {
    let (cx, cy) = l.node_centers[note.state];
    let (nw, _nh) = sizes[note.state];
    let (tw, th) = measure::text_size(&note.text);
    let (bw, bh) = (tw + 16.0, th + 12.0);
    let x = if note.right {
        cx + nw / 2.0 + 12.0
    } else {
        cx - nw / 2.0 - 12.0 - bw
    };
    (x, cy - bh / 2.0, bw, bh)
}

pub(crate) fn emit(g: &StateGraph, l: &Layout, sizes: &[(f64, f64)]) -> String {
    let (lw, lh) = l.size;
    // Notes are a post-layout overlay; union their rects into the
    // canvas so none can land off-canvas. They may still OVERLAP other
    // content (accepted v1 behavior) — but never be invisible.
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (0.0f64, 0.0f64, lw, lh);
    for note in &g.notes {
        let (x, y, bw, bh) = note_rect(l, sizes, note);
        min_x = min_x.min(x - 4.0);
        min_y = min_y.min(y - 4.0);
        max_x = max_x.max(x + bw + 4.0);
        max_y = max_y.max(y + bh + 4.0);
    }
    let (dx, dy) = (-min_x, -min_y); // ≥ 0 by construction (mins start at 0)
    let (w, h) = (max_x - min_x, max_y - min_y);
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(r#"<defs><marker id="mmd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#);
    // One wrapper re-homes everything when a note overflowed left/top;
    // per-element coordinates stay untouched.
    let wrapped = dx > 0.0 || dy > 0.0;
    if wrapped {
        out.push_str(&format!(r#"<g transform="translate({dx:.1},{dy:.1})">"#));
    }

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
        // Boxgraph diagrams lay out top-to-bottom, so edges curve along
        // the vertical flow axis.
        let d = crate::curved_path(&ep.points, true);
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
    // can overlap other elements (edges, neighboring nodes/notes).
    for note in &g.notes {
        let (x, y, bw, bh) = note_rect(l, sizes, note);
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

    if wrapped {
        out.push_str("</g>");
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
    fn multiline_note_block_renders_multiple_lines() {
        let svg = render_state(
            "stateDiagram-v2\nA --> B\nnote left of A\nfirst line\nsecond line\nend note",
        )
        .unwrap();
        assert!(svg.contains("first line"));
        assert!(svg.contains("second line"));
        // Both body lines render as separate <tspan>s within the note.
        assert!(svg.matches("<tspan").count() >= 2, "expected multi-line note tspans: {svg}");
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

    #[test]
    fn left_note_is_inside_the_canvas() {
        let svg = render_state(
            "stateDiagram-v2\nState1 --> State2\nnote left of State2 : This is the note to the left.",
        )
        .unwrap();
        let (w, _h) = view_size(&svg);
        let dx = translate_dx(&svg);
        assert!(dx > 0.0, "expected a translate wrapper: {svg}");
        for (x, bw) in note_rects(&svg) {
            let on_canvas_x = x + dx;
            assert!(on_canvas_x >= -0.5, "note off-canvas left: {on_canvas_x} in {svg}");
            assert!(on_canvas_x + bw <= w + 0.5, "note off-canvas right in {svg}");
        }
    }

    #[test]
    fn right_note_extends_the_canvas_without_a_wrapper() {
        let svg = render_state("stateDiagram-v2\nA --> B\nnote right of A: a fairly long note text").unwrap();
        let (w, _h) = view_size(&svg);
        for (x, bw) in note_rects(&svg) {
            assert!(x >= 0.0 && x + bw <= w + 0.5, "{svg}");
        }
        assert!(!svg.contains("<g transform=\"translate("), "no left/top overflow: {svg}");
    }

    #[test]
    fn no_notes_means_no_wrapper_and_unchanged_size() {
        let svg = render_state("stateDiagram-v2\nA --> B").unwrap();
        assert!(!svg.contains("<g transform"), "{svg}");
    }

    // Test helpers (module-level in `mod tests`):
    fn view_size(svg: &str) -> (f64, f64) {
        let vb: Vec<f64> = svg.split("viewBox=\"").nth(1).unwrap()
            .split('"').next().unwrap()
            .split(' ').map(|v| v.parse().unwrap()).collect();
        (vb[2], vb[3])
    }

    /// (x, width) of every note rect (identified by the note fill var).
    fn note_rects(svg: &str) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        let mut rest = svg;
        while let Some(fi) = rest.find("--mermaid-note-fill") {
            let rect_start = rest[..fi].rfind("<rect").unwrap();
            let rect = &rest[rect_start..fi];
            let attr = |name: &str| -> f64 {
                let j = rect.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
                rect[j..].split('"').next().unwrap().parse().unwrap()
            };
            out.push((attr("x"), attr("width")));
            rest = &rest[fi + 1..];
        }
        out
    }

    /// dx of the single translate wrapper, or 0.0 when absent.
    fn translate_dx(svg: &str) -> f64 {
        svg.split("<g transform=\"translate(").nth(1)
            .and_then(|s| s.split(',').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    }
}
