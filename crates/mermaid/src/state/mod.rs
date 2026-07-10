//! Mermaid state diagrams: model types shared by the parser (this
//! slice) and the SVG renderer (Task 3).

// TODO(slice4): removed in Task 8
#![allow(dead_code)]
pub(crate) mod parse;
pub(crate) mod svg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateKind { Normal, Start, End, Choice, ForkJoin }

#[derive(Debug, Clone)]
pub(crate) struct StateNode {
    pub id: String,          // synthetic ids for [*]: "__start_N"/"__end_N"
    pub display: String,
    pub kind: StateKind,
    pub composite: Option<usize>, // index into composites
}

#[derive(Debug, Clone)]
pub(crate) struct Transition {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
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
                ((tw + 24.0).max(60.0), (th + 16.0).max(36.0))
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
            label: t.label.as_deref().map(|l| {
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

    let layout = crate::boxgraph::layout_boxgraph(
        &nodes,
        &edges,
        &clusters,
        crate::layout::Direction::TB,
    )?;
    Ok(svg::emit(&g, &layout, &sizes))
}
