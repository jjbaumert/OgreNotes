//! Stage 1: break cycles by reversing DFS back-edges; extract self-loops
//! (they carry no ranking information and are routed specially).

use super::LEdge;

pub(crate) struct AcyclicResult {
    /// Surviving edges (self-loops removed), back-edges flipped.
    pub edges: Vec<LEdge>,
    /// Parallel to `edges`: original index of each surviving edge.
    pub orig: Vec<usize>,
    /// Indexed by ORIGINAL edge index: was this edge flipped for layout?
    pub reversed: Vec<bool>,
    /// Original indices of self-loop edges (removed from `edges`).
    pub self_loops: Vec<usize>,
}

/// Iterative DFS from every unvisited node in index order (determinism).
/// An edge into a node currently on the DFS stack (gray) is a back-edge:
/// flip it. Self-loops are extracted first.
pub(crate) fn make_acyclic(node_count: usize, edges: &[LEdge]) -> AcyclicResult {
    let mut reversed = vec![false; edges.len()];
    let mut self_loops = Vec::new();

    // Adjacency of non-self-loop edges, by original index.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for (i, e) in edges.iter().enumerate() {
        if e.from == e.to {
            self_loops.push(i);
        } else {
            adj[e.from].push(i);
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color = vec![Color::White; node_count];

    for start in 0..node_count {
        if color[start] != Color::White {
            continue;
        }
        // Stack of (node, next adjacency slot to try).
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        color[start] = Color::Gray;
        while let Some(&mut (v, ref mut slot)) = stack.last_mut() {
            if *slot < adj[v].len() {
                let ei = adj[v][*slot];
                *slot += 1;
                let to = edges[ei].to;
                match color[to] {
                    Color::White => {
                        color[to] = Color::Gray;
                        stack.push((to, 0));
                    }
                    Color::Gray => reversed[ei] = true, // back-edge
                    Color::Black => {}
                }
            } else {
                color[v] = Color::Black;
                stack.pop();
            }
        }
    }

    let mut out_edges = Vec::new();
    let mut orig = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        if e.from == e.to {
            continue;
        }
        let mut e2 = e.clone();
        if reversed[i] {
            std::mem::swap(&mut e2.from, &mut e2.to);
        }
        out_edges.push(e2);
        orig.push(i);
    }
    AcyclicResult { edges: out_edges, orig, reversed, self_loops }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    /// Kahn's-algorithm check: the surviving edge set admits a topological order.
    fn is_acyclic(n: usize, edges: &[LEdge]) -> bool {
        let mut indeg = vec![0usize; n];
        for ed in edges {
            indeg[ed.to] += 1;
        }
        let mut queue: Vec<usize> =
            (0..n).filter(|&v| indeg[v] == 0).collect();
        let mut seen = 0;
        while let Some(v) = queue.pop() {
            seen += 1;
            for ed in edges.iter().filter(|ed| ed.from == v) {
                indeg[ed.to] -= 1;
                if indeg[ed.to] == 0 {
                    queue.push(ed.to);
                }
            }
        }
        seen == n
    }

    #[test]
    fn dag_passes_through_unchanged() {
        let edges = vec![e(0, 1), e(1, 2), e(0, 2)];
        let r = make_acyclic(3, &edges);
        assert!(r.reversed.iter().all(|&b| !b));
        assert!(r.self_loops.is_empty());
        assert_eq!(r.edges.len(), 3);
        assert!(is_acyclic(3, &r.edges));
    }

    #[test]
    fn two_cycle_reverses_one_edge() {
        let edges = vec![e(0, 1), e(1, 0)];
        let r = make_acyclic(2, &edges);
        assert_eq!(r.reversed.iter().filter(|&&b| b).count(), 1);
        assert!(is_acyclic(2, &r.edges));
    }

    #[test]
    fn three_cycle_becomes_acyclic() {
        let edges = vec![e(0, 1), e(1, 2), e(2, 0)];
        let r = make_acyclic(3, &edges);
        assert!(is_acyclic(3, &r.edges));
        // Exactly one reversal suffices for a simple 3-cycle.
        assert_eq!(r.reversed.iter().filter(|&&b| b).count(), 1);
    }

    #[test]
    fn self_loop_extracted_not_reversed() {
        let edges = vec![e(0, 0), e(0, 1)];
        let r = make_acyclic(2, &edges);
        assert_eq!(r.self_loops, vec![0]);
        assert_eq!(r.edges.len(), 1);
        assert_eq!(r.orig, vec![1]);
        assert!(!r.reversed[0]);
    }

    #[test]
    fn disconnected_components_handled() {
        let edges = vec![e(0, 1), e(2, 3), e(3, 2)];
        let r = make_acyclic(4, &edges);
        assert!(is_acyclic(4, &r.edges));
    }

    #[test]
    fn deterministic() {
        let edges = vec![e(0, 1), e(1, 2), e(2, 0), e(1, 0)];
        let a = make_acyclic(3, &edges);
        let b = make_acyclic(3, &edges);
        assert_eq!(a.reversed, b.reversed);
        assert_eq!(a.self_loops, b.self_loops);
    }
}
