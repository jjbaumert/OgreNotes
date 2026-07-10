//! Mermaid state diagrams: model types shared by the parser (this
//! slice) and the SVG renderer (Task 3).

// TODO(slice4): removed in Task 8
#![allow(dead_code)]
pub(crate) mod parse;
// pub(crate) mod svg; // Task 3

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
