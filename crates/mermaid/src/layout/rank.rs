//! Stage 2: longest-path ranking over the acyclic graph, then pull
//! sources down toward their children so single-edge chains from a
//! late-declared source don't stretch the whole diagram.

use super::LEdge;

#[cfg(test)]
mod tests {
    use super::*;

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    #[test]
    fn chain_ranks_increase() {
        let ranks = assign_ranks(3, &[e(0, 1), e(1, 2)]);
        assert_eq!(ranks, vec![0, 1, 2]);
    }

    #[test]
    fn diamond_takes_longest_path() {
        // 0 -> 1 -> 3, 0 -> 2 -> 3 plus 0 -> 3 direct: 3 must sit at rank 2.
        let ranks = assign_ranks(4, &[e(0, 1), e(0, 2), e(1, 3), e(2, 3), e(0, 3)]);
        assert_eq!(ranks[0], 0);
        assert_eq!(ranks[3], 2);
        assert!(ranks[1] == 1 && ranks[2] == 1);
    }

    #[test]
    fn every_edge_is_monotone() {
        let edges = vec![e(0, 2), e(1, 2), e(2, 3), e(1, 3)];
        let ranks = assign_ranks(4, &edges);
        for ed in &edges {
            assert!(ranks[ed.to] > ranks[ed.from]);
        }
    }

    #[test]
    fn source_tightened_to_children() {
        // 0->1->2->3 is the long chain; 4->3 is a lone source into the sink.
        // Without tightening 4 sits at rank 0; tightened it sits at rank 2.
        let ranks = assign_ranks(5, &[e(0, 1), e(1, 2), e(2, 3), e(4, 3)]);
        assert_eq!(ranks[4], ranks[3] - 1);
    }

    #[test]
    fn isolated_node_rank_zero() {
        let ranks = assign_ranks(2, &[]);
        assert_eq!(ranks, vec![0, 0]);
    }
}

/// Longest-path ranks via Kahn topological order, then one tightening
/// pass: any node with NO incoming edges moves down to
/// `min(rank of successors) - 1`.
pub(crate) fn assign_ranks(node_count: usize, edges: &[LEdge]) -> Vec<usize> {
    let mut indeg = vec![0usize; node_count];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for e in edges {
        indeg[e.to] += 1;
        adj[e.from].push(e.to);
    }
    let mut ranks = vec![0usize; node_count];
    // Deterministic Kahn: process in ascending index order per wave.
    let mut queue: Vec<usize> = (0..node_count).filter(|&v| indeg[v] == 0).collect();
    let mut head = 0;
    while head < queue.len() {
        let v = queue[head];
        head += 1;
        for &to in &adj[v] {
            ranks[to] = ranks[to].max(ranks[v] + 1);
            indeg[to] -= 1;
            if indeg[to] == 0 {
                queue.push(to);
            }
        }
    }
    // Tighten sources (recompute indegree; the loop above consumed it).
    let mut indeg2 = vec![0usize; node_count];
    for e in edges {
        indeg2[e.to] += 1;
    }
    for v in 0..node_count {
        if indeg2[v] == 0 && !adj[v].is_empty() {
            let min_child = adj[v].iter().map(|&t| ranks[t]).min().unwrap_or(1);
            if min_child > 0 {
                ranks[v] = min_child - 1;
            }
        }
    }
    ranks
}
