//! Mermaid state diagrams: model types shared by the parser (this
//! slice) and the SVG renderer (Task 3).

pub(crate) mod parse;
#[cfg(test)]
mod props;
pub(crate) mod svg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateKind {
    Normal,
    Start,
    End,
    Choice,
    ForkJoin,
    CompositePort,
}

#[derive(Debug, Clone)]
pub(crate) struct StateNode {
    /// Mermaid source identifier (synthetic ids for `[*]`:
    /// "__start_N"/"__end_N"). Not read by the render pipeline
    /// (downstream stages address nodes by index); kept because parser
    /// tests assert on it to verify id-extraction and synthetic-node
    /// generation are correct. Same precedent as
    /// `flowchart::FlowNode.id` / `sequence::SeqLayout.body_top` — see
    /// task-14-report.md.
    #[allow(dead_code)]
    pub id: String,
    pub display: String,
    pub kind: StateKind,
    pub composite: Option<usize>, // index into composites
    pub classes: Vec<String>,
    pub style: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Transition {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub style: Option<String>, // sanitized `linkStyle` override
    pub from_cluster: Option<usize>,
    pub to_cluster: Option<usize>,
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct StateNote {
    pub state: usize,
    pub right: bool, // false = left of
    pub text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Composite {
    pub id: String,
    pub display: String,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct StateGraph {
    pub nodes: Vec<StateNode>,
    pub transitions: Vec<Transition>,
    pub notes: Vec<StateNote>,
    pub composites: Vec<Composite>,
    pub class_defs: Vec<crate::style::ClassDef>,
}

/// Full state-diagram pipeline: parse -> size each node by kind -> lay
/// out via the shared `boxgraph` adapter -> SVG. Never panics; a layout
/// failure (diagram too large) surfaces as a `ParseError` with no source
/// line, same as `boxgraph::layout_boxgraph`'s other consumers.
pub(crate) fn render_state(source: &str) -> Result<String, crate::ParseError> {
    let g = parse::parse(source)?;

    let mut sizes = Vec::with_capacity(g.nodes.len());
    let mut nodes = Vec::with_capacity(g.nodes.len());
    for n in &g.nodes {
        let size = match n.kind {
            StateKind::Normal => {
                let (tw, th) = crate::measure::text_size(&n.display);
                // Mermaid's state.padding is 8, applied uniformly (was tw+24
                // with a 60px min-width — a horizontal-only inflation).
                ((tw + 16.0).max(36.0), (th + 16.0).max(36.0))
            }
            StateKind::Start | StateKind::End => (18.0, 18.0),
            // Reuse the flowchart diamond footprint formula so choice
            // nodes inscribe their label identically to a flowchart
            // decision diamond.
            StateKind::Choice => {
                let (tw, th) = crate::measure::text_size(&n.display);
                crate::flowchart::shapes::size_for(crate::flowchart::ShapeKind::Diamond, tw, th)
            }
            StateKind::ForkJoin => (44.0, 10.0),
            StateKind::CompositePort => (0.0, 0.0),
        };
        sizes.push(size);
        nodes.push(crate::boxgraph::BoxNode {
            width: size.0,
            height: size.1,
            cluster: n.composite,
        });
    }

    let edges: Vec<crate::boxgraph::BoxEdge> = g
        .transitions
        .iter()
        .map(|t| crate::boxgraph::BoxEdge {
            from: t.from,
            to: t.to,
            // Composite self-loop labels occupy the reserved outer envelope,
            // rather than redundant label boxes inside the composite.
            label: t
                .label
                .as_deref()
                .filter(|_| t.from_cluster.is_none() || t.from_cluster != t.to_cluster)
                .map(|l| {
                    let (w, h) = crate::measure::text_size(l);
                    (w + 8.0, h + 4.0)
                }),
        })
        .collect();

    let clusters: Vec<crate::boxgraph::BoxCluster> = g
        .composites
        .iter()
        .map(|c| crate::boxgraph::BoxCluster {
            parent: c.parent,
            title: crate::measure::text_size(&c.display),
        })
        .collect();

    let margins = svg::composite_loop_margins(&g);
    let layout = crate::boxgraph::layout_boxgraph_with_cluster_margins(
        &nodes,
        &edges,
        &clusters,
        &margins,
        crate::layout::Direction::TB,
    )?;
    Ok(svg::emit(&g, &layout, &sizes))
}
