//! Stage 5: assemble edge polylines through dummy waypoints, clip
//! endpoints at node bounding boxes, restore true direction for
//! reversed edges, and stub self-loops.

use super::acyclic::AcyclicResult;
use super::order::{OrderGraph, SlotKind};
use super::position::Coords;
use super::{EdgePath, LayoutInput};

/// Clip point `toward` -> `center` at the bounding box of a node with
/// the given half-extents, returning the border intersection.
pub(crate) fn clip_to_box(center: (f64, f64), half: (f64, f64), toward: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (toward.0 - center.0, toward.1 - center.1);
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return center;
    }
    let tx = if dx.abs() < 1e-9 { f64::INFINITY } else { half.0 / dx.abs() };
    let ty = if dy.abs() < 1e-9 { f64::INFINITY } else { half.1 / dy.abs() };
    let t = tx.min(ty).min(1.0);
    (center.0 + dx * t, center.1 + dy * t)
}

pub(crate) fn route_edges(
    input: &LayoutInput,
    ac: &AcyclicResult,
    g: &OrderGraph,
    coords: &Coords,
) -> Vec<EdgePath> {
    let mut out: Vec<EdgePath> = Vec::with_capacity(input.edges.len());

    // Per node-side edge counts (in rank space, a chain's source attaches
    // at its bottom, its target at its top). A side with exactly one edge
    // gets that edge centred on the box border rather than offset toward
    // the neighbour — so e.g. the middle nodes of a diamond take their lone
    // arrow head-on, while a fan-in/fan-out node keeps its spread.
    let mut bottom_count = vec![0u32; input.nodes.len()];
    let mut top_count = vec![0u32; input.nodes.len()];
    for chain in &g.chains {
        if let Some(SlotKind::Real(a)) = chain.first().copied() {
            bottom_count[a] += 1;
        }
        if let Some(SlotKind::Real(b)) = chain.last().copied() {
            top_count[b] += 1;
        }
    }

    // Surviving (non-self-loop) edges, chain index == acyclic edge index.
    for (ai, chain) in g.chains.iter().enumerate() {
        let orig = ac.orig[ai];
        let mut pts: Vec<(f64, f64)> = Vec::with_capacity(chain.len());
        let mut label_at = None;
        for kind in chain {
            let c = coords.centers[kind];
            if let SlotKind::Dummy { edge, .. } = kind {
                if label_at.is_none() && input.edges[ac.orig[*edge]].label.is_some() {
                    label_at = Some(c);
                }
            }
            pts.push(c);
        }
        // Clip first/last segment at the endpoint node boxes.
        let first_kind = chain.first().copied().unwrap();
        let last_kind = chain.last().copied().unwrap();
        if let (SlotKind::Real(a), SlotKind::Real(b)) = (first_kind, last_kind) {
            let na = &input.nodes[a];
            let nb = &input.nodes[b];
            let a_c = pts[0];
            let b_c = *pts.last().unwrap();
            let a_next = pts.get(1).copied().unwrap_or(b_c);
            let b_prev = pts[pts.len().saturating_sub(2)];
            // Source end (bottom side in rank space): centre it when it's
            // the node's only outgoing edge, else clip toward the waypoint.
            pts[0] = if bottom_count[a] == 1 {
                (a_c.0, a_c.1 + na.height / 2.0)
            } else {
                clip_to_box(a_c, (na.width / 2.0, na.height / 2.0), a_next)
            };
            // Target end (top side): centre it when it's the node's only
            // incoming edge.
            let n = pts.len();
            pts[n - 1] = if top_count[b] == 1 {
                (b_c.0, b_c.1 - nb.height / 2.0)
            } else {
                clip_to_box(b_c, (nb.width / 2.0, nb.height / 2.0), b_prev)
            };
        }
        // Single-span labeled edge: label at segment midpoint.
        if label_at.is_none() && input.edges[orig].label.is_some() {
            let a = pts[0];
            let b = *pts.last().unwrap();
            label_at = Some(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
        }
        let reversed = ac.reversed[orig];
        if reversed {
            pts.reverse(); // restore true direction
        }
        out.push(EdgePath { edge: orig, points: pts, label_at, reversed });
    }

    // Self-loops: small rectangle stub off the node's right edge.
    for &orig in &ac.self_loops {
        let v = input.edges[orig].from;
        // Node centers for real slots come from coords.
        let c = coords.centers[&SlotKind::Real(v)];
        let hw = input.nodes[v].width / 2.0;
        let hh = input.nodes[v].height / 2.0;
        let stub = 18.0;
        let pts = vec![
            (c.0 + hw, c.1 - hh * 0.5),
            (c.0 + hw + stub, c.1 - hh * 0.5),
            (c.0 + hw + stub, c.1 + hh * 0.5),
            (c.0 + hw, c.1 + hh * 0.5),
        ];
        let label_at = input.edges[orig]
            .label
            .map(|_| (c.0 + hw + stub + 4.0, c.1));
        out.push(EdgePath { edge: orig, points: pts, label_at, reversed: false });
    }

    // Emit in original-edge order for determinism.
    out.sort_by_key(|p| p.edge);
    out
}

#[cfg(test)]
mod tests {
    use super::clip_to_box;
    use crate::layout::{run, Direction, LEdge, LNode, LayoutInput};

    fn node() -> LNode {
        LNode { width: 40.0, height: 20.0, cluster: None }
    }
    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }
    fn labeled(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: Some((24.0, 12.0)) }
    }
    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    // ── clip_to_box (pure geometry) ─────────────────────────────

    #[test]
    fn clip_axis_aligned_hits_the_facing_border() {
        let c = (10.0, 20.0);
        let half = (5.0, 3.0);
        // Straight right → right border, same y.
        assert!(close(clip_to_box(c, half, (100.0, 20.0)), (15.0, 20.0)));
        // Straight up → top border, same x.
        assert!(close(clip_to_box(c, half, (10.0, -50.0)), (10.0, 17.0)));
        // Straight down → bottom border.
        assert!(close(clip_to_box(c, half, (10.0, 90.0)), (10.0, 23.0)));
        // Straight left → left border.
        assert!(close(clip_to_box(c, half, (-80.0, 20.0)), (5.0, 20.0)));
    }

    #[test]
    fn clip_diagonal_is_limited_by_the_nearer_axis() {
        // 45° ray from a wide-flat box: the y half-extent (3) binds before
        // the x half-extent (5), so the exit point is (c + 3, c + 3).
        let got = clip_to_box((0.0, 0.0), (5.0, 3.0), (100.0, 100.0));
        assert!(close(got, (3.0, 3.0)), "got {got:?}");
        // Steep ray toward a tall-thin box: x binds first.
        let got = clip_to_box((0.0, 0.0), (2.0, 50.0), (10.0, 10.0));
        assert!(close(got, (2.0, 2.0)), "got {got:?}");
    }

    #[test]
    fn clip_degenerate_inputs_stay_finite() {
        // toward == center → center (no direction to clip along).
        assert!(close(clip_to_box((7.0, 8.0), (5.0, 3.0), (7.0, 8.0)), (7.0, 8.0)));
        // toward strictly inside the box → t clamps to 1, returns toward
        // itself (the segment never reaches the border).
        assert!(close(clip_to_box((0.0, 0.0), (5.0, 3.0), (1.0, 1.0)), (1.0, 1.0)));
        // Zero-size box → collapses to the center.
        assert!(close(clip_to_box((4.0, 4.0), (0.0, 0.0), (9.0, 9.0)), (4.0, 4.0)));
    }

    // ── route_edges (via the full pipeline, TB = rank space) ────

    #[test]
    fn reversed_cycle_edge_is_emitted_in_true_direction() {
        // A <-> B: acyclic reverses exactly one edge; the emitted path must
        // still run true-source → true-target, with the flag set.
        let input = LayoutInput {
            nodes: vec![node(), node()],
            edges: vec![e(0, 1), e(1, 0)],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        assert_eq!(l.edge_paths.len(), 2);
        let dist = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).hypot(p.1 - q.1);
        for p in &l.edge_paths {
            let (from, to) = (input.edges[p.edge].from, input.edges[p.edge].to);
            let start = p.points[0];
            let end = *p.points.last().unwrap();
            assert!(
                dist(start, l.node_centers[from]) < dist(start, l.node_centers[to]),
                "edge {} starts nearer its target than its source",
                p.edge
            );
            assert!(
                dist(end, l.node_centers[to]) < dist(end, l.node_centers[from]),
                "edge {} ends nearer its source than its target",
                p.edge
            );
        }
        assert_eq!(
            l.edge_paths.iter().filter(|p| p.reversed).count(),
            1,
            "exactly one edge of a 2-cycle is reversed"
        );
    }

    #[test]
    fn self_loop_stubs_off_the_right_edge_and_sorts_into_edge_order() {
        // Self-loop listed FIRST so the deterministic sort (not emission
        // order — self-loops are appended after chains) is what's pinned.
        let input = LayoutInput {
            nodes: vec![node(), node()],
            edges: vec![labeled(0, 0), e(0, 1)],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        assert_eq!(l.edge_paths.len(), 2);
        assert_eq!(l.edge_paths[0].edge, 0, "paths must come back in original edge order");
        assert_eq!(l.edge_paths[1].edge, 1);

        let (cx, cy) = l.node_centers[0];
        let (hw, hh) = (20.0, 10.0); // node() is 40×20
        let stub = &l.edge_paths[0];
        assert_eq!(stub.points.len(), 4, "self-loop is a 4-point stub");
        assert!(close(stub.points[0], (cx + hw, cy - hh * 0.5)), "stub leaves the right border");
        assert!(close(stub.points[3], (cx + hw, cy + hh * 0.5)), "stub re-enters the right border");
        assert!(
            stub.points.iter().all(|p| p.0 >= cx + hw - 1e-9),
            "stub stays right of the node"
        );
        let label = stub.label_at.expect("labeled self-loop places a label");
        assert!(label.0 > cx + hw, "self-loop label sits beyond the stub");
        assert!(!stub.reversed);
    }

    #[test]
    fn single_span_label_sits_at_the_segment_midpoint() {
        let input = LayoutInput {
            nodes: vec![node(), node()],
            edges: vec![labeled(0, 1)],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        let p = &l.edge_paths[0];
        let a = p.points[0];
        let b = *p.points.last().unwrap();
        let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        assert!(close(p.label_at.expect("label placed"), mid));
    }

    #[test]
    fn multi_rank_label_sits_at_the_dummy_waypoint() {
        // A->B, B->C put C two ranks below A; the labeled A->C edge then
        // routes through one dummy waypoint, which hosts the label.
        let input = LayoutInput {
            nodes: vec![node(), node(), node()],
            edges: vec![e(0, 1), e(1, 2), labeled(0, 2)],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        let long = l.edge_paths.iter().find(|p| p.edge == 2).unwrap();
        assert_eq!(long.points.len(), 3, "one dummy waypoint between the endpoints");
        assert!(
            close(long.label_at.expect("label placed"), long.points[1]),
            "label rides the dummy waypoint, not an endpoint"
        );
    }

    #[test]
    fn every_endpoint_lies_on_its_node_border() {
        // Both the centred (single-edge side) and clipped (multi-edge side)
        // branches must land exactly on the endpoint node's bounding box.
        let input = LayoutInput {
            nodes: vec![node(), node(), node(), node()],
            edges: vec![e(0, 1), e(0, 2), e(1, 3), e(2, 3)],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        let on_border = |p: (f64, f64), n: usize| {
            let (cx, cy) = l.node_centers[n];
            let (dx, dy) = ((p.0 - cx).abs(), (p.1 - cy).abs());
            let (hw, hh) = (20.0, 10.0);
            ((dx - hw).abs() < 1e-6 && dy <= hh + 1e-6)
                || ((dy - hh).abs() < 1e-6 && dx <= hw + 1e-6)
        };
        for p in &l.edge_paths {
            let (from, to) = (input.edges[p.edge].from, input.edges[p.edge].to);
            assert!(on_border(p.points[0], from), "edge {} start off-border", p.edge);
            assert!(on_border(*p.points.last().unwrap(), to), "edge {} end off-border", p.edge);
        }
    }

    #[test]
    fn single_edge_side_centres_endpoint_multi_edge_side_stays_spread() {
        // Diamond A->B, A->C, B->D, C->D. B and C have one edge per side, so
        // those attach at the box centre; A (2 out) / D (2 in) stay spread.
        let input = LayoutInput {
            nodes: vec![node(), node(), node(), node()],
            edges: vec![e(0, 1), e(0, 2), e(1, 3), e(2, 3)],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        let cx = |i: usize| l.node_centers[i].0;
        let ep = |edge: usize| l.edge_paths.iter().find(|p| p.edge == edge).unwrap();

        // A->B arrives at B's (node 1) top-centre; B->D leaves B's bottom-centre.
        assert!((ep(0).points.last().unwrap().0 - cx(1)).abs() < 1e-6, "A->B not centred on B");
        assert!((ep(2).points[0].0 - cx(1)).abs() < 1e-6, "B->D not centred on B");
        // A (node 0, two outgoing) keeps its exits spread, not both at A.cx.
        assert!(
            (ep(0).points[0].0 - cx(0)).abs() > 1e-6 || (ep(1).points[0].0 - cx(0)).abs() > 1e-6,
            "fan-out endpoints should not both sit at the centre"
        );
        // D (node 3, two incoming) keeps its entries spread.
        assert!(
            (ep(2).points.last().unwrap().0 - cx(3)).abs() > 1e-6
                || (ep(3).points.last().unwrap().0 - cx(3)).abs() > 1e-6,
            "fan-in endpoints should not both sit at the centre"
        );
    }
}
