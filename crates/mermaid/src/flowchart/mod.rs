pub(crate) mod parse;
pub(crate) mod shapes;
pub(crate) mod svg;
#[cfg(test)]
mod props;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShapeKind {
    Rect,
    Subroutine,
    Rounded,
    Stadium,
    Circle,
    DoubleCircle,
    Diamond,
    Hexagon,
    Parallelogram,
    ParallelogramRev,
    Trapezoid,
    TrapezoidRev,
    Cylinder,
    Flag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    Arrow,
    Open,
    Dotted,
    Thick,
    /// `~~~` — participates in layout, draws nothing.
    Invisible,
}

/// Per-end edge decoration. `from_head` renders as `marker-start`,
/// `to_head` as `marker-end` (edge paths run from→to after the layout
/// engine restores true direction on reversed edges).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Head {
    None,
    Arrow,
    Circle,
    Cross,
}

#[derive(Debug, Clone)]
pub(crate) struct FlowNode {
    /// Mermaid source identifier. Read by the post-parse
    /// edge-to-subgraph check (parse.rs) and asserted on by parser
    /// tests; downstream render stages address nodes by index.
    pub id: String,
    pub label: String,          // raw; escaped only at SVG emission
    pub shape: ShapeKind,
    pub classes: Vec<String>,
    pub subgraph: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct FlowEdge {
    pub from: usize,
    pub to: usize,
    /// Line-style family. `Arrow` is retained (rather than folding into
    /// `Open`) because the parser's pre-polish tests assert it for `-->`;
    /// solid edges map to `Arrow` iff either head is `Head::Arrow`.
    pub kind: EdgeKind,
    pub from_head: Head,
    pub to_head: Head,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct FlowSubgraph {
    pub id: String,
    pub title: String,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct ClassDef {
    pub name: String,
    /// Sanitized `prop:value` pairs joined with `;` — allowlisted
    /// properties only (see parse.rs), safe to emit as a style attr.
    pub style: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FlowGraph {
    pub direction: crate::layout::Direction,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
    pub subgraphs: Vec<FlowSubgraph>,
    pub class_defs: Vec<ClassDef>,
}

/// Full flowchart pipeline: parse -> measure -> layout -> SVG. Never
/// panics; a layout failure (diagram too large, malformed cluster tree)
/// surfaces as a `ParseError` with no source line.
pub(crate) fn render_flowchart(source: &str) -> Result<String, crate::ParseError> {
    let g = parse::parse(source)?;
    let mut nodes = Vec::with_capacity(g.nodes.len());
    for n in &g.nodes {
        let (tw, th) = crate::measure::text_size(&n.label);
        let (w, h) = shapes::size_for(n.shape, tw, th);
        nodes.push(crate::layout::LNode { width: w, height: h, cluster: n.subgraph });
    }
    let edges = g
        .edges
        .iter()
        .map(|e| crate::layout::LEdge {
            from: e.from,
            to: e.to,
            label: e.label.as_deref().map(|l| {
                let (w, h) = crate::measure::text_size(l);
                (w + 8.0, h + 4.0)
            }),
        })
        .collect();
    let clusters = g
        .subgraphs
        .iter()
        .map(|s| crate::layout::LCluster {
            parent: s.parent,
            title: crate::measure::text_size(&s.title),
        })
        .collect();
    let input = crate::layout::LayoutInput {
        nodes,
        edges,
        clusters,
        direction: g.direction,
    };
    let layout = crate::layout::run(&input)
        .map_err(|message| crate::ParseError { message, line: None })?;
    Ok(svg::emit(&g, &layout))
}
