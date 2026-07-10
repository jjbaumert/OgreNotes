pub(crate) mod measure;
pub(crate) mod parse;
pub(crate) mod shapes;
pub(crate) mod svg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShapeKind {
    Rect,
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
}

#[derive(Debug, Clone)]
pub(crate) struct FlowNode {
    /// Mermaid source identifier. Not read by the render pipeline
    /// (downstream stages address nodes by index); kept because parser
    /// tests assert on it to verify id-extraction is correct (bare ids,
    /// subgraph-membership lookup, class-assignment lookup). See
    /// task-14-report.md for why this isn't deleted outright.
    #[allow(dead_code)]
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
    pub kind: EdgeKind,
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
        let (tw, th) = measure::text_size(&n.label);
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
                let (w, h) = measure::text_size(l);
                (w + 8.0, h + 4.0)
            }),
        })
        .collect();
    let clusters = g
        .subgraphs
        .iter()
        .map(|s| crate::layout::LCluster {
            parent: s.parent,
            title: measure::text_size(&s.title),
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
