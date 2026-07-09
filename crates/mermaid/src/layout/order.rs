//! Stage 3: per-rank ordering. Multi-rank edges are split through dummy
//! slots (edge labels reserve space as sized dummies); barycenter sweeps
//! reduce crossings; iteration capped; best-seen ordering wins.

use super::{LEdge, LNode};

#[cfg(test)]
mod tests {
    use super::*;

    fn n() -> LNode {
        LNode { width: 20.0, height: 10.0, cluster: None }
    }

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    #[test]
    fn real_slots_land_on_their_ranks() {
        let nodes = vec![n(), n(), n()];
        let edges = vec![e(0, 1), e(1, 2)];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 2]);
        assert_eq!(g.ranks.len(), 3);
        assert!(matches!(g.ranks[0][0].kind, SlotKind::Real(0)));
        assert!(matches!(g.ranks[1][0].kind, SlotKind::Real(1)));
    }

    #[test]
    fn long_edge_gets_dummies_on_intermediate_ranks() {
        let nodes = vec![n(), n(), n(), n()];
        // 0 (rank 0) -> 3 (rank 3): dummies on ranks 1 and 2.
        let edges = vec![e(0, 1), e(1, 2), e(2, 3), e(0, 3)];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 2, 3]);
        let dummies: Vec<_> = g.ranks[1]
            .iter()
            .filter(|s| matches!(s.kind, SlotKind::Dummy { edge: 3, .. }))
            .collect();
        assert_eq!(dummies.len(), 1);
        assert_eq!(g.chains[3].len(), 4); // from, d1, d2, to
    }

    #[test]
    fn labeled_long_edge_dummy_carries_label_size() {
        let nodes = vec![n(), n(), n()];
        let edges = vec![
            e(0, 1),
            e(1, 2),
            LEdge { from: 0, to: 2, label: Some((30.0, 12.0)) },
        ];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 2]);
        let d = g.ranks[1]
            .iter()
            .find(|s| matches!(s.kind, SlotKind::Dummy { edge: 2, .. }))
            .expect("dummy");
        assert_eq!(d.size, (30.0, 12.0));
    }

    #[test]
    fn crossing_count_detects_the_classic_x() {
        // rank0: [0, 1], rank1: [2, 3]; edges 0->3 and 1->2 cross.
        let nodes = vec![n(), n(), n(), n()];
        let edges = vec![e(0, 3), e(1, 2)];
        let g = build_order_graph(&nodes, &edges, &[0, 0, 1, 1]);
        assert_eq!(count_crossings(&g, &edges), 1);
    }

    #[test]
    fn barycenter_removes_removable_crossings() {
        let nodes = vec![n(), n(), n(), n()];
        let edges = vec![e(0, 3), e(1, 2)];
        let mut g = build_order_graph(&nodes, &edges, &[0, 0, 1, 1]);
        minimize_crossings(&mut g, &edges);
        assert_eq!(count_crossings(&g, &edges), 0);
    }

    #[test]
    fn minimize_never_increases_crossings() {
        // K2,2 has an unavoidable crossing; must not get worse than input.
        let nodes = vec![n(), n(), n(), n()];
        let edges = vec![e(0, 2), e(0, 3), e(1, 2), e(1, 3)];
        let mut g = build_order_graph(&nodes, &edges, &[0, 0, 1, 1]);
        let before = count_crossings(&g, &edges);
        minimize_crossings(&mut g, &edges);
        assert!(count_crossings(&g, &edges) <= before);
    }

    #[test]
    fn deterministic_ordering() {
        let nodes = vec![n(), n(), n(), n(), n(), n()];
        let edges = vec![e(0, 4), e(1, 3), e(2, 5), e(0, 5), e(1, 4)];
        let ranks = vec![0, 0, 0, 1, 1, 1];
        let mut a = build_order_graph(&nodes, &edges, &ranks);
        let mut b = build_order_graph(&nodes, &edges, &ranks);
        minimize_crossings(&mut a, &edges);
        minimize_crossings(&mut b, &edges);
        let ka: Vec<Vec<String>> = a
            .ranks
            .iter()
            .map(|r| r.iter().map(|s| format!("{:?}", s.kind)).collect())
            .collect();
        let kb: Vec<Vec<String>> = b
            .ranks
            .iter()
            .map(|r| r.iter().map(|s| format!("{:?}", s.kind)).collect())
            .collect();
        assert_eq!(ka, kb);
    }

    #[test]
    fn large_graph_minimize_crossings_completes_and_is_deterministic() {
        // Smoke bound at roughly engine-cap shape: a long chain plus a
        // batch of long-span edges, each of which fans out into dozens of
        // dummy slots. This exercises the bucketed neighbor/crossing
        // lookups against hundreds of chain windows per rank instead of
        // just a handful, without asserting on wall-clock time.
        let node_count = 120;
        let nodes: Vec<LNode> = (0..node_count).map(|_| n()).collect();
        let mut edges: Vec<LEdge> = (0..node_count - 1).map(|i| e(i, i + 1)).collect();
        for k in 0..60 {
            let from = k;
            let to = (k + 40).min(node_count - 1);
            if to > from {
                edges.push(e(from, to));
            }
        }
        let ranks: Vec<usize> = (0..node_count).collect();

        let mut a = build_order_graph(&nodes, &edges, &ranks);
        let mut b = build_order_graph(&nodes, &edges, &ranks);
        minimize_crossings(&mut a, &edges);
        minimize_crossings(&mut b, &edges);

        assert_eq!(a.ranks.len(), node_count);
        let ka: Vec<Vec<String>> = a
            .ranks
            .iter()
            .map(|r| r.iter().map(|s| format!("{:?}", s.kind)).collect())
            .collect();
        let kb: Vec<Vec<String>> = b
            .ranks
            .iter()
            .map(|r| r.iter().map(|s| format!("{:?}", s.kind)).collect())
            .collect();
        assert_eq!(ka, kb);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SlotKind {
    Real(usize),
    /// Dummy waypoint for `edge` (acyclic index) at `rank`.
    Dummy { edge: usize, rank: usize },
}

#[derive(Debug, Clone)]
pub(crate) struct Slot {
    pub kind: SlotKind,
    pub size: (f64, f64),
}

#[derive(Debug, Clone)]
pub(crate) struct OrderGraph {
    /// ranks[r] = ordered slots on rank r.
    pub ranks: Vec<Vec<Slot>>,
    /// chains[edge] = slot identities the edge passes through, in order
    /// from source to target (both endpoints included).
    pub chains: Vec<Vec<SlotKind>>,
}

/// Default footprint a dummy occupies so parallel long edges don't fuse.
const DUMMY_SIZE: (f64, f64) = (8.0, 8.0);

pub(crate) fn build_order_graph(
    nodes: &[LNode],
    edges: &[LEdge],
    ranks: &[usize],
) -> OrderGraph {
    let max_rank = ranks.iter().copied().max().unwrap_or(0);
    let mut rows: Vec<Vec<Slot>> = vec![Vec::new(); max_rank + 1];
    for (i, node) in nodes.iter().enumerate() {
        rows[ranks[i]].push(Slot {
            kind: SlotKind::Real(i),
            size: (node.width, node.height),
        });
    }
    let mut chains: Vec<Vec<SlotKind>> = Vec::with_capacity(edges.len());
    for (ei, e) in edges.iter().enumerate() {
        let (rf, rt) = (ranks[e.from], ranks[e.to]);
        let mut chain = vec![SlotKind::Real(e.from)];
        if rt > rf + 1 {
            for (step, r) in (rf + 1..rt).enumerate() {
                // First dummy of a labeled edge is sized to the label so
                // ordering/positioning reserve room for it.
                let size = if step == 0 {
                    e.label.unwrap_or(DUMMY_SIZE)
                } else {
                    DUMMY_SIZE
                };
                let kind = SlotKind::Dummy { edge: ei, rank: r };
                rows[r].push(Slot { kind, size });
                chain.push(kind);
            }
        }
        chain.push(SlotKind::Real(e.to));
        chains.push(chain);
    }
    OrderGraph { ranks: rows, chains }
}

/// Buckets every chain window `(a, b)` by the rank its upper element `a`
/// lives on. Because a chain's slots descend consecutive ranks, `a`'s rank
/// is also `b`'s rank minus one, so bucket `r` holds exactly the windows
/// bridging rank `r` and rank `r + 1`. Built in one pass over `g.chains`
/// (a second pass over `g.ranks` supplies the slot→rank lookup), so the
/// whole structure costs O(total_windows) regardless of how many ranks or
/// slots-per-rank the graph has. Windows are pushed in chain-index order
/// (chains outer, window-within-chain inner) so consumers that want a
/// deterministic per-slot neighbor order can rely on bucket order.
fn windows_by_rank(g: &OrderGraph) -> Vec<Vec<(SlotKind, SlotKind)>> {
    let mut slot_rank: std::collections::HashMap<SlotKind, usize> =
        std::collections::HashMap::new();
    for (r, row) in g.ranks.iter().enumerate() {
        for s in row {
            slot_rank.insert(s.kind, r);
        }
    }
    let mut buckets: Vec<Vec<(SlotKind, SlotKind)>> = vec![Vec::new(); g.ranks.len()];
    for chain in &g.chains {
        for w in chain.windows(2) {
            let (a, b) = (w[0], w[1]);
            if let Some(&r) = slot_rank.get(&a) {
                buckets[r].push((a, b));
            }
        }
    }
    buckets
}

/// Positions of each slot's chain-neighbors on the adjacent rank.
pub(crate) fn neighbor_positions(
    g: &OrderGraph,
    rank: usize,
    upstream: bool,
) -> Vec<Vec<usize>> {
    // For each slot on `rank`, collect current index positions of the
    // slots adjacent to it (previous rank if upstream, next if not)
    // along every edge chain that passes through it. Only the one bucket
    // that actually borders `rank` is walked (O(bucket) instead of
    // scanning every chain window in the graph); building the bucket
    // index itself is O(total_windows), so a call here costs
    // O(total_windows + slots_on_rank) rather than the previous
    // O(slots_on_rank * total_windows).
    let adj_rank = if upstream { rank.wrapping_sub(1) } else { rank + 1 };
    let mut pos_of: std::collections::HashMap<SlotKind, usize> =
        std::collections::HashMap::new();
    if adj_rank < g.ranks.len() {
        for (i, s) in g.ranks[adj_rank].iter().enumerate() {
            pos_of.insert(s.kind, i);
        }
    }
    let mut here_pos: std::collections::HashMap<SlotKind, usize> =
        std::collections::HashMap::new();
    for (i, s) in g.ranks[rank].iter().enumerate() {
        here_pos.insert(s.kind, i);
    }
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); g.ranks[rank].len()];
    let buckets = windows_by_rank(g);
    // Upstream neighbors of `rank` live in the bucket keyed by rank - 1
    // (windows whose upper element `a` is on rank - 1, lower `b` on
    // `rank`); downstream neighbors live in the bucket keyed by `rank`
    // itself (windows whose upper element is on `rank`).
    let bucket_idx = if upstream { rank.wrapping_sub(1) } else { rank };
    if bucket_idx < buckets.len() {
        for &(a, b) in &buckets[bucket_idx] {
            let (here, there) = if upstream { (b, a) } else { (a, b) };
            if let (Some(&hi), Some(&p)) = (here_pos.get(&here), pos_of.get(&there)) {
                out[hi].push(p);
            }
        }
    }
    out
}

pub(crate) fn count_crossings(g: &OrderGraph, _edges: &[LEdge]) -> usize {
    // For each adjacent rank pair, count inversions among edge segment
    // endpoint pairs. Windows are pre-bucketed by rank once via
    // `windows_by_rank` (O(total_windows)), so each rank pair only scans
    // the segments that actually bridge it instead of rescanning every
    // chain in the graph; the inversion count itself stays O(k^2) in the
    // number of segments crossing that one rank pair, which is fine at
    // our caps.
    let buckets = windows_by_rank(g);
    let mut total = 0;
    for r in 0..g.ranks.len().saturating_sub(1) {
        let pos_hi: std::collections::HashMap<SlotKind, usize> = g.ranks[r]
            .iter()
            .enumerate()
            .map(|(i, s)| (s.kind, i))
            .collect();
        let pos_lo: std::collections::HashMap<SlotKind, usize> = g.ranks[r + 1]
            .iter()
            .enumerate()
            .map(|(i, s)| (s.kind, i))
            .collect();
        let mut segs: Vec<(usize, usize)> = Vec::new();
        for &(a, b) in &buckets[r] {
            if let (Some(&ai), Some(&bi)) = (pos_hi.get(&a), pos_lo.get(&b)) {
                segs.push((ai, bi));
            }
        }
        for i in 0..segs.len() {
            for j in i + 1..segs.len() {
                let (a1, b1) = segs[i];
                let (a2, b2) = segs[j];
                if (a1 < a2 && b1 > b2) || (a1 > a2 && b1 < b2) {
                    total += 1;
                }
            }
        }
    }
    total
}

const MAX_SWEEPS: usize = 8;

pub(crate) fn minimize_crossings(g: &mut OrderGraph, edges: &[LEdge]) {
    let mut best = g.ranks.clone();
    let mut best_cross = count_crossings(g, edges);
    for sweep in 0..MAX_SWEEPS {
        let downward = sweep % 2 == 0;
        let rank_iter: Vec<usize> = if downward {
            (1..g.ranks.len()).collect()
        } else {
            (0..g.ranks.len().saturating_sub(1)).rev().collect()
        };
        for r in rank_iter {
            let nbrs = neighbor_positions(g, r, downward);
            // Barycenter with stable tiebreak on current index.
            let mut keyed: Vec<(f64, usize, Slot)> = g.ranks[r]
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, s)| {
                    let bc = if nbrs[i].is_empty() {
                        i as f64 // keep childless slots where they are
                    } else {
                        nbrs[i].iter().sum::<usize>() as f64 / nbrs[i].len() as f64
                    };
                    (bc, i, s)
                })
                .collect();
            keyed.sort_by(|a, b| {
                a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1))
            });
            g.ranks[r] = keyed.into_iter().map(|(_, _, s)| s).collect();
        }
        let c = count_crossings(g, edges);
        if c < best_cross {
            best_cross = c;
            best = g.ranks.clone();
        }
        if c == 0 {
            break;
        }
    }
    g.ranks = best;
}
