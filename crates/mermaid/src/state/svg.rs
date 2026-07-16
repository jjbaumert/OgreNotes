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
/// Horizontal bow applied to each edge in a parallel/opposing group so they
/// don't collapse onto one line.
const PARALLEL_BOW: f64 = 22.0;
/// Horizontal offset of each anti-parallel edge's boundary anchor along the
/// node face, so the two attach at distinct points (not one shared center).
const ANCHOR_SPREAD: f64 = 12.0;

/// A vertical-flow edge curve bowed horizontally by `dx` at its midpoint, so
/// opposing edges between the same node pair separate into two arcs.
fn bowed_path(points: &[(f64, f64)], dx: f64) -> String {
    if points.len() < 2 || dx == 0.0 {
        return crate::curved_path(points, true);
    }
    let start = points[0];
    let end = *points.last().unwrap();
    let mid = ((start.0 + end.0) / 2.0 + dx, (start.1 + end.1) / 2.0);
    crate::curved_path(&[start, mid, end], true)
}

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

/// Mask-rect geometry `(x, y, w, h)` of one edge label centered at `at` —
/// shared by the canvas-extent pre-pass and the emission loop so the two
/// cannot drift. Edge labels are frequently wider than the narrow state
/// column, so a midpoint near the left/right edge pushes the rect
/// off-canvas unless its extent is unioned into the viewBox.
fn edge_label_rect(label: &str, at: (f64, f64)) -> (f64, f64, f64, f64) {
    let (tw, th) = measure::text_size(label);
    let (mw, mh) = (tw + 4.0, th + 4.0);
    (at.0 - mw / 2.0, at.1 - mh / 2.0, mw, mh)
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
    // Edge labels are the other post-layout overlay that can spill past
    // the laid-out bounds (their mask rects are wider than the state
    // column). Union them the same way so a label near an edge stays
    // fully visible.
    for ep in &l.edge_paths {
        if let (Some(label), Some(at)) = (&g.transitions[ep.edge].label, ep.label_at) {
            let (x, y, mw, mh) = edge_label_rect(label, at);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + mw);
            max_y = max_y.max(y + mh);
        }
    }
    let (dx, dy) = (-min_x, -min_y); // ≥ 0 by construction (mins start at 0)
    let (w, h) = (max_x - min_x, max_y - min_y);
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(r#"<defs><marker id="mmd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="12" markerHeight="12" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#);
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
    // Group transitions that share the same unordered endpoint pair so
    // opposing/parallel edges (e.g. A-->B and B-->A) bow to opposite sides
    // instead of overlapping into one double-headed line.
    let mut parallel: std::collections::HashMap<(usize, usize), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, ep) in l.edge_paths.iter().enumerate() {
        let t = &g.transitions[ep.edge];
        if t.from != t.to {
            parallel.entry((t.from.min(t.to), t.from.max(t.to))).or_default().push(i);
        }
    }
    for (i, ep) in l.edge_paths.iter().enumerate() {
        let t = &g.transitions[ep.edge];
        // Boxgraph diagrams lay out top-to-bottom, so edges curve along
        // the vertical flow axis.
        let d = match parallel.get(&(t.from.min(t.to), t.from.max(t.to))) {
            Some(grp) if grp.len() > 1 => {
                let pos = grp.iter().position(|&x| x == i).unwrap_or(0);
                let center = (grp.len() as f64 - 1.0) / 2.0;
                // Each anti-parallel edge gets DISTINCT boundary anchors (its
                // endpoints shifted along the node face) AND a mid-path bow, so
                // the two are separate along their whole length — no collinear
                // stub at either end (Mermaid parity).
                let anchor = (pos as f64 - center) * ANCHOR_SPREAD;
                let mut pts = ep.points.clone();
                if pts.len() >= 2 {
                    let last = pts.len() - 1;
                    pts[0].0 += anchor;
                    pts[last].0 += anchor;
                }
                let off = (pos as f64 - center) * PARALLEL_BOW;
                bowed_path(&pts, off)
            }
            _ => crate::curved_path(&ep.points, true),
        };
        let mut attrs = String::from(r#"stroke="currentColor" fill="none" marker-end="url(#mmd-arrow)""#);
        if let Some(style) = &t.style {
            attrs.push_str(&format!(r#" style="{}""#, escape_xml(style)));
        }
        out.push_str(&format!(r#"<path d="{d}" {attrs}/>"#));

        if let (Some(label), Some((lx, ly))) = (&t.label, ep.label_at) {
            let (rx, ry, mw, mh) = edge_label_rect(label, (lx, ly));
            out.push_str(&format!(
                r#"<rect x="{rx:.1}" y="{ry:.1}" width="{mw:.1}" height="{mh:.1}" fill="var(--surface, #fff)"/>"#
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
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="7" fill="currentColor"/>"#
                ));
            }
            StateKind::End => {
                out.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="7" fill="none" stroke="currentColor"/>"#
                ));
                out.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="5" fill="currentColor"/>"#
                ));
            }
            StateKind::ForkJoin => {
                let (x, y) = (cx - nw / 2.0, cy - nh / 2.0);
                let resolved = crate::style::resolve(&n.classes, n.style.as_deref(), &g.class_defs);
                let style_attr = match &resolved {
                    Some(s) => format!(r#" style="{}""#, escape_xml(s)),
                    None => String::new(),
                };
                out.push_str(&format!(
                    r#"<rect x="{x:.1}" y="{y:.1}" width="{nw:.1}" height="{nh:.1}" fill="currentColor"{style_attr}/>"#
                ));
            }
            StateKind::Choice => {
                let resolved = crate::style::resolve(&n.classes, n.style.as_deref(), &g.class_defs);
                match &resolved {
                    Some(s) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(s))),
                    None => out.push_str("<g>"),
                }
                out.push_str(&shapes::emit(ShapeKind::Diamond, cx, cy, nw, nh, resolved.as_deref()));
                emit_label(&mut out, &n.display, cx, cy);
                out.push_str("</g>");
            }
            StateKind::Normal => {
                let resolved = crate::style::resolve(&n.classes, n.style.as_deref(), &g.class_defs);
                match &resolved {
                    Some(s) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(s))),
                    None => out.push_str("<g>"),
                }
                out.push_str(&shapes::emit(ShapeKind::Rounded, cx, cy, nw, nh, resolved.as_deref()));
                emit_label(&mut out, &n.display, cx, cy);
                out.push_str("</g>");
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
    fn opposing_transitions_bow_to_separate_arcs() {
        // A-->B and B-->A must render as two DISTINCT curved paths (bowed to
        // opposite sides), not one overlapping double-headed line.
        let svg = render_state("stateDiagram-v2\nA --> B\nB --> A").unwrap();
        let paths: Vec<&str> = svg.matches("<path d=\"").collect();
        assert!(paths.len() >= 2, "expected two edge paths: {svg}");
        // Extract the two edge `d` attributes and confirm they differ.
        let ds: Vec<&str> = svg
            .split("<path d=\"")
            .skip(1)
            .map(|s| s.split('"').next().unwrap_or(""))
            .filter(|d| d.contains('C') || d.contains('L')) // edge curves, not marker defs
            .collect();
        assert!(ds.len() >= 2, "two edge curves: {ds:?}");
        assert_ne!(ds[0], ds[1], "opposing edges must not share the same geometry");
        // Distinct BOUNDARY anchors: the two edges' start points (`M x y`) differ.
        let starts: Vec<&str> =
            ds.iter().map(|d| d.trim_start_matches("M ").split_whitespace().next().unwrap()).collect();
        assert_ne!(starts[0], starts[1], "anti-parallel edges share a start anchor: {ds:?}");
        // Both carry an arrowhead at their target.
        assert_eq!(svg.matches("marker-end=").count(), 2, "both edges arrowheaded: {svg}");
    }

    #[test]
    fn theme_and_marker_sizes_match_mermaid() {
        let svg = render_state("stateDiagram-v2\n[*] --> S\nS --> [*]").unwrap();
        // D4: node border is Mermaid's nodeBorder (#9370DB via the shared var).
        assert!(svg.contains("var(--mermaid-node-border, #9370DB)"), "purple border: {svg}");
        // D6: enlarged arrow marker. D5: start r=7, end r=7 outer / r=5 inner.
        assert!(svg.contains(r#"markerWidth="12""#), "arrow marker enlarged: {svg}");
        assert!(svg.contains(r#"r="7" fill="currentColor""#), "start circle r=7: {svg}");
        assert!(svg.contains(r#"r="7" fill="none""#), "end outer circle r=7: {svg}");
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

    #[test]
    fn state_style_applied() {
        let svg = crate::render("stateDiagram-v2\nclassDef mov fill:#0f0\n[*] --> S\nS:::mov").svg.unwrap();
        assert!(svg.contains("fill:#0f0;color:#000"), "styled state: {svg}");
        let plain = crate::render("stateDiagram-v2\n[*] --> S").svg.unwrap();
        assert!(!plain.contains("<g style="), "unstyled must not wrap: {plain}");
    }

    #[test]
    fn choice_and_forkjoin_take_style() {
        // Choice diamonds and fork/join bars now honor classDef/`:::`
        // (previously only Normal states were styled).
        let choice = crate::render(
            "stateDiagram-v2\nclassDef hot fill:#f00\nstate C <<choice>>\nC:::hot",
        )
        .svg
        .unwrap();
        assert!(choice.contains("fill:#f00"), "choice styled: {choice}");
        let fork = crate::render(
            "stateDiagram-v2\nclassDef hot fill:#f00\nstate F <<fork>>\nF:::hot",
        )
        .svg
        .unwrap();
        assert!(fork.contains("fill:#f00"), "fork styled: {fork}");
    }

    #[test]
    fn linkstyle_colours_edge() {
        let svg = crate::render("stateDiagram-v2\nA --> B\nlinkStyle 0 stroke:#f00").svg.unwrap();
        assert!(svg.contains("stroke:#f00"), "edge style: {svg}");
    }

    #[test]
    fn edge_labels_stay_within_the_canvas() {
        // Regression (#47): edge-label mask rects are wider than the
        // narrow state column, so a label whose midpoint sits near an edge
        // used to spill off-canvas (clipped). Their extents are now
        // unioned into the viewBox like notes. Both an inter-node label
        // and a self-loop label reproduced the overflow before the fix.
        for src in [
            "stateDiagram-v2\n[*] --> A: start now\nA --> B: a very long transition label here\nB --> [*]: finish",
            "stateDiagram-v2\nA --> A: self loop with a long label",
        ] {
            let svg = render_state(src).unwrap();
            let (w, h) = view_size(&svg);
            let (dx, dy) = (translate_dx(&svg), translate_dy(&svg));
            for (x, y, bw, bh) in label_rects(&svg) {
                assert!(x + dx >= -0.5, "label off-canvas left ({}) in {svg}", x + dx);
                assert!(y + dy >= -0.5, "label off-canvas top ({}) in {svg}", y + dy);
                assert!(x + dx + bw <= w + 0.5, "label off-canvas right in {svg}");
                assert!(y + dy + bh <= h + 0.5, "label off-canvas bottom in {svg}");
            }
        }
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

    /// dy of the single translate wrapper, or 0.0 when absent.
    fn translate_dy(svg: &str) -> f64 {
        svg.split("<g transform=\"translate(").nth(1)
            .and_then(|s| s.split(',').nth(1))
            .and_then(|s| s.split(')').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    }

    /// (x, y, width, height) of every edge-label mask rect (surface fill).
    fn label_rects(svg: &str) -> Vec<(f64, f64, f64, f64)> {
        let mut out = Vec::new();
        let mut rest = svg;
        while let Some(fi) = rest.find("var(--surface") {
            let rect_start = rest[..fi].rfind("<rect").unwrap();
            let rect = &rest[rect_start..fi];
            let attr = |name: &str| -> f64 {
                let j = rect.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
                rect[j..].split('"').next().unwrap().parse().unwrap()
            };
            out.push((attr("x"), attr("y"), attr("width"), attr("height")));
            rest = &rest[fi + 1..];
        }
        out
    }
}
