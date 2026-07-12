//! Shared adapter: generic measured box-graphs → the slice-2 layered
//! layout engine. Used by the state/class/er families (flowchart
//! predates it and keeps its inline equivalent — no churn this slice).

use crate::layout::{self, Direction, LCluster, LEdge, LNode, Layout};
use crate::ParseError;

#[derive(Debug, Clone)]
pub(crate) struct BoxNode {
    pub width: f64,
    pub height: f64,
    pub cluster: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct BoxEdge {
    pub from: usize,
    pub to: usize,
    pub label: Option<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub(crate) struct BoxCluster {
    pub parent: Option<usize>,
    pub title: (f64, f64),
}

pub(crate) fn layout_boxgraph(
    nodes: &[BoxNode],
    edges: &[BoxEdge],
    clusters: &[BoxCluster],
    direction: Direction,
) -> Result<Layout, ParseError> {
    let input = layout::LayoutInput {
        nodes: nodes
            .iter()
            .map(|b| LNode { width: b.width, height: b.height, cluster: b.cluster })
            .collect(),
        edges: edges
            .iter()
            .map(|e| LEdge { from: e.from, to: e.to, label: e.label })
            .collect(),
        clusters: clusters
            .iter()
            .map(|c| LCluster { parent: c.parent, title: c.title, direction: None })
            .collect(),
        direction,
    };
    layout::run(&input).map_err(|message| ParseError { message, line: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(w: f64, h: f64) -> BoxNode {
        BoxNode { width: w, height: h, cluster: None }
    }

    #[test]
    fn simple_chain_lays_out() {
        let nodes = vec![n(80.0, 40.0), n(80.0, 40.0)];
        let edges = vec![BoxEdge { from: 0, to: 1, label: Some((30.0, 14.0)) }];
        let l = layout_boxgraph(&nodes, &edges, &[], Direction::TB).unwrap();
        assert_eq!(l.node_centers.len(), 2);
        assert_eq!(l.edge_paths.len(), 1);
        assert!(l.node_centers[1].1 > l.node_centers[0].1);
        assert!(l.edge_paths[0].label_at.is_some());
    }

    #[test]
    fn clusters_pass_through() {
        let nodes = vec![
            BoxNode { width: 60.0, height: 30.0, cluster: Some(0) },
            BoxNode { width: 60.0, height: 30.0, cluster: Some(0) },
            n(60.0, 30.0),
        ];
        let edges = vec![
            BoxEdge { from: 0, to: 1, label: None },
            BoxEdge { from: 1, to: 2, label: None },
        ];
        let clusters = vec![BoxCluster { parent: None, title: (40.0, 16.0) }];
        let l = layout_boxgraph(&nodes, &edges, &clusters, Direction::TB).unwrap();
        assert_eq!(l.cluster_rects.len(), 1);
    }

    #[test]
    fn over_cap_maps_to_parse_error() {
        let nodes: Vec<BoxNode> =
            (0..=layout::MAX_NODES).map(|_| n(1.0, 1.0)).collect();
        let e = layout_boxgraph(&nodes, &[], &[], Direction::TB).unwrap_err();
        assert!(e.message.contains("too large"));
        assert_eq!(e.line, None);
    }
}
