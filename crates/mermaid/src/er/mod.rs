//! Mermaid ER (entity-relationship) diagrams: model types shared by the
//! parser and the SVG renderer (Task 7).

// TODO(slice4): removed in Task 8
#![allow(dead_code)]
pub(crate) mod parse;
// pub(crate) mod svg; // Task 7

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cardinality {
    ExactlyOne, // ||
    ZeroOrOne,  // |o / o|
    OneOrMore,  // }| / |{
    ZeroOrMore, // }o / o{
}

#[derive(Debug, Clone)]
pub(crate) struct ErAttribute {
    pub ty: String,
    pub name: String,
    pub key: Option<String>, // "PK" | "FK"
}

#[derive(Debug, Clone)]
pub(crate) struct Entity {
    pub id: String,
    pub attributes: Vec<ErAttribute>,
}

#[derive(Debug, Clone)]
pub(crate) struct ErRelation {
    pub from: usize,
    pub to: usize,
    pub card_from: Cardinality,
    pub card_to: Cardinality,
    pub identifying: bool, // -- solid vs .. dashed
    pub label: String,     // required by grammar
}

#[derive(Debug, Clone)]
pub(crate) struct ErGraph {
    pub entities: Vec<Entity>,
    pub relations: Vec<ErRelation>,
}
