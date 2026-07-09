#![allow(dead_code)]
// TODO(slice2): remove once flowchart consumes layout (Task 14 removes this)

//! Mermaid-agnostic layered (Sugiyama-style) layout engine.
//! Input: abstract digraph with sized nodes, optional cluster tree, direction.
//! Output: node centers, edge polylines, cluster rects. Deterministic,
//! never panics, all stages operate in TB space (direction is a final
//! coordinate transform). See the slice-2 design spec.

pub(crate) mod acyclic;
pub(crate) mod order;
pub(crate) mod position;
pub(crate) mod rank;
pub(crate) mod route;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    TB,
    BT,
    LR,
    RL,
}

#[derive(Debug, Clone)]
pub(crate) struct LNode {
    pub width: f64,
    pub height: f64,
    pub cluster: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct LEdge {
    pub from: usize,
    pub to: usize,
    /// Reserved (w, h) for an edge label, if any.
    pub label: Option<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub(crate) struct LCluster {
    pub parent: Option<usize>,
    /// Title strip size the renderer needs above the content box.
    pub title: (f64, f64),
}

#[derive(Debug, Clone)]
pub(crate) struct LayoutInput {
    pub nodes: Vec<LNode>,
    pub edges: Vec<LEdge>,
    pub clusters: Vec<LCluster>,
    pub direction: Direction,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct EdgePath {
    /// Index into `LayoutInput::edges`.
    pub edge: usize,
    pub points: Vec<(f64, f64)>,
    pub label_at: Option<(f64, f64)>,
    /// True when the edge was reversed for layout; the renderer draws
    /// the arrowhead at the true head (points are already in true order).
    pub reversed: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Layout {
    pub node_centers: Vec<(f64, f64)>,
    pub edge_paths: Vec<EdgePath>,
    pub cluster_rects: Vec<Rect>,
    pub size: (f64, f64),
}

/// Hard caps: layout runs server-side on export from untrusted document
/// content; a pathological diagram must fail fast, not burn CPU.
pub(crate) const MAX_NODES: usize = 400;
pub(crate) const MAX_EDGES: usize = 1000;
/// Minimum horizontal gap between sibling nodes and vertical gap
/// between ranks, in SVG units.
pub(crate) const NODE_GAP_X: f64 = 40.0;
pub(crate) const RANK_GAP_Y: f64 = 50.0;

pub(crate) fn validate(input: &LayoutInput) -> Result<(), String> {
    if input.nodes.len() > MAX_NODES {
        return Err(format!(
            "diagram too large: {} nodes (max {MAX_NODES})",
            input.nodes.len()
        ));
    }
    if input.edges.len() > MAX_EDGES {
        return Err(format!(
            "diagram too large: {} edges (max {MAX_EDGES})",
            input.edges.len()
        ));
    }
    for e in &input.edges {
        if e.from >= input.nodes.len() || e.to >= input.nodes.len() {
            return Err("edge references unknown node".to_string());
        }
    }
    for n in &input.nodes {
        if let Some(c) = n.cluster {
            if c >= input.clusters.len() {
                return Err("node references unknown cluster".to_string());
            }
        }
    }
    // Cluster parent links must form a forest (no cycles, valid indices).
    for (i, c) in input.clusters.iter().enumerate() {
        let mut seen = 0usize;
        let mut cur = c.parent;
        while let Some(p) = cur {
            if p >= input.clusters.len() {
                return Err("cluster references unknown parent".to_string());
            }
            seen += 1;
            if seen > input.clusters.len() {
                return Err(format!("cluster {i} parent chain forms a cycle"));
            }
            cur = input.clusters[p].parent;
        }
    }
    Ok(())
}

/// All stages lay out top-to-bottom. This is the only place direction
/// exists: BT flips y, LR swaps axes, RL swaps then flips x.
pub(crate) fn apply_direction(layout: &mut Layout, dir: Direction) {
    let (w, h) = layout.size;
    let flip_y = |p: (f64, f64)| (p.0, h - p.1);
    let swap = |p: (f64, f64)| (p.1, p.0);
    match dir {
        Direction::TB => {}
        Direction::BT => {
            for p in &mut layout.node_centers {
                *p = flip_y(*p);
            }
            for ep in &mut layout.edge_paths {
                for p in &mut ep.points {
                    *p = flip_y(*p);
                }
                if let Some(l) = &mut ep.label_at {
                    *l = flip_y(*l);
                }
            }
            for r in &mut layout.cluster_rects {
                r.y = h - r.y - r.h;
            }
        }
        Direction::LR | Direction::RL => {
            for p in &mut layout.node_centers {
                *p = swap(*p);
            }
            for ep in &mut layout.edge_paths {
                for p in &mut ep.points {
                    *p = swap(*p);
                }
                if let Some(l) = &mut ep.label_at {
                    *l = swap(*l);
                }
            }
            for r in &mut layout.cluster_rects {
                *r = Rect { x: r.y, y: r.x, w: r.h, h: r.w };
            }
            layout.size = (h, w);
            if dir == Direction::RL {
                let new_w = layout.size.0;
                for p in &mut layout.node_centers {
                    p.0 = new_w - p.0;
                }
                for ep in &mut layout.edge_paths {
                    for p in &mut ep.points {
                        p.0 = new_w - p.0;
                    }
                    if let Some(l) = &mut ep.label_at {
                        l.0 = new_w - l.0;
                    }
                }
                for r in &mut layout.cluster_rects {
                    r.x = new_w - r.x - r.w;
                }
            }
        }
    }
}

/// Full pipeline for FLAT graphs. Cluster inputs are dispatched to the
/// collapse-expand driver (Task 7); until then they error.
pub(crate) fn run(input: &LayoutInput) -> Result<Layout, String> {
    validate(input)?;
    if !input.clusters.is_empty() {
        return Err("clusters not yet supported".to_string()); // Task 7 replaces
    }
    run_flat(input)
}

pub(crate) fn run_flat(input: &LayoutInput) -> Result<Layout, String> {
    let ac = acyclic::make_acyclic(input.nodes.len(), &input.edges);
    let ranks = rank::assign_ranks(input.nodes.len(), &ac.edges);
    // Order-graph nodes are the ORIGINAL nodes; edges are the surviving
    // acyclic edges (labels travel with them).
    let surviving: Vec<LEdge> = ac
        .edges
        .iter()
        .zip(&ac.orig)
        .map(|(e, &o)| LEdge { from: e.from, to: e.to, label: input.edges[o].label })
        .collect();
    let mut g = order::build_order_graph(&input.nodes, &surviving, &ranks);
    order::minimize_crossings(&mut g, &surviving);
    let coords = position::assign_coords(&g);
    let mut node_centers = vec![(0.0, 0.0); input.nodes.len()];
    for (kind, c) in &coords.centers {
        if let order::SlotKind::Real(i) = kind {
            node_centers[*i] = *c;
        }
    }
    let edge_paths = route::route_edges(input, &ac, &g, &coords);
    let mut layout = Layout {
        node_centers,
        edge_paths,
        cluster_rects: vec![],
        size: coords.size,
    };
    apply_direction(&mut layout, input.direction);
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(w: f64, h: f64) -> LNode {
        LNode { width: w, height: h, cluster: None }
    }

    #[test]
    fn validate_accepts_small_graph() {
        let input = LayoutInput {
            nodes: vec![node(10.0, 10.0), node(10.0, 10.0)],
            edges: vec![LEdge { from: 0, to: 1, label: None }],
            clusters: vec![],
            direction: Direction::TB,
        };
        assert!(validate(&input).is_ok());
    }

    #[test]
    fn validate_rejects_over_cap_nodes() {
        let input = LayoutInput {
            nodes: (0..=MAX_NODES).map(|_| node(1.0, 1.0)).collect(),
            edges: vec![],
            clusters: vec![],
            direction: Direction::TB,
        };
        let err = validate(&input).unwrap_err();
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn validate_rejects_out_of_bounds_edge() {
        let input = LayoutInput {
            nodes: vec![node(1.0, 1.0)],
            edges: vec![LEdge { from: 0, to: 7, label: None }],
            clusters: vec![],
            direction: Direction::TB,
        };
        assert!(validate(&input).is_err());
    }

    #[test]
    fn validate_rejects_cluster_parent_cycle() {
        let input = LayoutInput {
            nodes: vec![],
            edges: vec![],
            clusters: vec![
                LCluster { parent: Some(1), title: (0.0, 0.0) },
                LCluster { parent: Some(0), title: (0.0, 0.0) },
            ],
            direction: Direction::TB,
        };
        assert!(validate(&input).is_err());
    }

    fn one_point_layout() -> Layout {
        Layout {
            node_centers: vec![(10.0, 20.0)],
            edge_paths: vec![],
            cluster_rects: vec![Rect { x: 5.0, y: 10.0, w: 10.0, h: 20.0 }],
            size: (100.0, 200.0),
        }
    }

    #[test]
    fn direction_tb_is_identity() {
        let mut l = one_point_layout();
        apply_direction(&mut l, Direction::TB);
        assert_eq!(l.node_centers[0], (10.0, 20.0));
        assert_eq!(l.size, (100.0, 200.0));
    }

    #[test]
    fn direction_bt_flips_y() {
        let mut l = one_point_layout();
        apply_direction(&mut l, Direction::BT);
        assert_eq!(l.node_centers[0], (10.0, 180.0)); // 200 - 20
        assert_eq!(l.size, (100.0, 200.0));
        // rect y flips around its own extent: y' = H - y - h
        assert_eq!(l.cluster_rects[0].y, 200.0 - 10.0 - 20.0);
    }

    #[test]
    fn direction_lr_swaps_axes() {
        let mut l = one_point_layout();
        apply_direction(&mut l, Direction::LR);
        assert_eq!(l.node_centers[0], (20.0, 10.0));
        assert_eq!(l.size, (200.0, 100.0));
        let r = &l.cluster_rects[0];
        assert_eq!((r.x, r.y, r.w, r.h), (10.0, 5.0, 20.0, 10.0));
    }

    #[test]
    fn direction_rl_swaps_then_flips_x() {
        let mut l = one_point_layout();
        apply_direction(&mut l, Direction::RL);
        // after swap: (20,10) in 200x100; flip x: 200-20 = 180
        assert_eq!(l.node_centers[0], (180.0, 10.0));
        assert_eq!(l.size, (200.0, 100.0));
    }

    fn simple_input(dir: Direction) -> LayoutInput {
        LayoutInput {
            nodes: vec![node(60.0, 24.0), node(60.0, 24.0), node(60.0, 24.0)],
            edges: vec![
                LEdge { from: 0, to: 1, label: None },
                LEdge { from: 1, to: 2, label: Some((30.0, 12.0)) },
            ],
            clusters: vec![],
            direction: dir,
        }
    }

    #[test]
    fn run_lays_out_a_chain() {
        let l = run(&simple_input(Direction::TB)).unwrap();
        assert_eq!(l.node_centers.len(), 3);
        assert_eq!(l.edge_paths.len(), 2);
        // Chain stacks downward in TB.
        assert!(l.node_centers[1].1 > l.node_centers[0].1);
        assert!(l.node_centers[2].1 > l.node_centers[1].1);
        // Edge endpoints clip at node boxes, not centers.
        let p0 = &l.edge_paths[0].points;
        assert!(p0.first().unwrap().1 > l.node_centers[0].1);
        assert!(p0.last().unwrap().1 < l.node_centers[1].1);
        // Labeled single-span edge gets a midpoint label anchor.
        assert!(l.edge_paths[1].label_at.is_some());
    }

    #[test]
    fn run_lr_flows_rightward() {
        let l = run(&simple_input(Direction::LR)).unwrap();
        assert!(l.node_centers[1].0 > l.node_centers[0].0);
        assert!(l.node_centers[2].0 > l.node_centers[1].0);
    }

    #[test]
    fn run_handles_cycle_with_true_direction_points() {
        let input = LayoutInput {
            nodes: vec![node(40.0, 20.0), node(40.0, 20.0)],
            edges: vec![
                LEdge { from: 0, to: 1, label: None },
                LEdge { from: 1, to: 0, label: None },
            ],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        let back = l.edge_paths.iter().find(|p| p.reversed).expect("one reversed");
        // True direction: starts near node 1, ends near node 0.
        let start = back.points.first().unwrap();
        let end = back.points.last().unwrap();
        let d_start_1 = (start.1 - l.node_centers[1].1).abs();
        let d_end_0 = (end.1 - l.node_centers[0].1).abs();
        assert!(d_start_1 < d_end_0.max(60.0));
        assert!(end.1 < start.1, "back edge should flow upward in TB");
    }

    #[test]
    fn run_self_loop_has_points() {
        let input = LayoutInput {
            nodes: vec![node(40.0, 20.0)],
            edges: vec![LEdge { from: 0, to: 0, label: None }],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        assert!(l.edge_paths[0].points.len() >= 4);
        for p in &l.edge_paths[0].points {
            assert!(p.0.is_finite() && p.1.is_finite());
        }
    }

    #[test]
    fn run_rejects_over_cap() {
        let input = LayoutInput {
            nodes: (0..=MAX_NODES)
                .map(|_| node(1.0, 1.0))
                .collect(),
            edges: vec![],
            clusters: vec![],
            direction: Direction::TB,
        };
        assert!(run(&input).unwrap_err().contains("too large"));
    }

    #[test]
    fn run_empty_graph_ok() {
        let input = LayoutInput {
            nodes: vec![],
            edges: vec![],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        assert!(l.node_centers.is_empty());
        assert!(l.size.0 > 0.0 && l.size.1 > 0.0);
    }
}
