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

/// An open multi-line note block (`note left of X` with no colon, body
/// on following lines, closed by `end note`).
struct NoteBlock {
    state: usize,
    right: bool,
    body: Vec<String>,
    opening_line: usize,
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
    /// Open multi-line note block; body lines are captured verbatim until
    /// `end note`.
    note_block: Option<NoteBlock>,
    line: usize, // 1-based, for errors
    start_count: usize,
    end_count: usize,
}

/// Truncate a line at the first `%%` that begins the line or follows
/// whitespace — mermaid comments run to end of line. A `%%` glued to
/// non-whitespace (e.g. inside a label like `a%%b`) is left alone; a
/// single `%` never matches. `%` is ASCII, so the byte offset from
/// `match_indices` is a char boundary.
fn strip_trailing_comment(line: &str) -> &str {
    for (i, _) in line.match_indices("%%") {
        if i == 0 || line[..i].ends_with(char::is_whitespace) {
            return &line[..i];
        }
    }
    line
}

/// Split a post-header line into `;`-separated statements — EXCEPT that
/// once a `:` has appeared in the current statement (a transition label,
/// colon description, or note text), the `;` belongs to that text and
/// the statement runs to end of line, matching mermaid. `;` and `:` are
/// ASCII, so char_indices offsets are boundary-safe.
fn split_statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut colon_seen = false;
    for (i, c) in line.char_indices() {
        match c {
            ':' => colon_seen = true,
            ';' if !colon_seen => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out
}

pub(crate) fn parse(source: &str) -> Result<StateGraph, ParseError> {
    let mut p = Parser {
        g: StateGraph {
            nodes: vec![],
            transitions: vec![],
            notes: vec![],
            composites: vec![],
            class_defs: vec![],
        },
        ids: HashMap::new(),
        composite_ids: HashMap::new(),
        stack: Vec::new(),
        note_block: None,
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
        let line = strip_trailing_comment(line).trim_end();
        if line.is_empty() {
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
        // Inside a multi-line note block, every line is body text until
        // the closing `end note` — no statement splitting or transition
        // parsing applies.
        if p.note_block.is_some() {
            if line == "end note" {
                p.close_note_block();
            } else {
                p.note_block.as_mut().expect("checked is_some").body.push(line.to_string());
            }
            continue;
        }
        for stmt in split_statements(line) {
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
    if let Some(nb) = &p.note_block {
        return Err(ParseError {
            message: format!("unclosed note block on `{}`", p.g.nodes[nb.state].id),
            line: Some(nb.opening_line),
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
        // `State:::className` (standalone) attaches a style class.
        if let Some((id, cls)) = stmt.split_once(":::") {
            let id = id.trim();
            let cls = cls.trim();
            if !id.is_empty() && !cls.is_empty() && !cls.contains(char::is_whitespace)
                && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && cls.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                let idx = self.ensure_node(id)?;
                self.g.nodes[idx].classes.push(cls.to_string());
                return Ok(());
            }
        }
        let first = stmt.split_whitespace().next().unwrap_or("");
        match first {
            "state" => return self.parse_state_decl(stmt),
            "direction" => return Err(self.err("`direction` statements are not supported")),
            "classDef" => return self.parse_class_def(stmt),
            "class" => return self.parse_class_assign(stmt),
            "style" => return self.parse_style(stmt),
            "linkStyle" => return self.parse_link_style(stmt),
            _ if stmt.starts_with("accTitle") || stmt.starts_with("accDescr") => {
                let kw = if stmt.starts_with("accTitle") { "accTitle" } else { "accDescr" };
                return Err(self.err(format!("`{kw}` statements are not supported")));
            }
            "}" if stmt == "}" => return self.parse_composite_close(),
            _ if first.eq_ignore_ascii_case("note") => return self.parse_note(stmt),
            _ => {}
        }
        if self.try_parse_decl(stmt)? {
            return Ok(());
        }
        self.parse_transition(stmt)
    }

    /// Bare-id declarations (`stateId`) and colon descriptions
    /// (`id : display text`) — both start with an id and are NOT
    /// transitions. Returns Ok(false) when the statement doesn't match
    /// either form (so the transition parser gets its turn).
    fn try_parse_decl(&mut self, stmt: &str) -> Result<bool, ParseError> {
        // ASCII id scan: char count == byte length only because the
        // predicate is ASCII-only; do not relax without a byte-position
        // scan.
        let id_len = stmt.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
        if id_len == 0 {
            return Ok(false);
        }
        let id = &stmt[..id_len];
        let after = stmt[id_len..].trim_start();
        let description = if after.is_empty() {
            None
        } else if let Some(rest) = after.strip_prefix(':') {
            // `:::`/`::` are not descriptions; let the transition parser
            // produce its loud error (Task 1 owns `:::` targets).
            if rest.starts_with(':') {
                return Ok(false);
            }
            let text = rest.trim();
            if text.is_empty() {
                return Err(self.err("empty state description"));
            }
            Some(text.to_string())
        } else {
            return Ok(false);
        };
        if self.composite_ids.contains_key(id) {
            return Err(self.err(format!(
                "`{id}` is a composite state; composites are not states"
            )));
        }
        let idx = self.ensure_node(id)?;
        if let Some(text) = description {
            // mermaid stacks repeated descriptions as extra lines; the
            // label emitter renders `<br/>`-separated lines already. A
            // display still equal to the id is the untouched default and
            // is replaced rather than appended to.
            if self.g.nodes[idx].display == self.g.nodes[idx].id {
                self.g.nodes[idx].display = text;
            } else {
                self.g.nodes[idx].display.push_str("<br/>");
                self.g.nodes[idx].display.push_str(&text);
            }
        }
        Ok(true)
    }

    /// `classDef name prop:val,...`
    fn parse_class_def(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("classDef").unwrap().trim();
        let Some((name, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("classDef needs a name and styles"));
        };
        self.g.class_defs.push(crate::style::ClassDef {
            name: name.trim().to_string(),
            style: crate::style::sanitize_style(styles),
        });
        Ok(())
    }

    /// `class id[,id...] className`
    fn parse_class_assign(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("class").unwrap().trim();
        let Some((ids, name)) = rest.rsplit_once(char::is_whitespace) else {
            return Err(self.err("class needs a state list and a class name"));
        };
        let name = name.trim();
        for id in ids.trim().trim_matches('"').split(',') {
            let idx = self.ensure_node(id.trim())?;
            self.g.nodes[idx].classes.push(name.to_string());
        }
        Ok(())
    }

    /// `style id prop:val,...`
    fn parse_style(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("style").unwrap().trim();
        let Some((id, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("style needs a state id and styles"));
        };
        let id = id.trim();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(self.err("style refers to unknown/invalid id"));
        }
        let idx = self.ensure_node(id)?;
        let s = crate::style::sanitize_style(styles);
        if !s.is_empty() {
            self.g.nodes[idx].style = Some(s);
        }
        Ok(())
    }

    /// `linkStyle <index[,index...]|default> prop:val,...` — styles one or
    /// more transitions, addressed by declaration index.
    fn parse_link_style(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("linkStyle").unwrap().trim();
        let Some((sel, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("linkStyle needs an index and styles"));
        };
        let s = crate::style::sanitize_style(styles);
        if s.is_empty() {
            return Ok(());
        }
        let edges = &mut self.g.transitions;
        if sel.trim() == "default" {
            for e in edges.iter_mut() {
                e.style = Some(s.clone());
            }
        } else {
            for tok in sel.split(',') {
                if let Ok(i) = tok.trim().parse::<usize>() {
                    if let Some(e) = edges.get_mut(i) {
                        e.style = Some(s.clone());
                    }
                }
            }
        }
        Ok(())
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
        match after_placement.split_once(':') {
            // Single-line form: `note left of X: text`.
            Some((id_part, text)) => {
                let idx = self.resolve_note_target(id_part.trim())?;
                self.g.notes.push(StateNote { state: idx, right, text: text.trim().to_string() });
                Ok(())
            }
            // Block form: `note left of X` opens a note whose body runs on
            // the following lines until `end note`.
            None => {
                let idx = self.resolve_note_target(after_placement.trim())?;
                self.note_block =
                    Some(NoteBlock { state: idx, right, body: Vec::new(), opening_line: self.line });
                Ok(())
            }
        }
    }

    /// Validates and interns a note's target state id (shared by the
    /// single-line and block note forms).
    fn resolve_note_target(&mut self, id: &str) -> Result<usize, ParseError> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(self.err(format!("invalid state id {id:?}")));
        }
        if id.starts_with("__") {
            return Err(self.err("cannot attach a note to a synthetic state id"));
        }
        self.ensure_node(id)
    }

    /// Closes an open note block, joining its body lines with `<br/>`
    /// (the state label/note renderer already splits on `<br/>`).
    fn close_note_block(&mut self) {
        if let Some(nb) = self.note_block.take() {
            self.g.notes.push(StateNote {
                state: nb.state,
                right: nb.right,
                text: nb.body.join("<br/>"),
            });
        }
    }

    /// `endpoint --> endpoint [: label]`.
    fn parse_transition(&mut self, stmt: &str) -> Result<(), ParseError> {
        let mut rest = stmt;
        let from = self.parse_endpoint(&mut rest, EndpointRole::Source)?;
        self.consume_class_suffix(&mut rest, from)?;
        let r = rest.trim_start();
        let Some(after_arrow) = r.strip_prefix("-->") else {
            return Err(self.err(format!("expected a transition (e.g. `-->`), found {r:?}")));
        };
        rest = after_arrow;
        let to = self.parse_endpoint(&mut rest, EndpointRole::Target)?;
        self.consume_class_suffix(&mut rest, to)?;
        let tail = rest.trim_start();
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

    /// Strip and apply an optional `:::className` suffix immediately
    /// following a transition endpoint (`rest` starts right after the
    /// endpoint was scanned). A no-op when no `:::` is present.
    fn consume_class_suffix(&mut self, rest: &mut &str, node_idx: usize) -> Result<(), ParseError> {
        let r = rest.trim_start();
        if let Some(after) = r.strip_prefix(":::") {
            let n = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
            if n == 0 {
                return Err(self.err("expected a class name after `:::`"));
            }
            self.g.nodes[node_idx].classes.push(after[..n].to_string());
            *rest = &after[n..];
        }
        Ok(())
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
            classes: vec![],
            style: None,
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
        self.g.transitions.push(Transition { from, to, label, style: None });
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
    fn multiline_note_block() {
        let g = p("stateDiagram-v2\na --> b\nnote left of a\nline one\nline two\nend note");
        assert_eq!(g.notes.len(), 1);
        assert!(!g.notes[0].right);
        assert_eq!(g.notes[0].text, "line one<br/>line two");
        // right-of block form + a single-line note still coexist.
        let g = p("stateDiagram-v2\na --> b\nnote right of b\nonly line\nend note\nnote left of a: quick");
        assert_eq!(g.notes.len(), 2);
        assert!(g.notes[0].right);
        assert_eq!(g.notes[0].text, "only line");
        assert_eq!(g.notes[1].text, "quick");
    }

    #[test]
    fn unclosed_note_block_errors_at_opening_line() {
        let e = parse("stateDiagram-v2\na --> b\nnote left of a\nsome text").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("unclosed note"), "got: {}", e.message);
    }

    #[test]
    fn note_block_body_is_not_parsed_as_statements() {
        // Body lines that look like transitions/keywords are literal text,
        // not parsed — the whole block is one note.
        let g = p("stateDiagram-v2\nnote left of a\nx --> y\nstate Z {\nend note\na --> b");
        assert_eq!(g.notes.len(), 1);
        assert_eq!(g.notes[0].text, "x --> y<br/>state Z {");
        // Only the real transition outside the block was parsed.
        assert_eq!(g.transitions.len(), 1);
        assert_eq!(g.composites.len(), 0);
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
    fn triple_colon_target_attaches_class() {
        // Was THE silent-misparse regression (used to render an edge
        // labeled `::notMoving`, then later a loud error); Task 3 makes
        // it the documented styling shorthand.
        let g = p("stateDiagram-v2\n[*] --> Still:::notMoving");
        let still = g.nodes.iter().find(|n| n.id == "Still").unwrap();
        assert_eq!(still.classes, vec!["notMoving".to_string()]);
    }

    #[test]
    fn triple_colon_source_attaches_class() {
        // Source-side `:::` attaches the class the same way the target
        // side does.
        let g = p("stateDiagram-v2\nStill:::notMoving --> [*]");
        let still = g.nodes.iter().find(|n| n.id == "Still").unwrap();
        assert_eq!(still.classes, vec!["notMoving".to_string()]);
    }

    #[test]
    fn spaceless_arrow_with_class_suffix_is_a_transition() {
        // Regression: `A-->B:::c` must be a transition (2 nodes + 1 edge)
        // with class `c` on B — not swallowed as one node id "A-->B".
        let g = p("stateDiagram-v2\nA-->B:::c");
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.transitions.len(), 1);
        let b = g.nodes.iter().find(|n| n.id == "B").expect("node B exists");
        assert_eq!(b.classes, vec!["c".to_string()]);
        // Standalone form still attaches.
        let g2 = p("stateDiagram-v2\n[*] --> S\nS:::hot");
        assert_eq!(g2.nodes.iter().find(|n| n.id == "S").unwrap().classes, vec!["hot".to_string()]);
    }

    #[test]
    fn ordinary_labels_unaffected_by_colon_guard() {
        let g = p("stateDiagram-v2\na --> b: go: now");
        assert_eq!(g.transitions[0].label.as_deref(), Some("go: now"));
    }

    #[test]
    fn acc_statements_error_named() {
        // `classDef`/`class` moved out of this list: Task 3 turns them
        // into supported styling statements (see `state_styling_parses`
        // and `class_assign_to_multiple_ids` below). `accTitle`/`accDescr`
        // remain out of scope and still fall through to the transition
        // parser with a misleading-message guard, so pin the named error.
        for stmt in ["accTitle: My title", "accDescr: My description"] {
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

    #[test]
    fn bare_id_declares_a_state() {
        // The docs' intro example: a statement that is just an id.
        let g = p("stateDiagram-v2\nstateId");
        assert_eq!(g.nodes.len(), 1);
        assert_eq!(g.nodes[0].id, "stateId");
        // Re-declaring is a no-op.
        let g = p("stateDiagram-v2\ns\ns\ns --> t");
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn colon_description_sets_display() {
        let g = p("stateDiagram-v2\ns2 : This is a state description");
        assert_eq!(g.nodes[0].display, "This is a state description");
        assert_eq!(g.nodes[0].id, "s2");
    }

    #[test]
    fn repeated_colon_description_appends_lines() {
        let g = p("stateDiagram-v2\ns : first line\ns : second line");
        assert_eq!(g.nodes[0].display, "first line<br/>second line");
    }

    #[test]
    fn colon_description_composes_with_quoted_decl() {
        let g = p("stateDiagram-v2\nstate \"Base\" as s\ns : more");
        assert_eq!(g.nodes[0].display, "Base<br/>more");
    }

    #[test]
    fn bare_or_described_composite_id_errors() {
        let e = parse("stateDiagram-v2\nstate X {\na\n}\nX").unwrap_err();
        assert_eq!(e.line, Some(5));
        assert!(e.message.contains("composite"));
        let e = parse("stateDiagram-v2\nstate X {\na\n}\nX : desc").unwrap_err();
        assert_eq!(e.line, Some(5));
    }

    #[test]
    fn double_colon_still_falls_through_to_a_loud_error() {
        // `s ::x` is not a description; the transition parser rejects it.
        let e = parse("stateDiagram-v2\ns ::x").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn trailing_comment_after_transition() {
        // Doc-blessed spelling: `Moving --> Still %% another comment`.
        let g = p("stateDiagram-v2\nMoving --> Still %% another comment");
        assert_eq!(g.transitions.len(), 1);
        assert_eq!(g.transitions[0].label, None);
    }

    #[test]
    fn trailing_comment_runs_to_end_of_line_across_semicolons() {
        let g = p("stateDiagram-v2\na --> b %% comment; not --> parsed");
        assert_eq!(g.transitions.len(), 1);
    }

    #[test]
    fn single_percent_in_label_survives() {
        let g = p("stateDiagram-v2\na --> b: 50% done");
        assert_eq!(g.transitions[0].label.as_deref(), Some("50% done"));
    }

    #[test]
    fn note_on_synthetic_id_errors() {
        // `__start_0`/`__end_0` are the synthesizer's reserved ids;
        // targeting one used to mint a phantom normal node (filed on
        // #32). A user state deliberately named `__x` also errors —
        // acceptable per the spec.
        let e = parse("stateDiagram-v2\n[*] --> A\nnote right of __start_0: boo").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("synthetic"), "got: {}", e.message);
    }

    // ── Final review fix wave ────────────────────────────────────────

    #[test]
    fn semicolon_inside_label_stays_in_the_label() {
        // Final-review regression find: bare-id support turned the old
        // loud error into a phantom node `Stop`. Mermaid keeps `;` in
        // the label to end of line.
        let g = p("stateDiagram-v2\na --> b: go; Stop");
        assert_eq!(g.transitions.len(), 1);
        assert_eq!(g.transitions[0].label.as_deref(), Some("go; Stop"));
        assert_eq!(g.nodes.len(), 2, "no phantom node");
    }

    #[test]
    fn semicolon_inside_description_stays_in_the_description() {
        let g = p("stateDiagram-v2\ns : Hello; World");
        assert_eq!(g.nodes[0].display, "Hello; World");
        assert_eq!(g.nodes.len(), 1);
    }

    #[test]
    fn semicolons_before_any_colon_still_split() {
        let g = p("stateDiagram-v2\na --> b; c --> d");
        assert_eq!(g.transitions.len(), 2);
    }

    #[test]
    fn empty_colon_description_errors() {
        let e = parse("stateDiagram-v2\ns :").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("empty"), "got: {}", e.message);
    }

    #[test]
    fn class_assign_applies_to_multiple_ids() {
        let g = p("stateDiagram-v2\na --> b\nclass a, b movement");
        assert_eq!(g.nodes.iter().find(|n| n.id == "a").unwrap().classes, vec!["movement".to_string()]);
        assert_eq!(g.nodes.iter().find(|n| n.id == "b").unwrap().classes, vec!["movement".to_string()]);
    }

    #[test]
    fn single_underscore_note_target_is_valid() {
        // Only the double-underscore synthetic prefix is reserved.
        let g = p("stateDiagram-v2\n_x --> y\nnote right of _x: fine");
        assert_eq!(g.notes.len(), 1);
    }

    // ── Styling (Task 3) ──────────────────────────────────────────────

    #[test]
    fn state_styling_parses() {
        let g = p("stateDiagram-v2\nclassDef mov fill:#0f0\n[*] --> Still\nStill:::mov\nstyle Still fill:#00f");
        let still = g.nodes.iter().find(|n| n.id == "Still").unwrap();
        assert_eq!(still.classes, vec!["mov".to_string()]);
        assert_eq!(still.style.as_deref(), Some("fill:#00f"));
        assert_eq!(g.class_defs.iter().find(|d| d.name == "mov").unwrap().style, "fill:#0f0");
    }

    #[test]
    fn spaceless_arrow_on_class_side_is_a_transition() {
        // Regression: `A:::c-->B` must be a transition (2 nodes + 1 edge),
        // class `c` on A — not swallowed with class "c-->B".
        let g = p("stateDiagram-v2\nA:::c-->B");
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.transitions.len(), 1);
        assert_eq!(g.nodes.iter().find(|n| n.id == "A").unwrap().classes, vec!["c".to_string()]);
    }
}
