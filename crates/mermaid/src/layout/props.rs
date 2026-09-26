//! Property tests: the full layout pipeline holds its invariants for
//! arbitrary graphs. Uses proptest (dev-dependency only).

use super::*;
use proptest::prelude::*;

fn arb_input() -> impl Strategy<Value = LayoutInput> {
    // Up to 12 nodes, 20 edges, 3 clusters — small enough to run fast,
    // shaped enough to hit cycles, self-loops, fan-in/out, clusters.
    (1usize..12).prop_flat_map(|n| {
        let nodes = proptest::collection::vec(
            ((10.0f64..120.0), (10.0f64..40.0), proptest::option::of(0usize..3)),
            n,
        );
        let edges = proptest::collection::vec((0usize..n, 0usize..n), 0..20);
        (nodes, edges, 0usize..4).prop_map(|(nodes, edges, dir)| {
            let cluster_count = nodes
                .iter()
                .filter_map(|(_, _, c)| *c)
                .max()
                .map(|m| m + 1)
                .unwrap_or(0);
            LayoutInput {
                nodes: nodes
                    .into_iter()
                    .map(|(w, h, c)| LNode { width: w, height: h, cluster: c })
                    .collect(),
                edges: edges
                    .into_iter()
                    .map(|(f, t)| LEdge { from: f, to: t, label: None })
                    .collect(),
                clusters: (0..cluster_count)
                    .map(|_| LCluster { parent: None, title: (30.0, 14.0), direction: None })
                    .collect(),
                direction: match dir {
                    0 => Direction::TB,
                    1 => Direction::BT,
                    2 => Direction::LR,
                    _ => Direction::RL,
                },
            }
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn pipeline_never_panics_and_no_nan(input in arb_input()) {
        if let Ok(l) = run(&input) {
            for (x, y) in &l.node_centers {
                prop_assert!(x.is_finite() && y.is_finite());
            }
            for ep in &l.edge_paths {
                for (x, y) in &ep.points {
                    prop_assert!(x.is_finite() && y.is_finite());
                }
            }
            prop_assert!(l.size.0.is_finite() && l.size.1.is_finite());
            prop_assert_eq!(l.edge_paths.len(), input.edges.len());
        }
    }

    #[test]
    fn pipeline_deterministic(input in arb_input()) {
        let a = run(&input);
        let b = run(&input);
        match (a, b) {
            (Ok(la), Ok(lb)) => {
                prop_assert_eq!(la.node_centers, lb.node_centers);
                prop_assert_eq!(la.size, lb.size);
            }
            (Err(ea), Err(eb)) => prop_assert_eq!(ea, eb),
            _ => prop_assert!(false, "one Ok one Err"),
        }
    }

    #[test]
    fn flat_no_node_overlaps(input in arb_input()) {
        // Overlap invariant asserted on flat graphs only (cluster
        // expansion translates sub-layouts; cross-cluster spacing is a
        // looser guarantee).
        let mut input = input;
        for n in &mut input.nodes { n.cluster = None; }
        input.clusters.clear();
        if let Ok(l) = run(&input) {
            for i in 0..input.nodes.len() {
                for j in i + 1..input.nodes.len() {
                    let (ci, cj) = (l.node_centers[i], l.node_centers[j]);
                    let (ni, nj) = (&input.nodes[i], &input.nodes[j]);
                    let x_sep = (ci.0 - cj.0).abs()
                        >= (ni.width + nj.width) / 2.0 - 1e-6;
                    let y_sep = (ci.1 - cj.1).abs()
                        >= (ni.height + nj.height) / 2.0 - 1e-6;
                    prop_assert!(
                        x_sep || y_sep,
                        "nodes {i} and {j} overlap: {ci:?} {cj:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn clustered_no_node_overlaps(input in arb_input()) {
        // Like `flat_no_node_overlaps`, but KEEPING clusters: containment
        // is a looser guarantee (cross-cluster spacing includes title/pad
        // strips), so only real-node pairwise non-overlap is asserted
        // here, not cluster-rect containment or cross-cluster clearance.
        if let Ok(l) = run(&input) {
            for i in 0..input.nodes.len() {
                for j in i + 1..input.nodes.len() {
                    let (ci, cj) = (l.node_centers[i], l.node_centers[j]);
                    let (ni, nj) = (&input.nodes[i], &input.nodes[j]);
                    let x_sep = (ci.0 - cj.0).abs()
                        >= (ni.width + nj.width) / 2.0 - 1e-6;
                    let y_sep = (ci.1 - cj.1).abs()
                        >= (ni.height + nj.height) / 2.0 - 1e-6;
                    prop_assert!(
                        x_sep || y_sep,
                        "nodes {i} and {j} overlap: {ci:?} {cj:?}"
                    );
                }
            }
        }
    }
}

/// `arb_input` builds every cluster with `parent: None`; production
/// (flowchart subgraphs, C4 boundaries) nests them, and `cluster.rs`
/// carries `expect("child clusters are built before their parent")`
/// guarded only by build ordering. This strategy assigns each cluster
/// `i > 0` a random parent `j < i` (always acyclic) and a random
/// direction override.
fn arb_nested_input() -> impl Strategy<Value = LayoutInput> {
    arb_input().prop_flat_map(|input| {
        let n = input.clusters.len();
        let parents = proptest::collection::vec(any::<u16>(), n);
        let dirs = proptest::collection::vec(0u8..5, n);
        (Just(input), parents, dirs).prop_map(|(mut input, parents, dirs)| {
            for i in 1..input.clusters.len() {
                input.clusters[i].parent = Some(parents[i] as usize % i);
            }
            for (c, d) in input.clusters.iter_mut().zip(dirs) {
                c.direction = match d {
                    0 => Some(Direction::TB),
                    1 => Some(Direction::BT),
                    2 => Some(Direction::LR),
                    3 => Some(Direction::RL),
                    _ => None,
                };
            }
            input
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Nested clusters at any depth: never panic, and every coordinate
    /// the pipeline emits is finite with one path per edge.
    #[test]
    fn nested_clusters_never_panic_and_stay_finite(input in arb_nested_input()) {
        if let Ok(l) = run(&input) {
            for (x, y) in &l.node_centers {
                prop_assert!(x.is_finite() && y.is_finite());
            }
            for ep in &l.edge_paths {
                for (x, y) in &ep.points {
                    prop_assert!(x.is_finite() && y.is_finite());
                }
            }
            prop_assert!(l.size.0.is_finite() && l.size.1.is_finite());
            prop_assert_eq!(l.edge_paths.len(), input.edges.len());
        }
    }
}
