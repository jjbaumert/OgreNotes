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
            pts[0] = clip_to_box(a_c, (na.width / 2.0, na.height / 2.0), a_next);
            let n = pts.len();
            pts[n - 1] = clip_to_box(b_c, (nb.width / 2.0, nb.height / 2.0), b_prev);
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
