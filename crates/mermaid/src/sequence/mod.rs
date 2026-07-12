//! Mermaid sequence diagrams: parser → two-pass lifeline layout → SVG.
//! Bespoke layout (participant columns + event rows) — the layered
//! graph engine in `crate::layout` is deliberately not involved.

pub(crate) mod parse;
pub(crate) mod layout;
pub(crate) mod svg;
#[cfg(test)]
mod props;

/// Render mermaid sequence-diagram `source` to a self-contained `<svg>`
/// string: parse → lifeline layout → SVG assembly.
pub(crate) fn render_sequence(source: &str) -> Result<String, crate::ParseError> {
    let d = parse::parse(source)?;
    let l = layout::run(&d);
    Ok(svg::emit(&d, &l))
}

/// Caps enforced during parse — sequence rendering runs server-side on
/// untrusted document content; work is bounded before layout begins.
pub(crate) const MAX_PARTICIPANTS: usize = 50;
pub(crate) const MAX_EVENTS: usize = 1000;
pub(crate) const MAX_FRAGMENT_DEPTH: usize = 16;

#[derive(Debug, Clone)]
pub(crate) struct Participant {
    pub id: String,
    pub display: String,   // raw; escaped only at SVG emission
    pub is_actor: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineStyle { Solid, Dotted }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Head { None, Arrow, Cross, Async }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FragmentKind { Loop, Alt, Opt, Par, Critical, Break }

impl FragmentKind {
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            FragmentKind::Loop => "loop",
            FragmentKind::Alt => "alt",
            FragmentKind::Opt => "opt",
            FragmentKind::Par => "par",
            FragmentKind::Critical => "critical",
            FragmentKind::Break => "break",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NotePlacement {
    LeftOf(usize),
    RightOf(usize),
    Over(usize, Option<usize>),
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Message {
        from: usize,
        to: usize,
        line: LineStyle,
        /// Head at the `to` end (`marker-end`).
        head: Head,
        /// Head at the `from` end (`marker-start`) — `Head::None` except
        /// for bidirectional arrows (`<<->>` / `<<-->>`).
        from_head: Head,
        text: String,
        /// `->>+B`: activate the TARGET on arrival.
        activate_target: bool,
        /// `-->>-B` (minus before target): deactivate the SOURCE.
        deactivate_source: bool,
    },
    Note { placement: NotePlacement, text: String },
    FragmentOpen { kind: FragmentKind, label: String },
    /// `else` (in alt) or `and` (in par).
    FragmentDivider { label: String },
    FragmentClose,
    Activate { p: usize },
    Deactivate { p: usize },
    Autonumber,
}

#[derive(Debug, Clone)]
pub(crate) struct SeqDiagram {
    pub participants: Vec<Participant>,
    pub events: Vec<Event>,
}
