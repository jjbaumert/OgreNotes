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
//! (using representative placeholders for whichever side collapsed), then
//! have their cluster-side endpoint re-clipped to the true inner node's
//! absolute box so arrows terminate on the real node, not the cluster
//! border. `apply_direction` runs exactly once, on the fully assembled
//! whole.

use super::route;
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
}

/// One level of the collapse hierarchy: either a single cluster's induced
/// subgraph, or the top-level graph (real top-level nodes + top-level
/// cluster placeholders). Laid out in isolation, in local TB coordinates.
struct SubBuild {
    /// Direct members of this level, in the same order as `layout`'s nodes.
    entities: Vec<Entity>,
    /// This level's induced edges, `from`/`to` indexing into `entities`.
    local_edges: Vec<LEdge>,
    /// Parallel to `local_edges`: the ORIGINAL `LayoutInput::edges` index.
    local_edge_orig: Vec<usize>,
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
    let mut local_edges = Vec::new();
    let mut local_edge_orig = Vec::new();
    for (ei, edge) in input.edges.iter().enumerate() {
        let rf = representative_of(input, edge.from, target);
        let rt = representative_of(input, edge.to, target);
        if let (Some(rf), Some(rt)) = (rf, rt) {
            if rf != rt || edge.from == edge.to {
                local_edges.push(LEdge {
                    from: entity_index[&rf],
                    to: entity_index[&rt],
                    label: edge.label,
                });
                local_edge_orig.push(ei);
            }
        }
    }

    let mut local_nodes: Vec<LNode> = Vec::with_capacity(entities.len());
    for e in &entities {
        match *e {
            Entity::Node(v) => local_nodes.push(input.nodes[v].clone()),
            Entity::Cluster(c) => {
                let (w, h) = sub_builds[c]
                    .as_ref()
                    .expect("child clusters are built before their parent")
                    .placeholder_size;
                local_nodes.push(LNode { width: w, height: h, cluster: None });
            }
        }
    }

    let local_input = LayoutInput {
        nodes: local_nodes,
        edges: local_edges.clone(),
        clusters: vec![],
        direction: Direction::TB,
    };
    let layout = layout_tb(&local_input)?;

    Ok(SubBuild {
        entities,
        local_edges,
        local_edge_orig,
        layout,
        placeholder_size: (0.0, 0.0),
    })
}

/// Recursively translates `build`'s local layout into absolute coordinates
/// by `translate`, writing real node centers and cluster rects into the
/// shared output buffers, and appending this level's edges (with
/// cluster-side endpoints re-clipped to the true inner node once known).
fn expand_level(
    input: &LayoutInput,
    build: &SubBuild,
    translate: (f64, f64),
    sub_builds: &[Option<SubBuild>],
    node_centers: &mut [(f64, f64)],
    cluster_rects: &mut [Option<Rect>],
    edges_out: &mut Vec<EdgePath>,
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
                let content_translate = (rect.x, rect.y + title_h + CLUSTER_PAD);
                cluster_rects[c] = Some(rect);
                expand_level(
                    input,
                    child,
                    content_translate,
                    sub_builds,
                    node_centers,
                    cluster_rects,
                    edges_out,
                );
            }
        }
    }

    for ep in &build.layout.edge_paths {
        let orig = build.local_edge_orig[ep.edge];
        let local_edge = &build.local_edges[ep.edge];
        let mut pts: Vec<(f64, f64)> = ep
            .points
            .iter()
            .map(|p| (p.0 + translate.0, p.1 + translate.1))
            .collect();
        let label_at = ep.label_at.map(|p| (p.0 + translate.0, p.1 + translate.1));

        // Re-clip cluster-side endpoints to the true inner node's box: the
        // top-level route clipped against the PLACEHOLDER's box, not the
        // real node buried inside it. By now every real node under this
        // level's entities has an absolute center (set by the recursion
        // just above), regardless of nesting depth.
        if let Entity::Cluster(_) = build.entities[local_edge.from] {
            let true_v = input.edges[orig].from;
            let center = node_centers[true_v];
            let half = (input.nodes[true_v].width / 2.0, input.nodes[true_v].height / 2.0);
            let toward = pts.get(1).copied().unwrap_or(center);
            pts[0] = route::clip_to_box(center, half, toward);
        }
        if let Entity::Cluster(_) = build.entities[local_edge.to] {
            let true_v = input.edges[orig].to;
            let center = node_centers[true_v];
            let half = (input.nodes[true_v].width / 2.0, input.nodes[true_v].height / 2.0);
            let n = pts.len();
            let toward = if n >= 2 { pts[n - 2] } else { center };
            pts[n - 1] = route::clip_to_box(center, half, toward);
        }

        edges_out.push(EdgePath { edge: orig, points: pts, label_at, reversed: ep.reversed });
    }
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

    let mut sub_builds: Vec<Option<SubBuild>> = (0..input.clusters.len()).map(|_| None).collect();
    for c in order {
        let build = build_level(input, Some(c), &sub_builds)?;
        let title_h = input.clusters[c].title.1;
        let placeholder_size = (build.layout.size.0, build.layout.size.1 + title_h + CLUSTER_PAD);
        sub_builds[c] = Some(SubBuild { placeholder_size, ..build });
    }

    let top = build_level(input, None, &sub_builds)?;

    let mut node_centers = vec![(0.0, 0.0); input.nodes.len()];
    let mut cluster_rects: Vec<Option<Rect>> = (0..input.clusters.len()).map(|_| None).collect();
    let mut edges_out: Vec<EdgePath> = Vec::new();
    expand_level(
        input,
        &top,
        (0.0, 0.0),
        &sub_builds,
        &mut node_centers,
        &mut cluster_rects,
        &mut edges_out,
    );
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

    let mut layout =
        Layout { node_centers, edge_paths: edges_out, cluster_rects, size: top.layout.size };
    apply_direction(&mut layout, input.direction);
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
            clusters: vec![LCluster { parent: None, title: (50.0, 16.0) }],
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
                LCluster { parent: None, title: (40.0, 16.0) },
                LCluster { parent: Some(0), title: (40.0, 16.0) },
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
    fn empty_cluster_is_harmless() {
        let input = LayoutInput {
            nodes: vec![node_in(None)],
            edges: vec![],
            clusters: vec![LCluster { parent: None, title: (40.0, 16.0) }],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        // Rect exists (title-sized minimum), no panic.
        assert_eq!(l.cluster_rects.len(), 1);
        assert!(l.cluster_rects[0].w > 0.0);
    }
}
