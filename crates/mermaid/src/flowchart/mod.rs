#![allow(dead_code)]
// TODO(slice2): removed in Task 14

pub(crate) mod measure;

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
