//! Cluster (subgraph) layout via recursive collapse-expand.
//!
//! Each cluster's direct members (real nodes plus already-collapsed child
//! clusters) are laid out on their own as a flat TB subgraph, then
//! collapsed into a single placeholder node sized to fit that sub-layout
//! plus its title strip. The graph one level up sees only placeholders in
//! place of collapsed clusters, and is laid out the same way, all the way
//! to the top. Expansion walks back down top-first, rigidly translating
//! each sub-layout into its placeholder's rect. Edges that cross a cluster
//! boundary are routed at the level of their lowest common container
//! (using representative placeholders for whichever side collapsed). Inside
//! each cluster they cross, the edge has a leg of its own: every cluster
//! with outgoing (incoming) crossing edges gets an exit (entry) port pinned
//! below (above) all its members, and the leg member→port (port→member) is
//! laid out with the cluster, so it detours around sibling nodes instead of
//! cutting straight through them. The legs and the main path are stitched
//! into one polyline. `apply_direction` runs exactly once, on the fully
//! assembled whole.

use super::{
    apply_direction, layout_tb, validate, Direction, EdgePath, LEdge, LNode, Layout, LayoutInput,
    Rect,
};

const CLUSTER_PAD: f64 = 12.0;

/// A node's immediate container in the collapse hierarchy: either the real
/// node itself, or (once collapsed) the placeholder standing in for a
/// child cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Entity {
    Node(usize),
    Cluster(usize),
    /// A cluster's exit (`true`) or entry port for boundary-crossing edges.
    Port(bool),
}

/// What a level-local edge stands for.
#[derive(Debug, Clone, Copy)]
enum LocalRole {
    /// An original edge routed at this level (its lowest common container).
    Real(usize),
    /// The inside leg of original edge `.0` in this cluster: member → exit
    /// port, or entry port → member.
    Leg(usize),
    /// Invisible: pins a port below every sink / above every source.
    Pin,
}

/// Port footprint: a waypoint, not a box.
const PORT_SIZE: f64 = 2.0;

/// One level of the collapse hierarchy: either a single cluster's induced
/// subgraph, or the top-level graph (real top-level nodes + top-level
/// cluster placeholders). Laid out in isolation, in local TB coordinates.
struct SubBuild {
    /// Direct members of this level, in the same order as `layout`'s nodes.
    entities: Vec<Entity>,
    /// Parallel to `local_edges`: what each one stands for.
    local_edge_orig: Vec<LocalRole>,
    /// This level's flat TB layout (local coordinates, no direction applied).
    layout: Layout,
    /// Size of the placeholder node this level collapses to in its parent:
    /// `(layout.size.0, layout.size.1 + title.1 + CLUSTER_PAD)`. Unused
    /// (left zeroed) for the top level, which has no parent.
    placeholder_size: (f64, f64),
}

/// Walks a node's cluster-ancestor chain to find its representative entity
/// at `target`'s level (`target = None` means the top level). Returns
/// `None` if `v` is not nested (directly or indirectly) under `target`.
fn representative_of(input: &LayoutInput, v: usize, target: Option<usize>) -> Option<Entity> {
    let mut cur = input.nodes[v].cluster;
    if cur == target {
        return Some(Entity::Node(v));
    }
    let mut guard = 0usize;
    loop {
        let c = cur?;
        let parent = input.clusters[c].parent;
        if parent == target {
            return Some(Entity::Cluster(c));
        }
        cur = parent;
        guard += 1;
        if guard > input.clusters.len() {
            // Defensive only: validate() rejects parent cycles, so this
            // never triggers on validated input.
            return None;
        }
    }
}

/// The direction cluster `c` lays out in: its own `direction` if set,
/// else the nearest ancestor's, falling back to `graph_dir`. A cluster with
/// a member linked outside it ignores its own `direction` and inherits
/// (mermaid.js: "if any of a subgraph's nodes are linked to the outside,
/// subgraph direction will be ignored").
fn effective_dir(clusters: &[super::LCluster], links_out: &[bool], c: usize, graph_dir: Direction) -> Direction {
    let mut cur = Some(c);
    let mut guard = 0usize;
    while let Some(ci) = cur {
        if let Some(d) = clusters[ci].direction.filter(|_| !links_out[ci]) {
            return d;
        }
        cur = clusters[ci].parent;
        guard += 1;
        if guard > clusters.len() {
            break; // defensive only; validate() rules out parent cycles
        }
    }
    graph_dir
}

/// Whether node `v` sits in cluster `c` (directly or nested).
fn under(input: &LayoutInput, v: usize, c: usize) -> bool {
    let mut cur = input.nodes[v].cluster;
    let mut guard = 0usize;
    while let Some(x) = cur {
        if x == c {
            return true;
        }
        cur = input.clusters[x].parent;
        guard += 1;
        if guard > input.clusters.len() {
            break; // defensive only; validate() rules out cycles
        }
    }
    false
}

/// The clusters containing `v` but not `other`, innermost first.
fn exclusive_chain(input: &LayoutInput, v: usize, other: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut cur = input.nodes[v].cluster;
    while let Some(c) = cur {
        if under(input, other, c) || out.len() > input.clusters.len() {
            break;
        }
        out.push(c);
        cur = input.clusters[c].parent;
    }
    out
}

/// Depth of cluster `c` in the parent forest (root clusters are depth 0).
fn cluster_depth(clusters: &[super::LCluster], c: usize) -> usize {
    let mut depth = 0usize;
    let mut cur = clusters[c].parent;
    let mut guard = 0usize;
    while let Some(p) = cur {
        depth += 1;
        cur = clusters[p].parent;
        guard += 1;
        if guard > clusters.len() {
            break; // defensive only; validate() rules out cycles
        }
    }
    depth
}

/// Builds and lays out one level of the collapse hierarchy: the induced
/// subgraph of `target`'s direct members (real nodes with `cluster ==
/// target`, plus already-processed child-cluster placeholders).
fn build_level(
    input: &LayoutInput,
    target: Option<usize>,
    level_dir: Direction,
    sub_builds: &[Option<SubBuild>],
) -> Result<SubBuild, String> {
    let mut entities: Vec<Entity> = Vec::new();
    for (i, n) in input.nodes.iter().enumerate() {
        if n.cluster == target {
            entities.push(Entity::Node(i));
        }
    }
    for (ci, c) in input.clusters.iter().enumerate() {
        if c.parent == target {
            entities.push(Entity::Cluster(ci));
        }
    }
    let mut entity_index: std::collections::HashMap<Entity, usize> =
        std::collections::HashMap::with_capacity(entities.len());
    for (i, e) in entities.iter().enumerate() {
        entity_index.insert(*e, i);
    }

    // Intra-level edges: original edges whose two endpoints resolve to
    // different entities at this level (a normal cross-member edge), or to
    // the SAME `Entity::Node` via a true self-loop (from == to). Edges
    // whose endpoints resolve to the same entity for any other reason
    // belong to a deeper level and were already consumed there; edges with
    // an endpoint outside `target`'s subtree belong to a shallower level.
    // The `Entity::Node` gate on the self-loop branch matters: at ancestor
    // levels a self-loop's endpoints both resolve to the same CLUSTER
    // placeholder (`rf == rt` there too), and without the gate the loop
    // would be emitted again at every ancestor level. A self-loop belongs
    // only to the one level where its node is a direct member.
    let mut local_edges = Vec::new();
    let mut local_edge_orig = Vec::new();
    for (ei, edge) in input.edges.iter().enumerate() {
        let rf = representative_of(input, edge.from, target);
        let rt = representative_of(input, edge.to, target);
        if let (Some(rf), Some(rt)) = (rf, rt) {
            if rf != rt || (edge.from == edge.to && matches!(rf, Entity::Node(_))) {
                local_edges.push(LEdge {
                    from: entity_index[&rf],
                    to: entity_index[&rt],
                    label: edge.label,
                });
                local_edge_orig.push(LocalRole::Real(ei));
            }
        }
    }

    // Boundary-crossing edges: a leg from the inside member to this
    // cluster's exit port (or from its entry port to the member).
    if let Some(c) = target {
        let port_of = |exit: bool, entities: &mut Vec<Entity>, idx: &mut std::collections::HashMap<Entity, usize>| {
            *idx.entry(Entity::Port(exit)).or_insert_with(|| {
                entities.push(Entity::Port(exit));
                entities.len() - 1
            })
        };
        for (ei, edge) in input.edges.iter().enumerate() {
            let (from_in, to_in) = (under(input, edge.from, c), under(input, edge.to, c));
            if from_in == to_in {
                continue;
            }
            let inner = if from_in { edge.from } else { edge.to };
            let Some(rep) = representative_of(input, inner, target) else { continue };
            let member = entity_index[&rep];
            let port = port_of(from_in, &mut entities, &mut entity_index);
            let (from, to) = if from_in { (member, port) } else { (port, member) };
            local_edges.push(LEdge { from, to, label: None });
            local_edge_orig.push(LocalRole::Leg(ei));
        }
        // Pin the exit port below every sink and the entry port above every
        // source in the ranking DAG. In the original graph a cycle can hide
        // those sources/sinks, leaving a port beside a sibling mid-cluster;
        // the boundary join would then cut through that sibling. Adding pins
        // cannot change DFS back-edges: entry ports have no incoming edges and
        // exit ports have no outgoing edges.
        let n = entities.len();
        let (mut has_out, mut has_in) = (vec![false; n], vec![false; n]);
        let ranked = super::acyclic::make_acyclic(n, &local_edges);
        for e in &ranked.edges {
            has_out[e.from] = true;
            has_in[e.to] = true;
        }
        for (i, ent) in entities.clone().iter().enumerate() {
            if matches!(ent, Entity::Port(_)) {
                continue;
            }
            if let Some(&p) = entity_index.get(&Entity::Port(true)) {
                if !has_out[i] {
                    local_edges.push(LEdge { from: i, to: p, label: None });
                    local_edge_orig.push(LocalRole::Pin);
                }
            }
            if let Some(&p) = entity_index.get(&Entity::Port(false)) {
                if !has_in[i] {
                    local_edges.push(LEdge { from: p, to: i, label: None });
                    local_edge_orig.push(LocalRole::Pin);
                }
            }
        }
    }

    // Each level is laid out in ITS OWN direction (`level_dir`): member
    // nodes enter at their true extents, and child clusters enter as
    // opaque, already-oriented placeholder blocks (their `placeholder_size`
    // is the FINAL oriented footprint produced when this same function ran
    // one level down). `layout_tb` + `apply_direction(level_dir)` is exactly
    // the "transpose-in, swap-out" `run_flat` performs for a flat graph, so
    // this level's members are oriented and separated correctly for
    // `level_dir`. Levels then nest by pure translation in `expand_level`,
    // which is why a subgraph can flow in a different direction than its
    // parent: each block is oriented before it is placed, and no global
    // post-transform re-rotates it.
    let mut local_nodes: Vec<LNode> = Vec::with_capacity(entities.len());
    for e in &entities {
        match *e {
            Entity::Node(v) => {
                let n = &input.nodes[v];
                local_nodes.push(LNode { width: n.width, height: n.height, cluster: None });
            }
            Entity::Cluster(c) => {
                let (w, h) = sub_builds[c]
                    .as_ref()
                    .expect("child clusters are built before their parent")
                    .placeholder_size;
                local_nodes.push(LNode { width: w, height: h, cluster: None });
            }
            Entity::Port(_) => {
                local_nodes.push(LNode { width: PORT_SIZE, height: PORT_SIZE, cluster: None });
            }
        }
    }

    let local_input = LayoutInput {
        nodes: local_nodes,
        edges: local_edges,
        clusters: vec![],
        direction: level_dir,
    };
    let mut layout = layout_tb(&local_input)?;
    apply_direction(&mut layout, level_dir);

    Ok(SubBuild {
        entities,
        local_edge_orig,
        layout,
        placeholder_size: (0.0, 0.0),
    })
}

/// Recursively translates `build`'s local layout into absolute coordinates
/// by `translate`, writing real node centers and cluster rects into the
/// shared output buffers, and appending this level's edges (with
/// cluster-side endpoints re-clipped to the true inner node once known).
#[allow(clippy::too_many_arguments)]
fn expand_level(
    input: &LayoutInput,
    build: &SubBuild,
    level: Option<usize>,
    translate: (f64, f64),
    sub_builds: &[Option<SubBuild>],
    node_centers: &mut [(f64, f64)],
    cluster_rects: &mut [Option<Rect>],
    edges_out: &mut Vec<EdgePath>,
    legs: &mut std::collections::HashMap<(usize, usize), Vec<(f64, f64)>>,
) {
    for (local_idx, entity) in build.entities.iter().enumerate() {
        match *entity {
            Entity::Node(v) => {
                let c = build.layout.node_centers[local_idx];
                node_centers[v] = (c.0 + translate.0, c.1 + translate.1);
            }
            Entity::Cluster(c) => {
                let center = build.layout.node_centers[local_idx];
                let abs_center = (center.0 + translate.0, center.1 + translate.1);
                let child = sub_builds[c]
                    .as_ref()
                    .expect("child clusters are built before their parent");
                let (w, h) = child.placeholder_size;
                let rect = Rect {
                    x: abs_center.0 - w / 2.0,
                    y: abs_center.1 - h / 2.0,
                    w,
                    h,
                };
                let title_h = input.clusters[c].title.1;
                let slack_x = (w - child.layout.size.0).max(0.0) / 2.0;
                let content_translate = (rect.x + slack_x, rect.y + title_h + CLUSTER_PAD);
                cluster_rects[c] = Some(rect);
                expand_level(
                    input,
                    child,
                    Some(c),
                    content_translate,
                    sub_builds,
                    node_centers,
                    cluster_rects,
                    edges_out,
                    legs,
                );
            }
            Entity::Port(_) => {}
        }
    }

    for ep in &build.layout.edge_paths {
        let pts: Vec<(f64, f64)> = ep
            .points
            .iter()
            .map(|p| (p.0 + translate.0, p.1 + translate.1))
            .collect();
        match build.local_edge_orig[ep.edge] {
            LocalRole::Pin => {}
            LocalRole::Leg(orig) => {
                if let Some(c) = level {
                    legs.insert((orig, c), pts);
                }
            }
            LocalRole::Real(orig) => {
                let label_at = ep.label_at.map(|p| (p.0 + translate.0, p.1 + translate.1));
                edges_out.push(EdgePath { edge: orig, points: pts, label_at, reversed: ep.reversed });
            }
        }
    }
}

/// Splice each crossing edge's legs onto its main path: source-side legs
/// innermost first, then the main path (which starts and ends on the
/// outermost crossed clusters' borders), then target-side legs outermost
/// first — one polyline from the true source to the true target. Each
/// junction joins a port to the point where the next piece meets that
/// cluster's border; when the straight join would cut through a node it
/// runs along the cluster's inner margin instead (`ring_route`).
fn stitch_legs(
    input: &LayoutInput,
    edges: &mut [EdgePath],
    legs: &mut std::collections::HashMap<(usize, usize), Vec<(f64, f64)>>,
    cluster_rects: &[Option<Rect>],
    node_centers: &[(f64, f64)],
) {
    for ep in edges.iter_mut() {
        let e = &input.edges[ep.edge];
        if e.from == e.to {
            continue;
        }
        let src = exclusive_chain(input, e.from, e.to);
        let dst = exclusive_chain(input, e.to, e.from);
        if src.is_empty() && dst.is_empty() {
            continue;
        }
        // Nodes a join must not cross: everything but this edge's ends.
        let blocked = |a: (f64, f64), b: (f64, f64)| {
            input.nodes.iter().enumerate().any(|(v, n)| {
                v != e.from
                    && v != e.to
                    && segment_hits_box(a, b, node_centers[v], (n.width / 2.0 - 1.0, n.height / 2.0 - 1.0))
            })
        };
        let mut pts: Vec<(f64, f64)> = Vec::new();
        let join = |pts: &mut Vec<(f64, f64)>, next: (f64, f64), ring: Option<&Rect>| {
            if let (Some(&last), Some(r)) = (pts.last(), ring) {
                if blocked(last, next) {
                    pts.extend(ring_route(r, last, next));
                }
            }
        };
        let push_all = |pts: &mut Vec<(f64, f64)>, seg: &[(f64, f64)]| {
            for &p in seg {
                if pts.last().is_none_or(|q: &(f64, f64)| (q.0 - p.0).abs() > 1e-6 || (q.1 - p.1).abs() > 1e-6) {
                    pts.push(p);
                }
            }
        };
        let mut ring: Option<&Rect> = None;
        for c in &src {
            if let Some(seg) = legs.remove(&(ep.edge, *c)) {
                if let Some(&first) = seg.first() {
                    join(&mut pts, first, ring);
                }
                push_all(&mut pts, &seg);
                ring = cluster_rects[*c].as_ref();
            }
        }
        if let Some(&first) = ep.points.first() {
            join(&mut pts, first, ring);
        }
        push_all(&mut pts, &ep.points);
        for c in dst.iter().rev() {
            if let Some(seg) = legs.remove(&(ep.edge, *c)) {
                if let Some(&first) = seg.first() {
                    join(&mut pts, first, cluster_rects[*c].as_ref());
                }
                push_all(&mut pts, &seg);
            }
        }
        if pts.len() >= 2 {
            ep.points = pts;
        }
    }
}

/// Does segment a→b pass through the box centered at `c` with half-extents
/// `half` (Liang–Barsky clip)?
fn segment_hits_box(a: (f64, f64), b: (f64, f64), c: (f64, f64), half: (f64, f64)) -> bool {
    let (hx, hy) = half;
    if hx <= 0.0 || hy <= 0.0 {
        return false;
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-dx, a.0 - (c.0 - hx)), (dx, (c.0 + hx) - a.0), (-dy, a.1 - (c.1 - hy)), (dy, (c.1 + hy) - a.1)] {
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

/// Waypoints from `from` (inside `r`) to `to` (on `r`'s border) along a
/// track just inside the border — the cluster's padding, which holds no
/// nodes — taking the shorter way round.
fn ring_route(r: &Rect, from: (f64, f64), to: (f64, f64)) -> Vec<(f64, f64)> {
    let d = (CLUSTER_PAD / 2.0).min(r.w / 4.0).min(r.h / 4.0);
    let (x0, y0, x1, y1) = (r.x + d, r.y + d, r.x + r.w - d, r.y + r.h - d);
    // Perimeter coordinate of the nearest track point, clockwise from the
    // top-left corner.
    let (w, h) = (x1 - x0, y1 - y0);
    let perim = 2.0 * (w + h);
    let project = |p: (f64, f64)| -> ((f64, f64), f64) {
        let (px, py) = (p.0.clamp(x0, x1), p.1.clamp(y0, y1));
        let dists = [(py - y0).abs(), (x1 - px).abs(), (y1 - py).abs(), (px - x0).abs()];
        let side = (0..4).min_by(|&a, &b| dists[a].partial_cmp(&dists[b]).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0);
        match side {
            0 => ((px, y0), px - x0),
            1 => ((x1, py), w + (py - y0)),
            2 => ((px, y1), w + h + (x1 - px)),
            _ => ((x0, py), 2.0 * w + h + (y1 - py)),
        }
    };
    let (pa, sa) = project(from);
    let (pb, sb) = project(to);
    let corners = [(0.0, (x0, y0)), (w, (x1, y0)), (w + h, (x1, y1)), (2.0 * w + h, (x0, y1))];
    let cw = (sb - sa).rem_euclid(perim);
    let mut out = vec![pa];
    if cw <= perim - cw {
        // Clockwise: corners with coordinate in (sa, sa + cw).
        let mut cs: Vec<(f64, (f64, f64))> =
            corners.iter().map(|&(t, c)| ((t - sa).rem_euclid(perim), c)).filter(|&(t, _)| t > 1e-9 && t < cw).collect();
        cs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out.extend(cs.into_iter().map(|(_, c)| c));
    } else {
        let ccw = perim - cw;
        let mut cs: Vec<(f64, (f64, f64))> =
            corners.iter().map(|&(t, c)| ((sa - t).rem_euclid(perim), c)).filter(|&(t, _)| t > 1e-9 && t < ccw).collect();
        cs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out.extend(cs.into_iter().map(|(_, c)| c));
    }
    out.push(pb);
    out
}

/// Lays out a graph whose `clusters` are non-empty via recursive
/// collapse-expand: see the module doc for the algorithm.
pub(crate) fn run_clustered(input: &LayoutInput) -> Result<Layout, String> {
    validate(input)?;

    let depths: Vec<usize> =
        (0..input.clusters.len()).map(|c| cluster_depth(&input.clusters, c)).collect();
    let mut order: Vec<usize> = (0..input.clusters.len()).collect();
    // Stable sort: deepest first, ties broken by ascending cluster index.
    order.sort_by_key(|&c| std::cmp::Reverse(depths[c]));

    let links_out: Vec<bool> = (0..input.clusters.len())
        .map(|c| input.edges.iter().any(|e| under(input, e.from, c) != under(input, e.to, c)))
        .collect();
    let mut sub_builds: Vec<Option<SubBuild>> = (0..input.clusters.len()).map(|_| None).collect();
    for c in order {
        let level_dir = effective_dir(&input.clusters, &links_out, c, input.direction);
        let build = build_level(input, Some(c), level_dir, &sub_builds)?;
        let (title_w, title_h) = input.clusters[c].title;
        // At least as wide as its title (#274); `expand_level` centers the
        // content in any extra width.
        let placeholder_size = (
            build.layout.size.0.max(title_w + 2.0 * CLUSTER_PAD),
            build.layout.size.1 + title_h + CLUSTER_PAD,
        );
        sub_builds[c] = Some(SubBuild { placeholder_size, ..build });
    }

    let top = build_level(input, None, input.direction, &sub_builds)?;

    let mut node_centers = vec![(0.0, 0.0); input.nodes.len()];
    let mut cluster_rects: Vec<Option<Rect>> = (0..input.clusters.len()).map(|_| None).collect();
    let mut edges_out: Vec<EdgePath> = Vec::new();
    let mut legs = std::collections::HashMap::new();
    expand_level(
        input,
        &top,
        None,
        (0.0, 0.0),
        &sub_builds,
        &mut node_centers,
        &mut cluster_rects,
        &mut edges_out,
        &mut legs,
    );
    stitch_legs(input, &mut edges_out, &mut legs, &cluster_rects, &node_centers);
    edges_out.sort_by_key(|p| p.edge);

    let cluster_rects: Vec<Rect> = cluster_rects
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            r.unwrap_or_else(|| {
                // Unreachable on validated input: every cluster is nested
                // (directly or indirectly) under the top level, so
                // `expand_level` always visits it. Kept as a hard fallback
                // rather than a panic.
                let (w, h) = input.clusters[i].title;
                Rect { x: 0.0, y: 0.0, w: w.max(1.0), h: h.max(1.0) }
            })
        })
        .collect();

    // No global `apply_direction` here: every level (including the top) was
    // already oriented inside `build_level`, so the assembled whole is in
    // final coordinates. This is what lets a subgraph flow in a different
    // direction than its parent.
    let layout =
        Layout { node_centers, edge_paths: edges_out, cluster_rects, size: top.layout.size };
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Direction, LCluster, LEdge, LNode, LayoutInput};

    fn node_in(c: Option<usize>) -> LNode {
        LNode { width: 60.0, height: 24.0, cluster: c }
    }

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    fn input_one_cluster() -> LayoutInput {
        // 0 top-level -> 1 (in cluster) -> 2 (in cluster) -> 3 top-level
        LayoutInput {
            nodes: vec![node_in(None), node_in(Some(0)), node_in(Some(0)), node_in(None)],
            edges: vec![e(0, 1), e(1, 2), e(2, 3)],
            clusters: vec![LCluster { parent: None, title: (50.0, 16.0), direction: None }],
            direction: Direction::TB,
        }
    }

    fn inside(p: (f64, f64), r: &crate::layout::Rect) -> bool {
        p.0 >= r.x && p.0 <= r.x + r.w && p.1 >= r.y && p.1 <= r.y + r.h
    }

    #[test]
    fn members_inside_cluster_rect() {
        let l = run_clustered(&input_one_cluster()).unwrap();
        let r = &l.cluster_rects[0];
        assert!(inside(l.node_centers[1], r));
        assert!(inside(l.node_centers[2], r));
        assert!(!inside(l.node_centers[0], r));
        assert!(!inside(l.node_centers[3], r));
    }

    #[test]
    fn cross_boundary_edges_reach_inner_nodes() {
        let l = run_clustered(&input_one_cluster()).unwrap();
        // Edge 0 (0 -> 1): last point should be at node 1's box border,
        // i.e. within half-extents of node 1's center.
        let end = *l.edge_paths[0].points.last().unwrap();
        let c1 = l.node_centers[1];
        assert!((end.0 - c1.0).abs() <= 30.0 + 1e-6);
        assert!((end.1 - c1.1).abs() <= 12.0 + 1e-6);
    }

    #[test]
    fn nested_clusters_nest_rects() {
        let input = LayoutInput {
            nodes: vec![node_in(Some(0)), node_in(Some(1))],
            edges: vec![e(0, 1)],
            clusters: vec![
                LCluster { parent: None, title: (40.0, 16.0), direction: None },
                LCluster { parent: Some(0), title: (40.0, 16.0), direction: None },
            ],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        let outer = &l.cluster_rects[0];
        let inner = &l.cluster_rects[1];
        assert!(inner.x >= outer.x && inner.y >= outer.y);
        assert!(inner.x + inner.w <= outer.x + outer.w + 1e-6);
        assert!(inner.y + inner.h <= outer.y + outer.h + 1e-6);
        assert!(inside(l.node_centers[1], inner));
    }

    #[test]
    fn lr_direction_applies_to_whole() {
        let mut input = input_one_cluster();
        input.direction = Direction::LR;
        let l = run_clustered(&input).unwrap();
        // Chain flows rightward overall.
        assert!(l.node_centers[3].0 > l.node_centers[0].0);
    }

    #[test]
    fn subgraph_direction_overrides_parent() {
        // Parent graph is TB, but the subgraph declares `direction LR`: its
        // two members must flow left-to-right (node 2 right of node 1, same
        // row) instead of top-to-bottom, while still nesting in the cluster.
        // Node 0 stays unlinked: a member linked outside would make the
        // subgraph ignore its direction (#275, see the next test).
        let input = LayoutInput {
            nodes: vec![node_in(None), node_in(Some(0)), node_in(Some(0))],
            edges: vec![e(1, 2)],
            clusters: vec![LCluster {
                parent: None,
                title: (50.0, 16.0),
                direction: Some(Direction::LR),
            }],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        assert!(
            l.node_centers[2].0 > l.node_centers[1].0 + 20.0,
            "LR subgraph: node 2 {:?} must be right of node 1 {:?}",
            l.node_centers[2],
            l.node_centers[1]
        );
        assert!(
            (l.node_centers[2].1 - l.node_centers[1].1).abs() < 20.0,
            "LR subgraph: members share a row (y close): {:?} {:?}",
            l.node_centers[1],
            l.node_centers[2]
        );
        let r = &l.cluster_rects[0];
        assert!(inside(l.node_centers[1], r));
        assert!(inside(l.node_centers[2], r));
        assert!(!inside(l.node_centers[0], r));
    }

    #[test]
    fn subgraph_direction_is_ignored_when_a_member_links_outside() {
        // mermaid.js: a subgraph with a member linked to the outside
        // inherits the parent direction instead of its own (#275).
        let input = LayoutInput {
            nodes: vec![node_in(None), node_in(Some(0)), node_in(Some(0))],
            edges: vec![e(0, 1), e(1, 2)],
            clusters: vec![LCluster {
                parent: None,
                title: (50.0, 16.0),
                direction: Some(Direction::LR),
            }],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        assert!(
            l.node_centers[2].1 > l.node_centers[1].1 + 10.0,
            "parent TB applies: node 2 {:?} below node 1 {:?}",
            l.node_centers[2],
            l.node_centers[1]
        );
    }

    /// Does the segment a→b pass through the box (shrunk by `inset` so
    /// grazing a corner doesn't count)?
    fn segment_hits_box(a: (f64, f64), b: (f64, f64), c: (f64, f64), half: (f64, f64), inset: f64) -> bool {
        let (hx, hy) = (half.0 - inset, half.1 - inset);
        if hx <= 0.0 || hy <= 0.0 {
            return false;
        }
        // Liang–Barsky clip against the box.
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        for (p, q) in [(-dx, a.0 - (c.0 - hx)), (dx, (c.0 + hx) - a.0), (-dy, a.1 - (c.1 - hy)), (dy, (c.1 + hy) - a.1)] {
            if p.abs() < 1e-12 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let r = q / p;
                if p < 0.0 { t0 = t0.max(r) } else { t1 = t1.min(r) }
                if t0 > t1 {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn edge_leaving_a_cluster_does_not_cross_a_sibling() {
        // #275: `subgraph S: A-->B end; A-->C` drew A→C straight through B.
        let input = LayoutInput {
            nodes: vec![node_in(Some(0)), node_in(Some(0)), node_in(None)],
            edges: vec![e(0, 1), e(0, 2)],
            clusters: vec![LCluster { parent: None, title: (30.0, 16.0), direction: None }],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        let p = &l.edge_paths.iter().find(|p| p.edge == 1).unwrap().points;
        for w in p.windows(2) {
            assert!(
                !segment_hits_box(w[0], w[1], l.node_centers[1], (30.0, 12.0), 1.0),
                "A→C segment {w:?} crosses B at {:?}",
                l.node_centers[1]
            );
        }
        // Still runs from A's border to C's border.
        let (start, end) = (p[0], *p.last().unwrap());
        assert!((start.0 - l.node_centers[0].0).abs() <= 30.0 + 1e-6 && (start.1 - l.node_centers[0].1).abs() <= 12.0 + 1e-6);
        assert!((end.0 - l.node_centers[2].0).abs() <= 30.0 + 1e-6 && (end.1 - l.node_centers[2].1).abs() <= 12.0 + 1e-6);
    }

    #[test]
    fn same_members_flow_vertically_without_subgraph_direction() {
        // Contrast to `subgraph_direction_overrides_parent`: the identical
        // graph with a TB (default) subgraph stacks the members vertically.
        let input = LayoutInput {
            nodes: vec![node_in(None), node_in(Some(0)), node_in(Some(0))],
            edges: vec![e(0, 1), e(1, 2)],
            clusters: vec![LCluster { parent: None, title: (50.0, 16.0), direction: None }],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        assert!(
            l.node_centers[2].1 > l.node_centers[1].1 + 10.0,
            "TB subgraph: node 2 below node 1"
        );
    }

    #[test]
    fn empty_cluster_is_harmless() {
        let input = LayoutInput {
            nodes: vec![node_in(None)],
            edges: vec![],
            clusters: vec![LCluster { parent: None, title: (40.0, 16.0), direction: None }],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        // Rect exists (title-sized minimum), no panic.
        assert_eq!(l.cluster_rects.len(), 1);
        assert!(l.cluster_rects[0].w > 0.0);
    }

    #[test]
    fn self_loop_in_cluster_emitted_exactly_once() {
        // 0 top-level -> 1 (in cluster); 1 -> 1 self-loop.
        let input = LayoutInput {
            nodes: vec![node_in(None), node_in(Some(0))],
            edges: vec![e(0, 1), e(1, 1)],
            clusters: vec![LCluster { parent: None, title: (50.0, 16.0), direction: None }],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        assert_eq!(l.edge_paths.len(), input.edges.len());
        let loops: Vec<_> = l.edge_paths.iter().filter(|p| p.edge == 1).collect();
        assert_eq!(loops.len(), 1, "self-loop must be emitted exactly once");
        for p in &loops[0].points {
            assert!(p.0.is_finite() && p.1.is_finite());
        }
    }

    #[test]
    fn self_loop_in_nested_cluster_emitted_exactly_once() {
        // 0 top-level -> 1 (in inner cluster, two levels deep); 1 -> 1.
        let input = LayoutInput {
            nodes: vec![node_in(None), node_in(Some(1))],
            edges: vec![e(0, 1), e(1, 1)],
            clusters: vec![
                LCluster { parent: None, title: (50.0, 16.0), direction: None },
                LCluster { parent: Some(0), title: (50.0, 16.0), direction: None },
            ],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        assert_eq!(l.edge_paths.len(), input.edges.len());
        let loops: Vec<_> = l.edge_paths.iter().filter(|p| p.edge == 1).collect();
        assert_eq!(loops.len(), 1, "self-loop must be emitted exactly once");
        for p in &loops[0].points {
            assert!(p.0.is_finite() && p.1.is_finite());
        }
    }

    #[test]
    fn lr_clustered_wide_short_nodes_no_overlap() {
        // Two wide-short members (120x20) inside a cluster, plus one
        // top-level node, direction LR. Before the fix, `build_level`
        // hardcoded TB on every per-level input (no transpose-in), while
        // the single terminal `apply_direction` swapped the untransposed
        // geometry — wide/short nodes' clearances landed on the wrong
        // final axis and overlapped.
        let input = LayoutInput {
            nodes: vec![
                LNode { width: 60.0, height: 24.0, cluster: None },
                LNode { width: 120.0, height: 20.0, cluster: Some(0) },
                LNode { width: 120.0, height: 20.0, cluster: Some(0) },
            ],
            edges: vec![e(0, 1), e(1, 2)],
            clusters: vec![LCluster { parent: None, title: (50.0, 16.0), direction: None }],
            direction: Direction::LR,
        };
        let l = run_clustered(&input).unwrap();

        // Pairwise non-overlap using true node dimensions: same
        // x_sep-or-y_sep check as `flat_no_node_overlaps` in props.rs.
        for i in 0..input.nodes.len() {
            for j in i + 1..input.nodes.len() {
                let (ci, cj) = (l.node_centers[i], l.node_centers[j]);
                let (ni, nj) = (&input.nodes[i], &input.nodes[j]);
                let x_sep = (ci.0 - cj.0).abs() >= (ni.width + nj.width) / 2.0 - 1e-6;
                let y_sep = (ci.1 - cj.1).abs() >= (ni.height + nj.height) / 2.0 - 1e-6;
                assert!(x_sep || y_sep, "nodes {i} and {j} overlap: {ci:?} {cj:?}");
            }
        }

        // Members land inside their cluster rect; the top-level node does not.
        let r = &l.cluster_rects[0];
        assert!(
            inside(l.node_centers[1], r),
            "node 1 center {:?} outside cluster rect {r:?}",
            l.node_centers[1]
        );
        assert!(
            inside(l.node_centers[2], r),
            "node 2 center {:?} outside cluster rect {r:?}",
            l.node_centers[2]
        );
        assert!(!inside(l.node_centers[0], r));
    }
}
