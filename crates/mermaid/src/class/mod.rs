//! Mermaid class diagrams: model types shared by the parser (this
//! slice) and the SVG renderer (Task 5).

// TODO(slice4): removed in Task 8
#![allow(dead_code)]
pub(crate) mod parse;
// pub(crate) mod svg; // Task 5

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelKind {
    Inheritance, // <|--   marker at `to` end, solid
    Realization, // <|..   marker at `to` end, dashed
    Composition, // *--    filled diamond at `to` end, solid
    Aggregation, // o--    hollow diamond at `to` end, solid
    Association, // --> or --  open arrow at `to` end (--> only), solid
    Dependency,  // ..>    open arrow at `to` end, dashed
}

#[derive(Debug, Clone)]
pub(crate) struct ClassBox {
    pub id: String,
    pub annotation: Option<String>, // <<interface>> etc.
    pub attributes: Vec<String>,    // raw member text, verbatim
    pub methods: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Relation {
    pub from: usize,
    pub to: usize,   // the MARKER end (normalized during parse)
    pub kind: RelKind,
    pub arrow: bool, // Association `--` (false) vs `-->` (true)
    pub m_from: Option<String>, // multiplicity near `from`
    pub m_to: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ClassGraph {
    pub classes: Vec<ClassBox>,
    pub relations: Vec<Relation>,
}
