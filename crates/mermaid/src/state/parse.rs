//! State-diagram parser. Line-oriented, mirrors `flowchart::parse` and
//! `sequence::parse`: `err()` helper, ASCII id scan with byte-safety
//! comment, `split_once(':')` for labels, exact-match guards for
//! keyword/brace statements, a composite stack shaped like flowchart's
//! subgraph stack (membership-on-creation, `(index, opening_line)`,
//! EOF unclosed check).

use crate::state::{Composite, StateGraph, StateKind, StateNode, StateNote, Transition};
use crate::ParseError;
use std::collections::HashMap;

/// Which side of a transition an endpoint was parsed from — decides
/// whether a bare `[*]` materializes a `Start` or an `End` node.
#[derive(Clone, Copy)]
enum EndpointRole {
    Source,
    Target,
}

struct Parser {
    g: StateGraph,
    ids: HashMap<String, usize>,
    /// Composite ids are a separate namespace from node ids (composites
    /// are clusters, not nodes) but conceptually share the same id
    /// space: a transition endpoint naming a composite id is an error,
    /// checked against this set.
    composite_ids: HashMap<String, usize>,
    /// Open composite states: (composite index, opening line). Top of
    /// stack is the innermost currently-open composite.
    stack: Vec<(usize, usize)>,
    line: usize, // 1-based, for errors
    start_count: usize,
    end_count: usize,
}

pub(crate) fn parse(source: &str) -> Result<StateGraph, ParseError> {
    let mut p = Parser {
        g: StateGraph { nodes: vec![], transitions: vec![], notes: vec![], composites: vec![] },
        ids: HashMap::new(),
        composite_ids: HashMap::new(),
        stack: Vec::new(),
        line: 0,
        start_count: 0,
        end_count: 0,
    };
    let mut seen_header = false;
    for (idx, raw) in source.lines().enumerate() {
        p.line = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            let header = line.strip_suffix(';').unwrap_or(line).trim_end();
            if header != "stateDiagram-v2" && header != "stateDiagram" {
                return Err(p.err(
                    "state diagram must start with `stateDiagram` or `stateDiagram-v2`",
                ));
            }
            seen_header = true;
            continue;
        }
        for stmt in line.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            p.parse_statement(stmt)?;
        }
    }
    if !seen_header {
        return Err(ParseError {
            message: "state diagram must start with `stateDiagram` or `stateDiagram-v2`".into(),
            line: Some(1),
        });
    }
    if let Some(&(idx, opening_line)) = p.stack.last() {
        return Err(ParseError {
            message: format!("unclosed composite state `{}`", p.g.composites[idx].id),
            line: Some(opening_line),
        });
    }
    Ok(p.g)
}

impl Parser {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { message: msg.into(), line: Some(self.line) }
    }

    fn parse_statement(&mut self, stmt: &str) -> Result<(), ParseError> {
        if stmt == "--" {
            return Err(self.err("`--` (concurrency) is not supported"));
        }
        let first = stmt.split_whitespace().next().unwrap_or("");
        match first {
            "state" => return self.parse_state_decl(stmt),
            "direction" => return Err(self.err("`direction` statements are not supported")),
            "classDef" | "class" => {
                return Err(self.err(format!("`{first}` statements are not supported")));
            }
            _ if stmt.starts_with("accTitle") || stmt.starts_with("accDescr") => {
                let kw = if stmt.starts_with("accTitle") { "accTitle" } else { "accDescr" };
                return Err(self.err(format!("`{kw}` statements are not supported")));
            }
            "}" if stmt == "}" => return self.parse_composite_close(),
            _ if first.eq_ignore_ascii_case("note") => return self.parse_note(stmt),
            _ => {}
        }
        self.parse_transition(stmt)
    }

    /// `state "Display" as ID` / `state ID {` / `state ID <<stereotype>>`
    /// / bare `state ID`.
    fn parse_state_decl(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("state").unwrap().trim_start();
        if let Some(after_open_quote) = rest.strip_prefix('"') {
            let Some(qi) = after_open_quote.find('"') else {
                return Err(self.err("unclosed `\"` in state display text"));
            };
            let display = &after_open_quote[..qi];
            let after_quote = after_open_quote[qi + 1..].trim_start();
            let Some(id_part) = after_quote.strip_prefix("as") else {
                return Err(self.err("expected `as` after quoted display text"));
            };
            let id_part = id_part.trim_start();
            // ASCII id scan: char count == byte length only because the
            // predicate is ASCII-only; do not relax without a
            // byte-position scan.
            let id_len =
                id_part.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
            if id_len == 0 {
                return Err(self.err("expected a state id after `as`"));
            }
            let id = &id_part[..id_len];
            let trailing = id_part[id_len..].trim();
            if !trailing.is_empty() {
                return Err(self.err(format!("unexpected text after state id: {trailing:?}")));
            }
            if self.composite_ids.contains_key(id) {
                return Err(self.err(format!(
                    "`{id}` is a composite state; composites are not states"
                )));
            }
            let idx = self.ensure_node(id)?;
            self.g.nodes[idx].display = display.to_string();
            return Ok(());
        }

        // ASCII id scan: char count == byte length only because the
        // predicate is ASCII-only; do not relax without a byte-position
        // scan.
        let id_len = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
        if id_len == 0 {
            return Err(self.err("expected a state id after `state`"));
        }
        let id = rest[..id_len].to_string();
        let after_id = rest[id_len..].trim_start();
        if after_id == "{" {
            return self.open_composite(id);
        }
        if let Some(stereo_rest) = after_id.strip_prefix("<<") {
            return self.parse_stereotype(&id, stereo_rest);
        }
        if after_id.is_empty() {
            if self.composite_ids.contains_key(&id) {
                return Err(self.err(format!(
                    "`{id}` is a composite state; composites are not states"
                )));
            }
            self.ensure_node(&id)?;
            return Ok(());
        }
        Err(self.err(format!("unexpected text after state id: {after_id:?}")))
    }

    /// `<<choice>>` / `<<fork>>` / `<<join>>` (fork and join both map to
    /// `ForkJoin`); anything else in `<<...>>` errors naming it.
    fn parse_stereotype(&mut self, id: &str, stereo_rest: &str) -> Result<(), ParseError> {
        let Some(gi) = stereo_rest.find(">>") else {
            return Err(self.err("unclosed `<<` stereotype"));
        };
        let word = stereo_rest[..gi].trim();
        let trailing = stereo_rest[gi + 2..].trim();
        if !trailing.is_empty() {
            return Err(self.err(format!("unexpected text after stereotype: {trailing:?}")));
        }
        let kind = match word {
            "choice" => StateKind::Choice,
            "fork" | "join" => StateKind::ForkJoin,
            other => return Err(self.err(format!("unknown stereotype `<<{other}>>`"))),
        };
        if self.composite_ids.contains_key(id) {
            return Err(
                self.err(format!("`{id}` is a composite state; composites are not states"))
            );
        }
        let idx = self.ensure_node(id)?;
        self.g.nodes[idx].kind = kind;
        Ok(())
    }

    /// `state ID {` — opens a composite; membership-on-creation like
    /// flowchart's subgraphs.
    fn open_composite(&mut self, id: String) -> Result<(), ParseError> {
        if self.ids.contains_key(&id) {
            return Err(self.err(format!("`{id}` is already a state, not a composite")));
        }
        let parent = self.stack.last().map(|&(i, _)| i);
        let idx = self.g.composites.len();
        self.g.composites.push(Composite { id: id.clone(), display: id.clone(), parent });
        self.composite_ids.insert(id, idx);
        self.stack.push((idx, self.line));
        Ok(())
    }

    /// Bare `}` — closes the innermost open composite.
    fn parse_composite_close(&mut self) -> Result<(), ParseError> {
        if self.stack.pop().is_none() {
            return Err(self.err("found `}` outside a composite state"));
        }
        Ok(())
    }

    /// `note left of|right of ID: text` — keyword `note` is
    /// case-insensitive; the block form (`note left of ID` with no
    /// colon, body on following lines, closed by `end note`) errors at
    /// its own line.
    fn parse_note(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt
            .split_once(char::is_whitespace)
            .map(|(_, r)| r.trim_start())
            .unwrap_or("");

        const PLACEMENTS: &[(&str, bool)] = &[("left of", false), ("right of", true)];
        let mut matched: Option<(&str, bool)> = None;
        for &(lit, right) in PLACEMENTS {
            let n = lit.len();
            // `is_char_boundary(n)` (checked before any byte/str slicing
            // at `n`) guarantees `rest[n..]` is a valid slice start, so
            // reading the next char there is safe even though `rest` may
            // contain multibyte UTF-8 past this ASCII literal.
            if rest.len() >= n
                && rest.is_char_boundary(n)
                && &rest[..n] == lit
                && (rest.len() == n || rest[n..].starts_with(char::is_whitespace))
            {
                matched = Some((lit, right));
                break;
            }
        }
        let Some((lit, right)) = matched else {
            return Err(self.err("note needs a placement (`left of` or `right of`)"));
        };
        let after_placement = rest[lit.len()..].trim_start();
        let Some((id_part, text)) = after_placement.split_once(':') else {
            return Err(self.err("multi-line notes are not supported"));
        };
        let id_part = id_part.trim();
        if id_part.is_empty()
            || !id_part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(self.err(format!("invalid state id {id_part:?}")));
        }
        let idx = self.ensure_node(id_part)?;
        self.g.notes.push(StateNote { state: idx, right, text: text.trim().to_string() });
        Ok(())
    }

    /// `endpoint --> endpoint [: label]`.
    fn parse_transition(&mut self, stmt: &str) -> Result<(), ParseError> {
        let mut rest = stmt;
        let from = self.parse_endpoint(&mut rest, EndpointRole::Source)?;
        let r = rest.trim_start();
        let Some(after_arrow) = r.strip_prefix("-->") else {
            return Err(self.err(format!("expected a transition (e.g. `-->`), found {r:?}")));
        };
        rest = after_arrow;
        let to = self.parse_endpoint(&mut rest, EndpointRole::Target)?;
        let tail = rest.trim_start();
        // `X:::class` — the label branch below would eat the first colon
        // and render an edge labeled `::class` (silent misparse, issue
        // #47). Styling application is out of scope; error naming it.
        if tail.starts_with(":::") {
            return Err(self.err("`:::` class styling is not supported"));
        }
        let label = match tail.strip_prefix(':') {
            Some(t) => Some(t.trim().to_string()),
            None if tail.is_empty() => None,
            None => {
                return Err(self.err(format!(
                    "unexpected text after transition target: {tail:?}"
                )))
            }
        };
        self.push_transition(from, to, label)
    }

    /// One transition endpoint: `[*]` (materializes a fresh Start/End
    /// node per `role`), a plain id, or one of the unsupported history
    /// pseudostates `[H]`/`[H*]` (error, named).
    fn parse_endpoint(
        &mut self,
        rest: &mut &str,
        role: EndpointRole,
    ) -> Result<usize, ParseError> {
        let r = rest.trim_start();
        // `[*]` must be matched BEFORE the id scan below: it is not part
        // of the id charset.
        if let Some(after) = r.strip_prefix("[*]") {
            *rest = after;
            return self.make_star_node(role);
        }
        if r.starts_with("[H*]") {
            return Err(self.err("history pseudostate `[H*]` is not supported"));
        }
        if r.starts_with("[H]") {
            return Err(self.err("history pseudostate `[H]` is not supported"));
        }
        // ASCII id scan: char count == byte length only because the
        // predicate is ASCII-only; do not relax without a byte-position
        // scan.
        let id_len = r.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
        if id_len == 0 {
            return Err(self.err(format!("expected a state id, found {r:?}")));
        }
        let id = &r[..id_len];
        *rest = &r[id_len..];
        if self.composite_ids.contains_key(id) {
            return Err(self.err(format!(
                "transitions to composite states are not supported: `{id}`"
            )));
        }
        self.ensure_node(id)
    }

    /// Fresh synthetic `Start`/`End` node for one `[*]` occurrence,
    /// scoped to the currently-open composite (if any).
    fn make_star_node(&mut self, role: EndpointRole) -> Result<usize, ParseError> {
        let (kind, id) = match role {
            EndpointRole::Source => {
                let n = self.start_count;
                self.start_count += 1;
                (StateKind::Start, format!("__start_{n}"))
            }
            EndpointRole::Target => {
                let n = self.end_count;
                self.end_count += 1;
                (StateKind::End, format!("__end_{n}"))
            }
        };
        self.push_node(id.clone(), id, kind)
    }

    /// Look up an existing node by id, or create it (implicit
    /// creation), scoped to the currently-open composite.
    fn ensure_node(&mut self, id: &str) -> Result<usize, ParseError> {
        if let Some(&i) = self.ids.get(id) {
            return Ok(i);
        }
        let idx = self.push_node(id.to_string(), id.to_string(), StateKind::Normal)?;
        self.ids.insert(id.to_string(), idx);
        Ok(idx)
    }

    fn push_node(&mut self, id: String, display: String, kind: StateKind) -> Result<usize, ParseError> {
        let idx = self.g.nodes.len();
        self.g.nodes.push(StateNode {
            id,
            display,
            kind,
            composite: self.stack.last().map(|&(i, _)| i),
        });
        if self.g.nodes.len() > crate::layout::MAX_NODES {
            return Err(self.err(format!(
                "diagram too large: too many states (max {})",
                crate::layout::MAX_NODES
            )));
        }
        Ok(idx)
    }

    fn push_transition(
        &mut self,
        from: usize,
        to: usize,
        label: Option<String>,
    ) -> Result<(), ParseError> {
        self.g.transitions.push(Transition { from, to, label });
        if self.g.transitions.len() > crate::layout::MAX_EDGES {
            return Err(self.err(format!(
                "diagram too large: too many transitions (max {})",
                crate::layout::MAX_EDGES
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{StateKind, StateGraph};

    fn p(src: &str) -> StateGraph {
        parse(src).expect("parse ok")
    }

    #[test]
    fn header_forms() {
        assert!(parse("stateDiagram-v2\ns1 --> s2").is_ok());
        assert!(parse("stateDiagram\ns1 --> s2").is_ok());
        assert!(parse("stateDiagram-v2;\ns1 --> s2").is_ok());
        assert_eq!(parse("s1 --> s2").unwrap_err().line, Some(1));
    }

    #[test]
    fn transitions_and_labels() {
        let g = p("stateDiagram-v2\nIdle --> Busy: work arrives\nBusy --> Idle");
        assert_eq!(g.transitions.len(), 2);
        assert_eq!(g.transitions[0].label.as_deref(), Some("work arrives"));
        assert_eq!(g.transitions[1].label, None);
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn star_creates_start_and_end_nodes() {
        let g = p("stateDiagram-v2\n[*] --> A\nA --> [*]");
        assert_eq!(g.nodes.len(), 3);
        let kinds: Vec<StateKind> = g.nodes.iter().map(|n| n.kind).collect();
        assert!(kinds.contains(&StateKind::Start));
        assert!(kinds.contains(&StateKind::End));
        // fresh node per occurrence
        let g2 = p("stateDiagram-v2\n[*] --> A\n[*] --> B");
        assert_eq!(
            g2.nodes.iter().filter(|n| n.kind == StateKind::Start).count(),
            2
        );
    }

    #[test]
    fn display_and_stereotypes() {
        let g = p("stateDiagram-v2\nstate \"Waiting for input\" as W\nstate C <<choice>>\nstate F <<fork>>\nstate J <<join>>\nW --> C");
        let w = g.nodes.iter().find(|n| n.id == "W").unwrap();
        assert_eq!(w.display, "Waiting for input");
        assert_eq!(g.nodes.iter().find(|n| n.id == "C").unwrap().kind, StateKind::Choice);
        assert_eq!(g.nodes.iter().find(|n| n.id == "F").unwrap().kind, StateKind::ForkJoin);
        assert_eq!(g.nodes.iter().find(|n| n.id == "J").unwrap().kind, StateKind::ForkJoin);
    }

    #[test]
    fn composites_nest_and_scope_membership() {
        let g = p("stateDiagram-v2\nstate Outer {\nstate Inner {\na --> b\n}\nc --> a\n}\nd --> c");
        assert_eq!(g.composites.len(), 2);
        assert_eq!(g.composites[1].parent, Some(0)); // Inner in Outer
        let a = g.nodes.iter().find(|n| n.id == "a").unwrap();
        assert_eq!(a.composite, Some(1)); // created inside Inner
        let c = g.nodes.iter().find(|n| n.id == "c").unwrap();
        assert_eq!(c.composite, Some(0));
        let d = g.nodes.iter().find(|n| n.id == "d").unwrap();
        assert_eq!(d.composite, None);
    }

    #[test]
    fn unclosed_composite_errors_at_opening_line() {
        let e = parse("stateDiagram-v2\na --> b\nstate X {\nb --> c").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("unclosed"));
    }

    #[test]
    fn stray_close_errors() {
        assert_eq!(parse("stateDiagram-v2\n}").unwrap_err().line, Some(2));
    }

    #[test]
    fn transition_to_composite_id_errors() {
        let e = parse("stateDiagram-v2\nstate X {\na --> b\n}\nc --> X").unwrap_err();
        assert_eq!(e.line, Some(5));
        assert!(e.message.contains("composite"));
    }

    #[test]
    fn notes_both_sides() {
        let g = p("stateDiagram-v2\na --> b\nNote left of a: hello\nnote right of b: world");
        assert_eq!(g.notes.len(), 2);
        assert!(!g.notes[0].right);
        assert!(g.notes[1].right);
        assert_eq!(g.notes[0].text, "hello");
    }

    #[test]
    fn multiline_note_block_errors() {
        let e = parse("stateDiagram-v2\na --> b\nnote left of a\nsome text\nend note").unwrap_err();
        assert_eq!(e.line, Some(3));
    }

    #[test]
    fn out_of_scope_statements_error_named() {
        for (stmt, kw) in [("--", "--"), ("a --> [H]", "[H]"), ("direction LR", "direction")] {
            let src = format!("stateDiagram-v2\na --> b\n{stmt}");
            let e = parse(&src).unwrap_err();
            assert_eq!(e.line, Some(3), "for {stmt}");
            assert!(e.message.contains(kw), "message names {kw}: {}", e.message);
        }
    }

    #[test]
    fn multibyte_no_panic() {
        let _ = parse("stateDiagram-v2\nstate \"Émile 🎭\" as e\ne --> f: héllo\u{2003}🎉");
        let _ = parse("stateDiagram-v2\na\u{2003}--> b");
    }

    #[test]
    fn node_cap_enforced() {
        let mut src = String::from("stateDiagram-v2\n");
        for i in 0..=crate::layout::MAX_NODES {
            src.push_str(&format!("s{i} --> s{}\n", i + 1));
        }
        assert!(parse(&src).unwrap_err().message.contains("too large"));
    }

    // ── Polish slice (issue #47) ─────────────────────────────────────
    // (docs/superpowers/specs/2026-07-11-mermaid-state-polish-design.md)

    #[test]
    fn triple_colon_target_errors_naming_the_operator() {
        // THE silent-misparse regression: the docs' own example used to
        // render an edge labeled `::notMoving`.
        let e = parse("stateDiagram-v2\n[*] --> Still:::notMoving").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains(":::"), "got: {}", e.message);
    }

    #[test]
    fn triple_colon_source_still_errors() {
        // Source-side was already loud (the arrow check finds `:::`);
        // pin it so the two sides stay consistent.
        let e = parse("stateDiagram-v2\nStill:::notMoving --> [*]").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn ordinary_labels_unaffected_by_colon_guard() {
        let g = p("stateDiagram-v2\na --> b: go: now");
        assert_eq!(g.transitions[0].label.as_deref(), Some("go: now"));
    }

    #[test]
    fn styling_and_acc_statements_error_named() {
        // These fell through to the transition parser and produced
        // misleading `expected a transition` messages.
        for stmt in [
            "classDef notMoving fill:white",
            "class Moving, Crash movement",
            "accTitle: My title",
            "accDescr: My description",
        ] {
            let src = format!("stateDiagram-v2\na --> b\n{stmt}");
            let e = parse(&src).unwrap_err();
            assert_eq!(e.line, Some(3), "for {stmt}");
            let kw = stmt.split([' ', ':']).next().unwrap();
            assert!(e.message.contains(kw), "message names {kw}: {}", e.message);
        }
    }

    #[test]
    fn keyword_prefixed_ids_still_parse() {
        // `first` is a whole-token match: ids that merely start with a
        // keyword are not captured by the named-error arms.
        let g = p("stateDiagram-v2\nclassA --> stateB");
        assert_eq!(g.nodes.len(), 2);
    }
}
