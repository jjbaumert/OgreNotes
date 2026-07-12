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
    use crate::layout::{run, Direction, LEdge, LNode, LayoutInput};

    fn node() -> LNode {
        LNode { width: 40.0, height: 20.0, cluster: None }
    }
    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
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
