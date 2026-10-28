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

/// A vertical-flow edge curve bowed horizontally by `dx` at its midpoint, so
/// opposing edges between the same node pair separate into two arcs.
fn bowed_path(points: &[(f64, f64)], dx: f64) -> String {
    if points.len() != 2 || dx == 0.0 {
        return crate::curved_path(points, true);
    }
    let start = points[0];
    let end = *points.last().unwrap();
    let mid = ((start.0 + end.0) / 2.0 + dx, (start.1 + end.1) / 2.0);
    crate::curved_path(&[start, mid, end], true)
}

/// Intersect a center-to-port ray with the shape actually emitted below.
/// Pseudostates reserve an 18px layout footprint but draw a radius-7 circle.
fn state_boundary(
    kind: StateKind,
    center: (f64, f64),
    size: (f64, f64),
    toward: (f64, f64),
) -> (f64, f64) {
    let (dx, dy) = (toward.0 - center.0, toward.1 - center.1);
    if dx.abs() + dy.abs() < 1e-9 {
        return toward;
    }
    let (hw, hh) = (size.0 / 2.0, size.1 / 2.0);
    let scale = match kind {
        StateKind::Start | StateKind::End => 7.0 / dx.hypot(dy),
        StateKind::Choice => 1.0 / (dx.abs() / hw + dy.abs() / hh),
        StateKind::Normal | StateKind::ForkJoin => {
            let scale = (hw / dx.abs()).min(hh / dy.abs());
            let radius = if kind == StateKind::Normal { 8.0 } else { 0.0 };
            let (x, y) = (dx.abs() * scale, dy.abs() * scale);
            if radius == 0.0 || x <= hw - radius || y <= hh - radius {
                scale
            } else {
                // The ray hits a rounded corner instead of a flat face.
                let (cx, cy) = (hw - radius, hh - radius);
                let a = dx * dx + dy * dy;
                let b = -2.0 * (dx.abs() * cx + dy.abs() * cy);
                let c = cx * cx + cy * cy - radius * radius;
                (-b + (b * b - 4.0 * a * c).max(0.0).sqrt()) / (2.0 * a)
            }
        }
        StateKind::CompositePort => return toward,
    };
    (center.0 + dx * scale, center.1 + dy * scale)
}

/// Only direct routes between facing top/bottom borders have a free gap in
/// which to fan curves. Other routes carry obstacle-avoidance waypoints and
/// short exit/entry stubs that must remain intact.
fn facing_borders(
    t: &crate::state::Transition,
    points: &[(f64, f64)],
    l: &Layout,
    sizes: &[(f64, f64)],
) -> bool {
    if points.len() != 2 || t.from_cluster.is_some() || t.to_cluster.is_some() {
        return false;
    }
    let side = |node: usize, point: (f64, f64)| {
        let delta = point.1 - l.node_centers[node].1;
        ((delta.abs() - sizes[node].1 / 2.0).abs() < 1e-6).then_some(delta.signum())
    };
    let delta = points[1].1 - points[0].1;
    matches!((side(t.from, points[0]), side(t.to, points[1])), (Some(a), Some(b)) if a * delta > 0.0 && b * delta < 0.0)
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
    edge_label_rect_sized(measure::text_size(label), at)
}

fn edge_label_rect_sized((tw, th): (f64, f64), at: (f64, f64)) -> (f64, f64, f64, f64) {
    let (mw, mh) = (tw + 4.0, th + 4.0);
    (at.0 - mw / 2.0, at.1 - mh / 2.0, mw, mh)
}

type LoopGeometry = (Vec<(f64, f64)>, Option<(f64, f64)>);

/// Loop masks and reserved envelopes must contain literal glyph bounds too,
/// rather than assuming the general layout estimate is always wider.
fn loop_label_size(label: &str) -> (f64, f64) {
    let (width, height) = measure::text_size(label);
    let literal = measure::lines(label)
        .iter()
        .map(|line| measure::literal_text_width(line, measure::FONT_PX, false))
        .fold(0.0, f64::max);
    (width.max(literal), height)
}

/// Reserve the outside loop envelope before sibling layout, including all
/// label rows. Collapse-expand keeps that envelope free without inflating
/// the painted composite border itself.
pub(crate) fn composite_loop_margins(g: &StateGraph) -> Vec<(f64, f64)> {
    let mut counts = vec![0usize; g.composites.len()];
    let mut labels = vec![(0.0_f64, 0.0_f64); g.composites.len()];
    for transition in &g.transitions {
        let Some(cluster) = transition
            .from_cluster
            .filter(|c| Some(*c) == transition.to_cluster)
        else {
            continue;
        };
        counts[cluster] += 1;
        if let Some(label) = &transition.label {
            let (width, height) = loop_label_size(label);
            labels[cluster].0 = labels[cluster].0.max(width + 10.0);
            labels[cluster].1 += height + 4.0 + 6.0;
        }
    }
    counts
        .into_iter()
        .zip(labels)
        .map(|(count, (label_width, label_height))| {
            if count == 0 {
                (0.0, 0.0)
            } else {
                (
                    32.0 + (count - 1) as f64 * 12.0 + label_width + 4.0,
                    if label_height > 0.0 {
                        label_height + 20.0
                    } else {
                        0.0
                    },
                )
            }
        })
        .collect()
}

/// A composite-to-itself edge needs an explicit outside route: its two
/// hidden boundary ports can otherwise be joined wholly inside the box.
fn composite_loop(t: &crate::state::Transition, l: &Layout) -> Option<LoopGeometry> {
    let cluster = t.from_cluster.filter(|c| Some(*c) == t.to_cluster)?;
    let r = &l.cluster_rects[cluster];
    let right = r.x + r.w;
    let outer = right + 32.0;
    let (y1, y2) = (r.y + r.h / 3.0, r.y + 2.0 * r.h / 3.0);
    let points = vec![(right, y1), (outer, y1), (outer, y2), (right, y2)];
    let at = t.label.as_ref().map(|label| {
        (
            outer + 8.0 + loop_label_size(label).0 / 2.0,
            (y1 + y2) / 2.0,
        )
    });
    Some((points, at))
}

fn composite_loops(g: &StateGraph, l: &Layout) -> Vec<Option<LoopGeometry>> {
    let mut totals = vec![0usize; g.composites.len()];
    for transition in &g.transitions {
        if let Some(cluster) = transition
            .from_cluster
            .filter(|c| Some(*c) == transition.to_cluster)
        {
            totals[cluster] += 1;
        }
    }
    let mut label_bottoms = std::collections::HashMap::<usize, f64>::new();
    let mut loop_counts = std::collections::HashMap::<usize, usize>::new();
    g.transitions
        .iter()
        .map(|transition| {
            let mut geometry = composite_loop(transition, l)?;
            let cluster = transition
                .from_cluster
                .expect("composite_loop checked its source");
            let index = loop_counts.entry(cluster).or_default();
            // Separate each complete route, not only its label. Keep the
            // label beside that route's own outer vertical segment.
            let outer = l.cluster_rects[cluster].x
                + l.cluster_rects[cluster].w
                + 32.0
                + (totals[cluster] - 1 - *index) as f64 * 12.0;
            *index += 1;
            geometry.0[1].0 = outer;
            geometry.0[2].0 = outer;
            if let (Some(label), Some(at), Some(cluster)) =
                (&transition.label, &mut geometry.1, transition.from_cluster)
            {
                let height = loop_label_size(label).1 + 4.0;
                at.0 = outer + 8.0 + loop_label_size(label).0 / 2.0;
                // Descending rows use columns from farthest to nearest.
                // Earlier routes end above later labels, and later routes
                // stay left of earlier labels. Keep all rows below the box.
                at.1 = label_bottoms.get(&cluster).map_or(
                    l.cluster_rects[cluster].y + l.cluster_rects[cluster].h + 12.0 + height / 2.0,
                    |bottom| bottom + 6.0 + height / 2.0,
                );
                label_bottoms.insert(cluster, at.1 + height / 2.0);
                // A tall label stack can outgrow the composite. Detour
                // below its row, then return on a separate inner segment,
                // leaving the endpoints on the composite border.
                let lower = at.1 + height / 2.0 + 4.0;
                if lower > geometry.0[2].1 {
                    let start = geometry.0[0];
                    let end = geometry.0[3];
                    geometry.0 = vec![
                        start,
                        (outer, start.1),
                        (outer, lower),
                        (outer - 6.0, lower),
                        (outer - 6.0, end.1),
                        end,
                    ];
                }
            }
            Some(geometry)
        })
        .collect()
}

pub(crate) fn emit(g: &StateGraph, l: &Layout, sizes: &[(f64, f64)]) -> String {
    let (lw, lh) = l.size;
    // Share stacked loop-label geometry between sizing and SVG emission.
    let loop_routes = composite_loops(g, l);
    let node_obstacles: Vec<crate::layout::Rect> = g
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind != StateKind::CompositePort)
        .map(|(index, _)| {
            let (cx, cy) = l.node_centers[index];
            let (w, h) = sizes[index];
            crate::layout::Rect {
                x: cx - w / 2.0,
                y: cy - h / 2.0,
                w,
                h,
            }
        })
        .collect();
    let cluster_parents: Vec<_> = g.composites.iter().map(|cluster| cluster.parent).collect();
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
        let t = &g.transitions[ep.edge];
        let mut at = ep.label_at;
        if let Some((points, loop_at)) = &loop_routes[ep.edge] {
            for (x, y) in points {
                max_x = max_x.max(x + 4.0);
                max_y = max_y.max(y + 4.0);
            }
            at = *loop_at;
        }
        if let (Some(label), Some(at)) = (&t.label, at) {
            let (x, y, mw, mh) = if loop_routes[ep.edge].is_some() {
                edge_label_rect_sized(loop_label_size(label), at)
            } else {
                edge_label_rect(label, at)
            };
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
        if t.hidden {
            continue;
        }
        if t.from != t.to {
            parallel
                .entry((t.from.min(t.to), t.from.max(t.to)))
                .or_default()
                .push(i);
        }
    }
    for (i, ep) in l.edge_paths.iter().enumerate() {
        let t = &g.transitions[ep.edge];
        if t.hidden {
            continue;
        }
        let (path_points, label_at) = if let Some(route) = &loop_routes[ep.edge] {
            route.clone()
        } else {
            let mut points = ep.points.clone();
            if let Some(cluster) = t.from_cluster {
                crate::flowchart::svg::clip_start_to_cluster(
                    &mut points,
                    &l.cluster_rects[cluster],
                );
            }
            if let Some(cluster) = t.to_cluster {
                points.reverse();
                crate::flowchart::svg::clip_start_to_cluster(
                    &mut points,
                    &l.cluster_rects[cluster],
                );
                points.reverse();
            }
            (points, ep.label_at)
        };
        let direct_gap = facing_borders(t, &path_points, l, sizes);
        let mut points = path_points;
        let mut bow = 0.0;
        if direct_gap {
            if let Some(group) = parallel.get(&(t.from.min(t.to), t.from.max(t.to))) {
                if group.len() > 1 {
                    let pos = group.iter().position(|&edge| edge == i).unwrap_or(0);
                    let center = (group.len() as f64 - 1.0) / 2.0;
                    let fraction = (pos as f64 - center) / center;
                    // Bound ports by the smaller endpoint face, regardless of
                    // edge count. Reclip them below onto the actual shape.
                    let port_half = |node: usize| match g.nodes[node].kind {
                        StateKind::Start | StateKind::End => 4.0,
                        StateKind::Normal => (sizes[node].0 / 2.0 - 8.0).max(0.0),
                        _ => sizes[node].0 / 4.0,
                    };
                    let spread = fraction * port_half(t.from).min(port_half(t.to));
                    points[0].0 += spread;
                    points[1].0 += spread;
                    let mid_x = (points[0].0 + points[1].0) / 2.0;
                    let room = (mid_x - 4.0).min(lw - mid_x - 4.0).max(0.0);
                    bow = fraction * (center * PARALLEL_BOW).min(room);
                }
            }
        }
        if points.len() >= 2 {
            let last = points.len() - 1;
            if t.from_cluster.is_none() {
                points[0] = state_boundary(
                    g.nodes[t.from].kind,
                    l.node_centers[t.from],
                    sizes[t.from],
                    points[0],
                );
            }
            if t.to_cluster.is_none() {
                points[last] = state_boundary(
                    g.nodes[t.to].kind,
                    l.node_centers[t.to],
                    sizes[t.to],
                    points[last],
                );
            }
        }
        let d = match parallel.get(&(t.from.min(t.to), t.from.max(t.to))) {
            Some(group) if group.len() > 1 && t.from_cluster != t.to_cluster => {
                let position = group.iter().position(|&edge| edge == i).unwrap_or(0);
                let cluster_obstacles = crate::flowchart::svg::route_cluster_obstacles(
                    &l.cluster_rects,
                    &cluster_parents,
                    (g.nodes[t.from].composite, g.nodes[t.to].composite),
                    (t.from_cluster, t.to_cluster),
                );
                crate::flowchart::svg::routed_parallel_path(
                    &points,
                    position,
                    group.len(),
                    true,
                    &node_obstacles,
                    &cluster_obstacles,
                    l.size,
                )
                .unwrap_or_else(|| bowed_path(&points, bow))
            }
            _ => bowed_path(&points, bow),
        };
        let mut attrs =
            String::from(r#"stroke="currentColor" fill="none" marker-end="url(#mmd-arrow)""#);
        if let Some(style) = &t.style {
            attrs.push_str(&format!(r#" style="{}""#, escape_xml(style)));
        }
        out.push_str(&format!(r#"<path d="{d}" {attrs}/>"#));

        if let (Some(label), Some(at)) = (&t.label, label_at) {
            if loop_routes[ep.edge].is_some() {
                out.push_str(&crate::edge_label_svg_sized(
                    label,
                    at,
                    loop_label_size(label),
                ));
            } else {
                out.push_str(&crate::edge_label_svg(label, at));
            }
        }
    }

    // 5. nodes, by kind. Start/End/ForkJoin have no label; Choice's
    // diamond and Normal's rounded rect reuse flowchart's shape geometry
    // (`shapes::emit`) so the two families stay visually identical.
    for (i, n) in g.nodes.iter().enumerate() {
        let (cx, cy) = l.node_centers[i];
        let (nw, nh) = sizes[i];
        match n.kind {
            StateKind::CompositePort => {}
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
                out.push_str(&shapes::emit(
                    ShapeKind::Diamond,
                    cx,
                    cy,
                    nw,
                    nh,
                    resolved.as_deref(),
                ));
                emit_label(&mut out, &n.display, cx, cy);
                out.push_str("</g>");
            }
            StateKind::Normal => {
                let resolved = crate::style::resolve(&n.classes, n.style.as_deref(), &g.class_defs);
                match &resolved {
                    Some(s) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(s))),
                    None => out.push_str("<g>"),
                }
                out.push_str(&shapes::emit(
                    ShapeKind::Rounded,
                    cx,
                    cy,
                    nw,
                    nh,
                    resolved.as_deref(),
                ));
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
    #[test]
    fn composite_loop_literal_glyph_bounds_fit_their_own_masks() {
        let labels = ["o".repeat(200), "Ω".repeat(80), "e\u{301}".repeat(80)];
        let mut source = String::from("stateDiagram-v2\nstate X {\nA\n}\nstate Y {\nB\n}\n");
        for label in &labels {
            source.push_str(&format!("X --> X: {label}\n"));
        }
        let svg = crate::render(&source)
            .svg
            .expect("wide literal loop labels render");
        crate::extent::assert_inside(&svg);
        let (_, drawn) = crate::extent::scan(&svg);
        let routes: Vec<_> = drawn.iter().filter(|d| d.tag == "path").collect();
        for (index, label) in labels.iter().enumerate() {
            let text = drawn
                .iter()
                .find(|d| d.tag == "text" && &d.text == label)
                .unwrap()
                .bbox;
            let mask = drawn
                .iter()
                .filter(|d| {
                    d.tag == "rect"
                        && d.bbox.x0 <= text.x0
                        && d.bbox.x1 >= text.x1
                        && d.bbox.y0 <= text.y0
                        && d.bbox.y1 >= text.y1
                })
                .min_by(|a, b| {
                    ((a.bbox.x1 - a.bbox.x0) * (a.bbox.y1 - a.bbox.y0))
                        .total_cmp(&((b.bbox.x1 - b.bbox.x0) * (b.bbox.y1 - b.bbox.y0)))
                })
                .expect("own mask contains literal ink");
            assert!((mask.bbox.x0 - routes[index].bbox.x1 - 6.0).abs() < 0.2);
        }
    }

    #[test]
    fn distinct_composite_loops_keep_labels_beside_routes_and_avoid_other_states() {
        let source = "stateDiagram-v2\nstate X {\nA --> B\n}\nstate Y {\nC --> D\n}\nX --> X: first\nX --> X: second\nX --> X: third\nX --> Y: move\nY --> Y: fourth\nY --> Y: fifth";
        let svg = crate::render(source)
            .svg
            .expect("multiple composite loops render");
        crate::extent::assert_inside(&svg);
        let paths: Vec<_> = svg
            .split("</defs>")
            .nth(1)
            .unwrap()
            .split("<path d=\"")
            .skip(1)
            .map(|s| s.split('"').next().unwrap())
            .collect();
        assert_eq!(paths.len(), 8);
        assert_eq!(
            paths
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            8,
            "self-loop routes collapse: {svg}"
        );
        let (_, drawn) = crate::extent::scan(&svg);
        let area = |b: crate::extent::Box2| (b.x1 - b.x0) * (b.y1 - b.y0);
        let boxes: Vec<_> = ["A", "B", "C", "D"]
            .iter()
            .map(|name| {
                let text = drawn
                    .iter()
                    .find(|d| d.tag == "text" && d.text == *name)
                    .unwrap()
                    .bbox;
                drawn
                    .iter()
                    .filter(|d| {
                        d.tag == "rect"
                            && d.bbox.x0 <= text.x0
                            && d.bbox.x1 >= text.x1
                            && d.bbox.y0 <= text.y0
                            && d.bbox.y1 >= text.y1
                    })
                    .map(|d| d.bbox)
                    .min_by(|a, b| area(*a).total_cmp(&area(*b)))
                    .expect("state box")
            })
            .collect();
        let edge_bounds: Vec<_> = drawn
            .iter()
            .filter(|d| d.tag == "path")
            .map(|d| d.bbox)
            .collect();
        for (name, edge) in [
            ("first", 2),
            ("second", 3),
            ("third", 4),
            ("fourth", 6),
            ("fifth", 7),
        ] {
            let label = drawn
                .iter()
                .find(|d| d.tag == "text" && d.text == name)
                .unwrap()
                .bbox;
            let mask = drawn
                .iter()
                .filter(|d| {
                    d.tag == "rect"
                        && d.bbox.x0 <= label.x0
                        && d.bbox.x1 >= label.x1
                        && d.bbox.y0 <= label.y0
                        && d.bbox.y1 >= label.y1
                })
                .map(|d| d.bbox)
                .min_by(|a, b| area(*a).total_cmp(&area(*b)))
                .expect("own label mask");
            let route = edge_bounds[edge];
            let cy = (label.y0 + label.y1) / 2.0;
            assert!(
                (mask.x0 - route.x1 - 6.0).abs() < 0.2 && cy >= route.y0 && cy <= route.y1,
                "{name} mask must stay beside its own transition: {svg}"
            );
        }
        for path in paths {
            let mut tokens = path.split_whitespace();
            let point = |tokens: &mut std::str::SplitWhitespace<'_>| -> [f64; 2] {
                [
                    tokens.next().unwrap().parse().unwrap(),
                    tokens.next().unwrap().parse().unwrap(),
                ]
            };
            assert_eq!(tokens.next(), Some("M"));
            let mut from = point(&mut tokens);
            while let Some(command) = tokens.next() {
                assert_eq!(command, "C");
                let a = point(&mut tokens);
                let b = point(&mut tokens);
                let to = point(&mut tokens);
                for step in 0..=64 {
                    let t = step as f64 / 64.0;
                    let u = 1.0 - t;
                    let [x, y] = std::array::from_fn(|k| {
                        u * u * u * from[k]
                            + 3.0 * u * u * t * a[k]
                            + 3.0 * u * t * t * b[k]
                            + t * t * t * to[k]
                    });
                    for rect in &boxes {
                        assert!(
                            !(x > rect.x0 + 2.0
                                && x < rect.x1 - 2.0
                                && y > rect.y0 + 2.0
                                && y < rect.y1 - 2.0),
                            "loop/edge enters a state at {x},{y}: {path}"
                        );
                    }
                }
                from = to;
            }
        }
    }

    #[test]
    fn composite_self_loop_labels_stack_without_overlap_and_expand_canvas() {
        let source = "stateDiagram-v2\nstate X {\nA\n}\nX --> X: first\nX --> X\nX --> X: second<br/>extra\nX --> X: third";
        let svg = crate::render(source).svg.expect("composite loops render");
        crate::extent::assert_inside(&svg);
        let (_, drawn) = crate::extent::scan(&svg);
        let labels: Vec<_> = drawn
            .iter()
            .filter(|d| {
                d.tag == "text"
                    && ["first", "second", "third"]
                        .iter()
                        .any(|name| d.text.starts_with(name))
            })
            .collect();
        assert_eq!(labels.len(), 3);
        for (i, label) in labels.iter().enumerate() {
            for other in &labels[i + 1..] {
                assert_eq!(
                    label.bbox.overlap(&other.bbox),
                    0.0,
                    "overlapping loop labels: {svg}"
                );
            }
        }
    }
    #[test]
    fn composite_self_loop_and_long_label_stay_outside_the_box_and_inside_canvas() {
        let source = "stateDiagram-v2\nstate X {\nA --> B\n}\nX --> X: retry the composite safely";
        let svg = crate::render(source).svg.expect("composite loop renders");
        crate::extent::assert_inside(&svg);
        let (_, drawn) = crate::extent::scan(&svg);
        let label = drawn
            .iter()
            .find(|d| d.tag == "text" && d.text == "retry the composite safely")
            .expect("loop label");
        let composite = drawn
            .iter()
            .filter(|d| d.tag == "rect")
            .max_by(|a, b| {
                let area =
                    |d: &crate::extent::Drawn| (d.bbox.x1 - d.bbox.x0) * (d.bbox.y1 - d.bbox.y0);
                area(a).total_cmp(&area(b))
            })
            .expect("composite box");
        assert!(
            label.bbox.x0 > composite.bbox.x1,
            "label must be outside composite: {svg}"
        );
        assert!(
            drawn
                .iter()
                .any(|d| d.tag == "path" && d.bbox.x1 > composite.bbox.x1 + 20.0),
            "loop must leave composite: {svg}"
        );
    }

    use crate::state::render_state;

    #[test]
    fn basic_machine_renders() {
        let svg =
            render_state("stateDiagram-v2\n[*] --> Idle\nIdle --> Busy: go\nBusy --> [*]").unwrap();
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
        assert_ne!(
            ds[0], ds[1],
            "opposing edges must not share the same geometry"
        );
        // Distinct BOUNDARY anchors: the two edges' start points (`M x y`) differ.
        let starts: Vec<&str> = ds
            .iter()
            .map(|d| {
                d.trim_start_matches("M ")
                    .split_whitespace()
                    .next()
                    .unwrap()
            })
            .collect();
        assert_ne!(
            starts[0], starts[1],
            "anti-parallel edges share a start anchor: {ds:?}"
        );
        // Both carry an arrowhead at their target.
        assert_eq!(
            svg.matches("marker-end=").count(),
            2,
            "both edges arrowheaded: {svg}"
        );
    }

    #[test]
    fn theme_and_marker_sizes_match_mermaid() {
        let svg = render_state("stateDiagram-v2\n[*] --> S\nS --> [*]").unwrap();
        // D4: node border is Mermaid's nodeBorder (#9370DB via the shared var).
        assert!(
            svg.contains("var(--mermaid-node-border, #9370DB)"),
            "purple border: {svg}"
        );
        // D6: enlarged arrow marker. D5: start r=7, end r=7 outer / r=5 inner.
        assert!(
            svg.contains(r#"markerWidth="12""#),
            "arrow marker enlarged: {svg}"
        );
        assert!(
            svg.contains(r#"r="7" fill="currentColor""#),
            "start circle r=7: {svg}"
        );
        assert!(
            svg.contains(r#"r="7" fill="none""#),
            "end outer circle r=7: {svg}"
        );
    }

    #[test]
    fn composite_renders_cluster() {
        let svg = render_state("stateDiagram-v2\nstate Machine {\nA --> B\n}\nC --> A").unwrap();
        assert!(svg.contains("--mermaid-cluster-fill"));
        assert!(svg.contains("Machine"));
    }

    #[test]
    fn choice_renders_diamond() {
        let svg =
            render_state("stateDiagram-v2\nstate C <<choice>>\nA --> C\nC --> B: yes\nC --> D: no")
                .unwrap();
        assert!(svg.contains("<polygon"));
    }

    #[test]
    fn fork_renders_bar() {
        let svg =
            render_state("stateDiagram-v2\nstate F <<fork>>\nA --> F\nF --> B\nF --> C").unwrap();
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
        assert!(
            svg.matches("<tspan").count() >= 2,
            "expected multi-line note tspans: {svg}"
        );
    }

    #[test]
    fn labels_escaped() {
        let svg =
            render_state("stateDiagram-v2\nstate \"<script>x</script>\" as s\ns --> t").unwrap();
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
            assert!(
                on_canvas_x >= -0.5,
                "note off-canvas left: {on_canvas_x} in {svg}"
            );
            assert!(
                on_canvas_x + bw <= w + 0.5,
                "note off-canvas right in {svg}"
            );
        }
    }

    #[test]
    fn right_note_extends_the_canvas_without_a_wrapper() {
        let svg =
            render_state("stateDiagram-v2\nA --> B\nnote right of A: a fairly long note text")
                .unwrap();
        let (w, _h) = view_size(&svg);
        for (x, bw) in note_rects(&svg) {
            assert!(x >= 0.0 && x + bw <= w + 0.5, "{svg}");
        }
        assert!(
            !svg.contains("<g transform=\"translate("),
            "no left/top overflow: {svg}"
        );
    }

    #[test]
    fn no_notes_means_no_wrapper_and_unchanged_size() {
        let svg = render_state("stateDiagram-v2\nA --> B").unwrap();
        assert!(!svg.contains("<g transform"), "{svg}");
    }

    #[test]
    fn state_style_applied() {
        let svg = crate::render("stateDiagram-v2\nclassDef mov fill:#0f0\n[*] --> S\nS:::mov")
            .svg
            .unwrap();
        assert!(svg.contains("fill:#0f0;color:#000"), "styled state: {svg}");
        let plain = crate::render("stateDiagram-v2\n[*] --> S").svg.unwrap();
        assert!(
            !plain.contains("<g style="),
            "unstyled must not wrap: {plain}"
        );
    }

    #[test]
    fn choice_and_forkjoin_take_style() {
        // Choice diamonds and fork/join bars now honor classDef/`:::`
        // (previously only Normal states were styled).
        let choice =
            crate::render("stateDiagram-v2\nclassDef hot fill:#f00\nstate C <<choice>>\nC:::hot")
                .svg
                .unwrap();
        assert!(choice.contains("fill:#f00"), "choice styled: {choice}");
        let fork =
            crate::render("stateDiagram-v2\nclassDef hot fill:#f00\nstate F <<fork>>\nF:::hot")
                .svg
                .unwrap();
        assert!(fork.contains("fill:#f00"), "fork styled: {fork}");
    }

    #[test]
    fn linkstyle_colours_edge() {
        let svg = crate::render("stateDiagram-v2\nA --> B\nlinkStyle 0 stroke:#f00")
            .svg
            .unwrap();
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
        let vb: Vec<f64> = svg
            .split("viewBox=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .split(' ')
            .map(|v| v.parse().unwrap())
            .collect();
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
        svg.split("<g transform=\"translate(")
            .nth(1)
            .and_then(|s| s.split(',').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    }

    /// dy of the single translate wrapper, or 0.0 when absent.
    fn translate_dy(svg: &str) -> f64 {
        svg.split("<g transform=\"translate(")
            .nth(1)
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
