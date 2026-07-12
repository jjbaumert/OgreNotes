//! Stage 4: x-coordinates via median-of-neighbors with separation
//! enforcement (3 bounded sweeps: down, up, down). Deliberately simpler
//! than Brandes-Köpf; replaceable behind this API.

use super::order::{OrderGraph, SlotKind};
use super::{NODE_GAP_X, RANK_GAP_Y};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::order::build_order_graph;
    use crate::layout::{LEdge, LNode};

    fn n(w: f64) -> LNode {
        LNode { width: w, height: 20.0, cluster: None }
    }

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    fn no_overlap(g: &OrderGraph, c: &Coords) {
        for row in &g.ranks {
            for w in row.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                let ax = c.centers[&a.kind].0;
                let bx = c.centers[&b.kind].0;
                assert!(
                    bx - b.size.0 / 2.0 - (ax + a.size.0 / 2.0) >= NODE_GAP_X - 1e-6,
                    "overlap: {:?} at {ax} vs {:?} at {bx}",
                    a.kind,
                    b.kind
                );
            }
        }
    }

    #[test]
    fn chain_is_vertically_stacked_and_aligned() {
        let nodes = vec![n(40.0), n(40.0), n(40.0)];
        let edges = vec![e(0, 1), e(1, 2)];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 2]);
        let c = assign_coords(&g);
        let x0 = c.centers[&SlotKind::Real(0)].0;
        let x1 = c.centers[&SlotKind::Real(1)].0;
        let x2 = c.centers[&SlotKind::Real(2)].0;
        assert!((x0 - x1).abs() < 1e-6 && (x1 - x2).abs() < 1e-6);
        let y0 = c.centers[&SlotKind::Real(0)].1;
        let y1 = c.centers[&SlotKind::Real(1)].1;
        assert!(y1 - y0 >= 20.0 + RANK_GAP_Y - 1e-6);
    }

    #[test]
    fn siblings_never_overlap() {
        let nodes = vec![n(120.0), n(120.0), n(120.0), n(40.0)];
        let edges = vec![e(3, 0), e(3, 1), e(3, 2)];
        let g = build_order_graph(&nodes, &edges, &[1, 1, 1, 0]);
        let c = assign_coords(&g);
        no_overlap(&g, &c);
    }

    #[test]
    fn parent_centers_over_children() {
        let nodes = vec![n(40.0), n(40.0), n(40.0)];
        let edges = vec![e(0, 1), e(0, 2)];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 1]);
        let c = assign_coords(&g);
        let px = c.centers[&SlotKind::Real(0)].0;
        let c1 = c.centers[&SlotKind::Real(1)].0;
        let c2 = c.centers[&SlotKind::Real(2)].0;
        let mid = (c1 + c2) / 2.0;
        assert!((px - mid).abs() < 1.0, "parent {px} vs children mid {mid}");
    }

    #[test]
    fn diamond_merge_node_centers_under_both_parents() {
        // A->B, A->C, B->D, C->D. The classic diamond must be SYMMETRIC:
        // D (the merge) centered under B and C, just like A (the split) is
        // centered over them — an even-neighbor median must average the two
        // middles, not snap D to the right parent.
        let nodes = vec![n(40.0), n(40.0), n(40.0), n(40.0)];
        let edges = vec![e(0, 1), e(0, 2), e(1, 3), e(2, 3)];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 1, 2]);
        let c = assign_coords(&g);
        let a = c.centers[&SlotKind::Real(0)].0;
        let b = c.centers[&SlotKind::Real(1)].0;
        let cc = c.centers[&SlotKind::Real(2)].0;
        let d = c.centers[&SlotKind::Real(3)].0;
        let mid = (b + cc) / 2.0;
        assert!((d - mid).abs() < 1.0, "D {d} not centered under B,C mid {mid}");
        assert!((a - mid).abs() < 1.0, "A {a} not centered over B,C mid {mid}");
        assert!((a - d).abs() < 1.0, "diamond asymmetric: A {a} vs D {d}");
    }

    #[test]
    fn all_coords_finite_and_on_canvas() {
        let nodes = vec![n(40.0), n(40.0), n(40.0), n(40.0), n(40.0)];
        let edges = vec![e(0, 2), e(1, 2), e(2, 3), e(2, 4), e(0, 4)];
        let g = build_order_graph(&nodes, &edges, &[0, 0, 1, 2, 2]);
        let c = assign_coords(&g);
        for row in &g.ranks {
            for s in row {
                let (x, y) = c.centers[&s.kind];
                assert!(x.is_finite() && y.is_finite());
                assert!(x - s.size.0 / 2.0 >= -1e-6);
                assert!(y - s.size.1 / 2.0 >= -1e-6);
                assert!(x + s.size.0 / 2.0 <= c.size.0 + 1e-6);
                assert!(y + s.size.1 / 2.0 <= c.size.1 + 1e-6);
            }
        }
        no_overlap(&g, &c);
    }
}

pub(crate) struct Coords {
    pub centers: std::collections::HashMap<SlotKind, (f64, f64)>,
    pub size: (f64, f64),
}

const COORD_SWEEPS: usize = 3;
const CANVAS_PAD: f64 = 20.0;

pub(crate) fn assign_coords(g: &OrderGraph) -> Coords {
    // y: rank tops from cumulative max heights.
    let rank_h: Vec<f64> = g
        .ranks
        .iter()
        .map(|row| row.iter().map(|s| s.size.1).fold(0.0, f64::max))
        .collect();
    let mut rank_y = Vec::with_capacity(g.ranks.len());
    let mut y = CANVAS_PAD;
    for h in &rank_h {
        rank_y.push(y + h / 2.0);
        y += h + RANK_GAP_Y;
    }

    // x: initial packing left-to-right per rank.
    let mut xs: Vec<Vec<f64>> = g
        .ranks
        .iter()
        .map(|row| {
            let mut out = Vec::with_capacity(row.len());
            let mut x = CANVAS_PAD;
            for s in row {
                out.push(x + s.size.0 / 2.0);
                x += s.size.0 + NODE_GAP_X;
            }
            out
        })
        .collect();

    // Median sweeps with separation enforcement.
    for sweep in 0..COORD_SWEEPS {
        let downward = sweep % 2 == 0;
        let ranks_iter: Vec<usize> = if downward {
            (1..g.ranks.len()).collect()
        } else {
            (0..g.ranks.len().saturating_sub(1)).rev().collect()
        };
        for r in ranks_iter {
            let nbrs = super::order::neighbor_positions(g, r, downward);
            let adj = if downward { r - 1 } else { r + 1 };
            // Desired x = median of neighbor centers on the adjacent rank.
            let desired: Vec<f64> = g.ranks[r]
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    if nbrs[i].is_empty() {
                        xs[r][i]
                    } else {
                        let mut vals: Vec<f64> =
                            nbrs[i].iter().map(|&p| xs[adj][p]).collect();
                        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        // True median: for an EVEN neighbor count, average the
                        // two middle values so a merge node lands centered
                        // between its neighbors (e.g. the bottom of a diamond
                        // over both parents), not snapped to the upper one.
                        let mid = vals.len() / 2;
                        if vals.len() % 2 == 0 {
                            (vals[mid - 1] + vals[mid]) / 2.0
                        } else {
                            vals[mid]
                        }
                    }
                })
                .collect();
            xs[r] = desired;
            enforce_separation(&g.ranks[r], &mut xs[r]);
        }
    }

    // Normalize: shift so min left edge = CANVAS_PAD; compute canvas size.
    let mut min_left = f64::INFINITY;
    let mut max_right = f64::NEG_INFINITY;
    for (r, row) in g.ranks.iter().enumerate() {
        for (i, s) in row.iter().enumerate() {
            min_left = min_left.min(xs[r][i] - s.size.0 / 2.0);
            max_right = max_right.max(xs[r][i] + s.size.0 / 2.0);
        }
    }
    if !min_left.is_finite() {
        min_left = 0.0;
        max_right = 0.0;
    }
    let shift = CANVAS_PAD - min_left;

    let mut centers = std::collections::HashMap::new();
    for (r, row) in g.ranks.iter().enumerate() {
        for (i, s) in row.iter().enumerate() {
            centers.insert(s.kind, (xs[r][i] + shift, rank_y[r]));
        }
    }
    let total_h = y - RANK_GAP_Y + CANVAS_PAD; // y overshoots by one gap
    let size = (max_right + shift + CANVAS_PAD, total_h.max(2.0 * CANVAS_PAD));
    Coords { centers, size }
}

/// Two-pass min-gap enforcement that preserves order: push right, then
/// push left, then average toward desired where slack allows. Guarantees
/// gap >= NODE_GAP_X between horizontal extents.
fn enforce_separation(row: &[super::order::Slot], xs: &mut [f64]) {
    let n = row.len();
    if n < 2 {
        return;
    }
    let mut right = xs.to_vec();
    for i in 1..n {
        let min_x =
            right[i - 1] + row[i - 1].size.0 / 2.0 + NODE_GAP_X + row[i].size.0 / 2.0;
        if right[i] < min_x {
            right[i] = min_x;
        }
    }
    let mut left = xs.to_vec();
    for i in (0..n - 1).rev() {
        let max_x =
            left[i + 1] - row[i + 1].size.0 / 2.0 - NODE_GAP_X - row[i].size.0 / 2.0;
        if left[i] > max_x {
            left[i] = max_x;
        }
    }
    for i in 0..n {
        xs[i] = (right[i] + left[i]) / 2.0;
    }
    // Final hard pass so float averaging can't reintroduce an overlap.
    for i in 1..n {
        let min_x = xs[i - 1] + row[i - 1].size.0 / 2.0 + NODE_GAP_X + row[i].size.0 / 2.0;
        if xs[i] < min_x {
            xs[i] = min_x;
        }
    }
}
