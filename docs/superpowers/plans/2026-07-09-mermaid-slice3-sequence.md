# Mermaid Slice 3 — Sequence Diagrams Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ogrenotes_mermaid::render()` renders `sequenceDiagram` sources to SVG via a bespoke two-pass lifeline layout, riding the existing pipeline with zero schema/frontend/export changes.

**Architecture:** New `crates/mermaid/src/sequence/` module mirroring `flowchart/`'s structure: `parse.rs` (statements → `SeqDiagram`), `layout.rs` (pass 1: participant columns widened by pair-spanning content; pass 2: top-down event walk assigning row y's, activation spans, fragment frames), `svg.rs` (document assembly). The shared text measurement module is promoted from `flowchart::measure` to crate-level `crate::measure` first. The slice-2 layered graph engine is NOT used.

**Tech Stack:** Pure Rust (std only at runtime), proptest dev-dependency (already present), wasm32-clean.

## Global Constraints

- Crate stays `#![forbid(unsafe_code)]`, **zero runtime dependencies**, wasm32-clean; `license.workspace = true`.
- `render()` never panics; exactly one of `svg`/`error` set (XOR invariant test stays green). UTF-8 slice discipline: no manual byte arithmetic after char-predicate counting unless the predicate is ASCII-only (comment it); prefer `split_once`/`rsplit_once`/`char_indices`.
- Caps enforced DURING PARSE (bounded before any layout work): `MAX_PARTICIPANTS = 50`, `MAX_EVENTS = 1000`, `MAX_FRAGMENT_DEPTH = 16` — over-cap → per-line `ParseError` "diagram too large: …". `render()`'s existing `MAX_SOURCE_LEN` gate applies before parsing.
- Layout is deterministic (no HashMap iteration into output), never panics, produces finite coordinates and monotonically increasing row y's.
- Every user string reaching SVG goes through `crate::escape_xml`; participant/fragment ids never emitted.
- Errors carry 1-based line numbers where known; first error wins; unclosed fragments error at the OPENING line.
- Tests immutable: slice 1/2 assertions untouched; the `measure` promotion (Task 1) must leave every existing test green UNCHANGED.
- No `git add -A`; stage explicit paths. Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Verify per task: `cargo test -p ogrenotes-mermaid` and `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown`.

## File Structure

```
crates/mermaid/src/
  measure.rs             MOVED from flowchart/measure.rs (Task 1, unchanged content)
  lib.rs                 modify: mod measure; mod sequence; Sequence arm (Task 7)
  flowchart/mod.rs       modify: drop `pub(crate) mod measure;`, update use paths
  flowchart/svg.rs       modify: use path update only
  sequence/mod.rs        model types, caps, render_sequence entry (Tasks 2, 7)
  sequence/parse.rs      statement parser (Tasks 3–4)
  sequence/layout.rs     two-pass lifeline layout (Task 5)
  sequence/svg.rs        SVG assembly (Task 6)
```

---

### Task 1: promote `measure` to crate level

**Files:**
- Rename: `crates/mermaid/src/flowchart/measure.rs` → `crates/mermaid/src/measure.rs` (use `git mv`)
- Modify: `crates/mermaid/src/lib.rs` (add `pub(crate) mod measure;`)
- Modify: `crates/mermaid/src/flowchart/mod.rs` (remove `pub(crate) mod measure;`; update `measure::` references)
- Modify: `crates/mermaid/src/flowchart/svg.rs` (update the `use crate::flowchart::{measure, …}` import to pull `measure` from `crate::`)

**Interfaces:**
- Produces: `crate::measure::{text_size, lines, FONT_PX, LINE_H}` — signatures unchanged: `text_size(s: &str) -> (f64, f64)`, `lines(s: &str) -> Vec<&str>`, `FONT_PX: f64 = 14.0`, `LINE_H: f64 = 19.0`.
- CONTRACT: pure move — zero content changes inside measure.rs (its `#[cfg(test)]` module moves with it); zero behavior change anywhere; every existing test passes unchanged.

- [ ] **Step 1: Move the file and update module declarations**

```bash
git mv crates/mermaid/src/flowchart/measure.rs crates/mermaid/src/measure.rs
```

In `crates/mermaid/src/lib.rs`, add `pub(crate) mod measure;` next to the existing `mod` declarations. In `crates/mermaid/src/flowchart/mod.rs`, delete the `pub(crate) mod measure;` line.

- [ ] **Step 2: Fix the use sites**

Find every reference: `grep -rn "measure" crates/mermaid/src/flowchart/`. Expected sites: `flowchart/mod.rs` (`render_flowchart` calls `measure::text_size`) and `flowchart/svg.rs` (imports `measure` from `crate::flowchart`). Update each to `crate::measure` (either `use crate::measure;` at the top or qualified paths — match each file's existing import style). Do NOT touch measure.rs content.

- [ ] **Step 3: Verify zero behavior change**

Run: `cargo test -p ogrenotes-mermaid`
Expected: identical pass count to before the move (run `git stash list`-free baseline first if unsure: on HEAD it's 150). All flowchart, pie, layout, lib tests green with no test-file diffs.
Run: `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown` → clean.

- [ ] **Step 4: Commit**

```bash
git add crates/mermaid/src/measure.rs crates/mermaid/src/flowchart/measure.rs crates/mermaid/src/lib.rs crates/mermaid/src/flowchart/mod.rs crates/mermaid/src/flowchart/svg.rs
git commit -m "refactor(mermaid): promote measure to crate level for sequence reuse

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: sequence model types + caps

**Files:**
- Create: `crates/mermaid/src/sequence/mod.rs`
- Modify: `crates/mermaid/src/lib.rs` (add `mod sequence;`)

**Interfaces:**
- Produces (all `pub(crate)`, consumed by Tasks 3–7):

```rust
//! Mermaid sequence diagrams: parser → two-pass lifeline layout → SVG.
//! Bespoke layout (participant columns + event rows) — the layered
//! graph engine in `crate::layout` is deliberately not involved.

// TODO(slice3): removed in Task 7
#![allow(dead_code)]

pub(crate) mod parse;   // Task 3 adds; declare then
pub(crate) mod layout;  // Task 5 adds; declare then
pub(crate) mod svg;     // Task 6 adds; declare then

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
        head: Head,
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
```

- [ ] **Step 1: Create the module** with exactly the content above, but with the three `pub(crate) mod …;` lines COMMENTED OUT (`// pub(crate) mod parse; // Task 3`) since the files don't exist yet — each later task uncomments its own line. Add `mod sequence;` to `lib.rs`.

- [ ] **Step 2: Verify** — `cargo test -p ogrenotes-mermaid` (all green, count unchanged) and wasm build clean. The `#![allow(dead_code)]` marker suppresses unused-type warnings until Task 7 removes it.

- [ ] **Step 3: Commit**

```bash
git add crates/mermaid/src/sequence/mod.rs crates/mermaid/src/lib.rs
git commit -m "feat(mermaid): sequence diagram model types and caps

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: parser — header, participants, messages, autonumber

**Files:**
- Create: `crates/mermaid/src/sequence/parse.rs`
- Modify: `crates/mermaid/src/sequence/mod.rs` (uncomment `pub(crate) mod parse;`)

**Interfaces:**
- Consumes: model types (Task 2), `crate::ParseError`.
- Produces: `pub(crate) fn parse(source: &str) -> Result<SeqDiagram, ParseError>` — Task 3 scope: `sequenceDiagram` header (first non-blank/non-`%%` line, exact keyword, nothing else on the line except optional trailing `;`), `participant ID` / `participant ID as Display` / `actor ID [as Display]`, all six arrow forms with `+`/`-` shorthand, message text after `:` (optional — empty text allowed), self-messages, implicit participant creation, `autonumber`, `%%` comments, blank lines. Notes/fragments/activate statements are Task 4 (until then such lines produce a generic "unsupported statement" error — Task 4 refines).

**Message grammar (write exactly this):** a message statement is `IDENT ARROW ['+'|'-'] IDENT [':' text]`. Arrow forms, LONGEST FIRST: `-->>` (Dotted, Arrow), `-->` (Dotted, None), `->>` (Solid, Arrow), `->` (Solid, None), `--x` (Dotted, Cross), `-x` (Solid, Cross), `--)` (Dotted, Async), `-)` (Solid, Async). A `+` immediately before the target id sets `activate_target`; a `-` there sets `deactivate_source` (mermaid semantics: `+` activates the receiver, `-` deactivates the SENDER). Ids: `[A-Za-z0-9_]+` (ASCII-only predicate — byte-safe slicing, comment it like flowchart's parser). Participant ids are shared with implicit creation: the display defaults to the id.

**Semantic tracking for the shorthand (needed for Task 4's error too):** the parser maintains `active_depth: Vec<usize>` per participant; `+` increments the target's depth; `-` checks the SOURCE's depth — if zero, per-line error "cannot deactivate ‹id›: not active"; else decrements.

- [ ] **Step 1: Write the failing tests**

Create `crates/mermaid/src/sequence/parse.rs` with this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::{Event, Head, LineStyle};

    fn p(src: &str) -> crate::sequence::SeqDiagram {
        parse(src).expect("parse ok")
    }

    fn msg(e: &Event) -> (usize, usize, LineStyle, Head, &str) {
        match e {
            Event::Message { from, to, line, head, text, .. } => (*from, *to, *line, *head, text.as_str()),
            other => panic!("expected message, got {other:?}"),
        }
    }

    #[test]
    fn header_required() {
        let e = parse("A->>B: hi").unwrap_err();
        assert_eq!(e.line, Some(1));
        assert!(parse("sequenceDiagram\nA->>B: hi").is_ok());
        assert!(parse("sequenceDiagram;\nA->>B: hi").is_ok()); // trailing ;
    }

    #[test]
    fn participant_declarations() {
        let g = p("sequenceDiagram\nparticipant A\nparticipant B as Bob Smith\nactor C as Carol");
        assert_eq!(g.participants.len(), 3);
        assert_eq!(g.participants[0].display, "A");
        assert_eq!(g.participants[1].display, "Bob Smith");
        assert!(!g.participants[1].is_actor);
        assert!(g.participants[2].is_actor);
        assert_eq!(g.participants[2].id, "C");
    }

    #[test]
    fn implicit_participants_in_declaration_order() {
        let g = p("sequenceDiagram\nZed->>Amy: hi\nAmy-->>Zed: yo");
        assert_eq!(g.participants[0].id, "Zed"); // first appearance wins
        assert_eq!(g.participants[1].id, "Amy");
        assert_eq!(g.participants.len(), 2);
    }

    #[test]
    fn all_arrow_forms() {
        let cases: &[(&str, LineStyle, Head)] = &[
            ("A->B: t", LineStyle::Solid, Head::None),
            ("A-->B: t", LineStyle::Dotted, Head::None),
            ("A->>B: t", LineStyle::Solid, Head::Arrow),
            ("A-->>B: t", LineStyle::Dotted, Head::Arrow),
            ("A-xB: t", LineStyle::Solid, Head::Cross),
            ("A--xB: t", LineStyle::Dotted, Head::Cross),
            ("A-)B: t", LineStyle::Solid, Head::Async),
            ("A--)B: t", LineStyle::Dotted, Head::Async),
        ];
        for (src, want_line, want_head) in cases {
            let g = p(&format!("sequenceDiagram\n{src}"));
            let (_, _, line, head, text) = msg(&g.events[0]);
            assert_eq!(line, *want_line, "for {src}");
            assert_eq!(head, *want_head, "for {src}");
            assert_eq!(text, "t", "for {src}");
        }
    }

    #[test]
    fn message_without_text() {
        let g = p("sequenceDiagram\nA->>B");
        assert_eq!(msg(&g.events[0]).4, "");
    }

    #[test]
    fn self_message() {
        let g = p("sequenceDiagram\nA->>A: think");
        let (from, to, ..) = msg(&g.events[0]);
        assert_eq!(from, to);
    }

    #[test]
    fn activation_shorthand() {
        let g = p("sequenceDiagram\nA->>+B: go\nB-->>-A: done");
        match &g.events[0] {
            Event::Message { activate_target, deactivate_source, .. } => {
                assert!(*activate_target && !*deactivate_source);
            }
            _ => unreachable!(),
        }
        match &g.events[1] {
            Event::Message { activate_target, deactivate_source, .. } => {
                assert!(!*activate_target && *deactivate_source);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn deactivate_shorthand_without_active_errors() {
        let e = parse("sequenceDiagram\nB-->>-A: done").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("not active"), "got: {}", e.message);
    }

    #[test]
    fn autonumber_event() {
        let g = p("sequenceDiagram\nautonumber\nA->>B: hi");
        assert!(matches!(g.events[0], Event::Autonumber));
    }

    #[test]
    fn comments_and_blanks_skipped() {
        let g = p("sequenceDiagram\n%% c\n\nA->>B: hi");
        assert_eq!(g.events.len(), 1);
    }

    #[test]
    fn participant_cap_enforced() {
        let mut src = String::from("sequenceDiagram\n");
        for i in 0..=crate::sequence::MAX_PARTICIPANTS {
            src.push_str(&format!("participant p{i}\n"));
        }
        let e = parse(&src).unwrap_err();
        assert!(e.message.contains("too large"));
    }

    #[test]
    fn event_cap_enforced() {
        let mut src = String::from("sequenceDiagram\n");
        for _ in 0..=crate::sequence::MAX_EVENTS {
            src.push_str("A->>B: x\n");
        }
        let e = parse(&src).unwrap_err();
        assert!(e.message.contains("too large"));
    }

    #[test]
    fn unknown_statement_errors_with_line() {
        let e = parse("sequenceDiagram\nA->>B: ok\nwibble wobble").unwrap_err();
        assert_eq!(e.line, Some(3));
    }

    #[test]
    fn multibyte_input_no_panic() {
        // Multi-byte whitespace and emoji in display/text must not panic.
        let _ = parse("sequenceDiagram\nparticipant A as Émile 🎭\nA->>B: héllo 🎉");
        let _ = parse("sequenceDiagram\nA\u{2003}->>B: x");
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p ogrenotes-mermaid sequence::parse` → FAIL (module/function missing). Uncomment `pub(crate) mod parse;` in `sequence/mod.rs`.

- [ ] **Step 3: Implement**

Structure (write exactly this shape):

```rust
//! Sequence-diagram parser. Line-oriented; ids are ASCII word chars
//! (char count == byte length — do not relax without a byte-position
//! scan); arrows matched longest-first; caps enforced inline so work
//! is bounded before layout.

use crate::sequence::{
    Event, Head, LineStyle, Participant, SeqDiagram,
    MAX_EVENTS, MAX_PARTICIPANTS,
};
use crate::ParseError;
use std::collections::HashMap;

struct Parser {
    g: SeqDiagram,
    ids: HashMap<String, usize>,
    active_depth: Vec<usize>,
    line: usize,
}

pub(crate) fn parse(source: &str) -> Result<SeqDiagram, ParseError> {
    let mut p = Parser {
        g: SeqDiagram { participants: vec![], events: vec![] },
        ids: HashMap::new(),
        active_depth: vec![],
        line: 0,
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
            if header != "sequenceDiagram" {
                return Err(p.err("sequence diagram must start with `sequenceDiagram`"));
            }
            seen_header = true;
            continue;
        }
        p.parse_statement(line)?;
    }
    if !seen_header {
        return Err(ParseError {
            message: "sequence diagram must start with `sequenceDiagram`".into(),
            line: Some(1),
        });
    }
    Ok(p.g)
}

impl Parser {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { message: msg.into(), line: Some(self.line) }
    }

    fn push_event(&mut self, e: Event) -> Result<(), ParseError> {
        if self.g.events.len() >= MAX_EVENTS {
            return Err(self.err(format!(
                "diagram too large: more than {MAX_EVENTS} events"
            )));
        }
        self.g.events.push(e);
        Ok(())
    }

    fn intern(&mut self, id: &str, display: Option<String>, is_actor: bool) -> Result<usize, ParseError> {
        if let Some(&i) = self.ids.get(id) {
            // Explicit declaration after implicit use upgrades display/actor.
            if let Some(d) = display {
                self.g.participants[i].display = d;
                self.g.participants[i].is_actor = is_actor;
            }
            return Ok(i);
        }
        if self.g.participants.len() >= MAX_PARTICIPANTS {
            return Err(self.err(format!(
                "diagram too large: more than {MAX_PARTICIPANTS} participants"
            )));
        }
        let i = self.g.participants.len();
        self.g.participants.push(Participant {
            id: id.to_string(),
            display: display.unwrap_or_else(|| id.to_string()),
            is_actor,
        });
        self.ids.insert(id.to_string(), i);
        self.active_depth.push(0);
        Ok(i)
    }

    fn parse_statement(&mut self, stmt: &str) -> Result<(), ParseError> {
        let first = stmt.split_whitespace().next().unwrap_or("");
        match first {
            "participant" | "actor" => return self.parse_participant(stmt, first == "actor"),
            "autonumber" => return self.push_event(Event::Autonumber),
            // Task 4 adds: activate/deactivate, Note, fragments, end,
            // out-of-scope keywords. Until then:
            _ => {}
        }
        if self.try_parse_message(stmt)? {
            return Ok(());
        }
        Err(self.err(format!("unsupported statement: {first:?}")))
    }

    fn parse_participant(&mut self, stmt: &str, is_actor: bool) -> Result<(), ParseError> {
        let rest = stmt
            .split_once(char::is_whitespace)
            .map(|(_, r)| r.trim())
            .unwrap_or("");
        if rest.is_empty() {
            return Err(self.err("participant needs an id"));
        }
        let (id, display) = match rest.split_once(" as ") {
            Some((id, d)) => (id.trim(), Some(d.trim().to_string())),
            None => (rest, None),
        };
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(self.err(format!("invalid participant id {id:?}")));
        }
        if matches!(&display, Some(d) if d.is_empty()) {
            return Err(self.err("participant alias must not be empty"));
        }
        self.intern(id, display, is_actor)?;
        Ok(())
    }

    /// Try `IDENT ARROW [+|-] IDENT [: text]`. Returns Ok(false) if the
    /// statement doesn't start with an id followed by an arrow.
    fn try_parse_message(&mut self, stmt: &str) -> Result<bool, ParseError> {
        // Leading id: ASCII-only predicate, so char count == byte length.
        let id_len = stmt
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        if id_len == 0 {
            return Ok(false);
        }
        let from_id = &stmt[..id_len];
        let rest = stmt[id_len..].trim_start();
        // Arrows longest-first; each maps to (line, head).
        const ARROWS: &[(&str, LineStyle, Head)] = &[
            ("-->>", LineStyle::Dotted, Head::Arrow),
            ("-->", LineStyle::Dotted, Head::None),
            ("->>", LineStyle::Solid, Head::Arrow),
            ("->", LineStyle::Solid, Head::None),
            ("--x", LineStyle::Dotted, Head::Cross),
            ("-x", LineStyle::Solid, Head::Cross),
            ("--)", LineStyle::Dotted, Head::Async),
            ("-)", LineStyle::Solid, Head::Async),
        ];
        let Some((arrow, line_style, head)) = ARROWS
            .iter()
            .find(|(a, _, _)| rest.starts_with(a))
            .map(|(a, l, h)| (*a, *l, *h))
        else {
            return Ok(false);
        };
        let mut after = rest[arrow.len()..].trim_start();
        let mut activate_target = false;
        let mut deactivate_source = false;
        if let Some(r) = after.strip_prefix('+') {
            activate_target = true;
            after = r;
        } else if let Some(r) = after.strip_prefix('-') {
            deactivate_source = true;
            after = r;
        }
        let to_len = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        if to_len == 0 {
            return Err(self.err("expected a target participant after the arrow"));
        }
        let to_id = &after[..to_len];
        let tail = after[to_len..].trim_start();
        let text = match tail.strip_prefix(':') {
            Some(t) => t.trim().to_string(),
            None if tail.is_empty() => String::new(),
            None => {
                return Err(self.err(format!(
                    "unexpected text after message target: {tail:?}"
                )))
            }
        };
        let from = self.intern(&from_id.to_string(), None, false)?;
        let to = self.intern(&to_id.to_string(), None, false)?;
        if activate_target {
            self.active_depth[to] += 1;
        }
        if deactivate_source {
            if self.active_depth[from] == 0 {
                let id = self.g.participants[from].id.clone();
                return Err(self.err(format!("cannot deactivate {id:?}: not active")));
            }
            self.active_depth[from] -= 1;
        }
        self.push_event(Event::Message {
            from,
            to,
            line: line_style,
            head,
            text,
            activate_target,
            deactivate_source,
        })?;
        Ok(true)
    }
}
```

NOTE on arrow ordering: `-->>` must precede `-->` AND `->>`; `--x`/`--)` must precede `-x`/`-)`; the table above is ordered so every dotted form is tried before its solid prefix-sibling and every longer form before its shorter prefix. Verify against the `all_arrow_forms` test — if any case mismatches, fix the ORDER, not the test.

- [ ] **Step 4: Run tests** — `cargo test -p ogrenotes-mermaid sequence::parse` → PASS (14 tests). Full suite + wasm build green.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/sequence/parse.rs crates/mermaid/src/sequence/mod.rs
git commit -m "feat(mermaid): sequence parser — participants, messages, autonumber

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: parser — activations, notes, fragments, scope errors

**Files:**
- Modify: `crates/mermaid/src/sequence/parse.rs`

**Interfaces:**
- Extends `parse()` to full spec coverage: `activate ID` / `deactivate ID` statements (same depth tracking + "not active" error), `Note left of|right of|over` (case-insensitive `note` keyword; `over A,B` spanning), fragment opens (`loop|alt|opt|par|critical|break [label]`), `else`/`and` dividers (validated against the OPEN fragment kind: `else`→Alt, `and`→Par; `critical` also accepts `option` as its divider — NO: spec scopes dividers to else/and only; `option` is out of scope → error), `end` closes, `MAX_FRAGMENT_DEPTH`, EOF-unclosed error at opening line, out-of-scope keywords (`box`, `create`, `destroy`, `rect`, `links`, `link`, `properties`) error naming the keyword.

**Behavioral rules (write exactly these):**
- Fragment stack entries carry `(kind, opening_line)`. `FragmentOpen` when depth would exceed `MAX_FRAGMENT_DEPTH` → "diagram too large: fragment nesting deeper than 16".
- `else` with stack top ≠ Alt → error "`else` outside an `alt` fragment" (same pattern for `and`/Par).
- `end` with empty stack → "found `end` outside a fragment". EOF with non-empty stack → "unclosed `loop` fragment" (kind keyword) at the OPENING line.
- Note participants must exist OR are implicitly created (mermaid implicitly creates them — match that; `Note over A,B` interning both).
- `activate`/`deactivate` statements share `active_depth` with the shorthand.
- Labels (fragment + note text) are raw strings, trimmed; empty labels allowed for fragments, but a Note REQUIRES `: text` (mermaid errors without it) → error "note needs `: text`".

- [ ] **Step 1: Write the failing tests** (append to the test module)

```rust
    #[test]
    fn activate_deactivate_statements() {
        let g = p("sequenceDiagram\nA->>B: go\nactivate B\nB-->>A: ok\ndeactivate B");
        assert!(matches!(g.events[1], Event::Activate { p: 1 }));
        assert!(matches!(g.events[3], Event::Deactivate { p: 1 }));
    }

    #[test]
    fn deactivate_statement_without_active_errors() {
        let e = parse("sequenceDiagram\nparticipant A\ndeactivate A").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("not active"));
    }

    #[test]
    fn note_placements() {
        use crate::sequence::NotePlacement;
        let g = p("sequenceDiagram\nA->>B: x\nNote left of A: la\nnote right of B: rb\nNote over A: oa\nNote over A,B: ab");
        assert!(matches!(&g.events[1], Event::Note { placement: NotePlacement::LeftOf(0), .. }));
        assert!(matches!(&g.events[2], Event::Note { placement: NotePlacement::RightOf(1), .. }));
        assert!(matches!(&g.events[3], Event::Note { placement: NotePlacement::Over(0, None), .. }));
        assert!(matches!(&g.events[4], Event::Note { placement: NotePlacement::Over(0, Some(1)), .. }));
    }

    #[test]
    fn note_without_text_errors() {
        let e = parse("sequenceDiagram\nA->>B: x\nNote over A").unwrap_err();
        assert_eq!(e.line, Some(3));
    }

    #[test]
    fn note_implicitly_creates_participant() {
        let g = p("sequenceDiagram\nNote over Ghost: boo");
        assert_eq!(g.participants[0].id, "Ghost");
    }

    #[test]
    fn fragments_all_kinds_and_nesting() {
        use crate::sequence::FragmentKind;
        let g = p("sequenceDiagram\nloop every day\nA->>B: hi\nalt ok\nB-->>A: yes\nelse bad\nB--xA: no\nend\nend");
        assert!(matches!(&g.events[0], Event::FragmentOpen { kind: FragmentKind::Loop, .. }));
        assert!(matches!(&g.events[2], Event::FragmentOpen { kind: FragmentKind::Alt, .. }));
        assert!(matches!(&g.events[4], Event::FragmentDivider { .. }));
        assert!(matches!(&g.events[6], Event::FragmentClose));
        assert!(matches!(&g.events[7], Event::FragmentClose));
        for kw in ["opt", "par", "critical", "break"] {
            assert!(parse(&format!("sequenceDiagram\n{kw} l\nA->>B: x\nend")).is_ok(), "{kw}");
        }
    }

    #[test]
    fn par_uses_and_divider() {
        assert!(parse("sequenceDiagram\npar one\nA->>B: x\nand two\nA->>C: y\nend").is_ok());
        let e = parse("sequenceDiagram\npar one\nA->>B: x\nelse two\nend").unwrap_err();
        assert!(e.message.contains("else"));
    }

    #[test]
    fn else_outside_alt_errors() {
        let e = parse("sequenceDiagram\nloop l\nelse x\nend").unwrap_err();
        assert_eq!(e.line, Some(3));
    }

    #[test]
    fn end_without_fragment_errors() {
        let e = parse("sequenceDiagram\nend").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn unclosed_fragment_errors_at_opening_line() {
        let e = parse("sequenceDiagram\nA->>B: x\nloop forever\nB-->>A: y").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("unclosed"));
        assert!(e.message.contains("loop"));
    }

    #[test]
    fn fragment_depth_cap() {
        let mut src = String::from("sequenceDiagram\n");
        for _ in 0..=crate::sequence::MAX_FRAGMENT_DEPTH {
            src.push_str("loop l\n");
        }
        let e = parse(&src).unwrap_err();
        assert!(e.message.contains("too large"));
    }

    #[test]
    fn out_of_scope_statements_error_named() {
        for stmt in ["box Purple", "create participant X", "destroy A",
                     "rect rgb(0,0,0)", "links A: {}", "link A: x", "properties A: {}"] {
            let src = format!("sequenceDiagram\nA->>B: x\n{stmt}");
            let e = parse(&src).unwrap_err();
            assert_eq!(e.line, Some(3), "for {stmt}");
            let kw = stmt.split_whitespace().next().unwrap();
            assert!(e.message.contains(kw), "message names {kw}: {}", e.message);
        }
    }
```

- [ ] **Step 2: RED** — new tests fail (statements route to "unsupported statement" or message parse).

- [ ] **Step 3: Implement** — extend `parse_statement`'s match:

```rust
            "activate" | "deactivate" => return self.parse_activation(stmt, first == "activate"),
            "loop" | "alt" | "opt" | "par" | "critical" | "break" => {
                return self.parse_fragment_open(stmt, first)
            }
            "else" | "and" => return self.parse_divider(stmt, first),
            "end" if stmt == "end" => return self.parse_fragment_close(),
            "box" | "create" | "destroy" | "rect" | "links" | "link" | "properties" => {
                return Err(self.err(format!("`{first}` statements are not supported")));
            }
            _ if first.eq_ignore_ascii_case("note") => return self.parse_note(stmt),
```

Add to `Parser`: `frags: Vec<(FragmentKind, usize)>` (kind, opening line). Implement per the behavioral rules above:
- `parse_activation`: id after keyword must intern; `activate` bumps depth; `deactivate` checks-then-decrements with the "not active" error; push `Event::Activate/Deactivate`.
- `parse_fragment_open`: depth check vs `MAX_FRAGMENT_DEPTH`; label = remainder after keyword, trimmed (may be empty); push stack + event.
- `parse_divider`: stack-top kind must be Alt for `else` / Par for `and`; label = remainder; push `FragmentDivider`.
- `parse_fragment_close`: pop or error; push `FragmentClose`.
- `parse_note`: after case-insensitive `note`, expect `left of` / `right of` / `over` (case-insensitive on the placement words), then one id or `A,B` (over only), then required `: text`. Use `split_once(':')` for the text (byte-safe); intern participants implicitly.
- End of `parse()`: if `p.frags` non-empty, error `format!("unclosed `{}` fragment", kind_keyword)` with the opening line (add a `FragmentKind::keyword() -> &'static str` helper on the enum in `sequence/mod.rs`).

- [ ] **Step 4: GREEN** — `cargo test -p ogrenotes-mermaid sequence::` all pass (27 tests); full suite + wasm green.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/sequence/parse.rs crates/mermaid/src/sequence/mod.rs
git commit -m "feat(mermaid): sequence parser — activations, notes, fragments

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: two-pass lifeline layout (`sequence/layout.rs`)

**Files:**
- Create: `crates/mermaid/src/sequence/layout.rs`
- Modify: `crates/mermaid/src/sequence/mod.rs` (uncomment `pub(crate) mod layout;`)

**Interfaces:**
- Consumes: `SeqDiagram`/`Event`/`NotePlacement`/`FragmentKind` (Task 2), `crate::measure::{text_size, LINE_H}`.
- Produces:

```rust
pub(crate) struct MsgLayout {
    pub event: usize,        // index into SeqDiagram.events
    pub y: f64,              // the message line's y
    pub text_anchor: (f64, f64), // centered above the line (or right of a self-loop)
    pub number: Option<u32>, // autonumber, if active
}
pub(crate) struct NoteLayout { pub event: usize, pub rect: crate::layout::Rect }
pub(crate) struct ActRect { pub p: usize, pub depth: usize, pub y0: f64, pub y1: f64 }
pub(crate) struct FrameRect {
    pub kind: crate::sequence::FragmentKind,
    pub label: String,
    pub rect: crate::layout::Rect,
    pub depth: usize,
    pub dividers: Vec<(f64, String)>, // y + label
}
pub(crate) struct SeqLayout {
    pub col_x: Vec<f64>,
    pub box_w: Vec<f64>,   // participant box width per column (svg draws with it)
    pub head_h: f64,       // participant box height (uniform)
    pub body_top: f64,
    pub body_bottom: f64,
    pub messages: Vec<MsgLayout>,
    pub notes: Vec<NoteLayout>,
    pub activations: Vec<ActRect>,
    pub frames: Vec<FrameRect>,
    pub size: (f64, f64),
}
pub(crate) fn run(d: &SeqDiagram) -> SeqLayout   // infallible: caps already enforced at parse
```

(`crate::layout::Rect` is reused as a plain geometry type — no other coupling to the graph engine.)

**Constants (write exactly):** `PAD = 20.0`, `COL_GAP_MIN = 110.0`, `BOX_PAD_X = 24.0`, `BOX_PAD_Y = 12.0`, `ACTOR_EXTRA_H = 34.0` (stick figure above the label), `ROW_GAP = 10.0`, `MSG_MIN_H = 24.0`, `SELF_EXTRA = 16.0`, `SELF_STUB = 30.0`, `NOTE_PAD = 8.0`, `FRAME_HEAD = 26.0`, `FRAME_INSET = 8.0`, `FRAME_BOTTOM_PAD = 10.0`, `DIVIDER_H = 22.0`, `ACT_W = 10.0`, `ACT_OFFSET = 6.0`.

**Pass 1 — columns (write exactly this algorithm):**
1. Box width per participant: `text_size(display).0 + BOX_PAD_X*2` (min 60). `head_h` = uniform: `max(text_size(display).1) + BOX_PAD_Y*2` + `ACTOR_EXTRA_H` if ANY participant is an actor (uniform strip keeps lifeline tops aligned).
2. Initial `col_x[0] = PAD + box_w[0]/2`; `col_x[i] = col_x[i-1] + max(COL_GAP_MIN, (box_w[i-1] + box_w[i])/2 + 20)`.
3. Widening: for each Message between distinct columns i<j: `need = text_size(text).0 + 24`; if `col_x[j] - col_x[i] < need`, shift columns `j..` right by the deficit. For each `Note over A,B` (i<j): same with `need = note_w + 12` where `note_w = text_size(text).0 + NOTE_PAD*2`. For self-messages: `need_right = SELF_STUB + text_size(text).0 + 12` — track `overhang_right = max(...)` for the LAST column (and any column, contributing to the gap to its right neighbor: if the self-message is on column i < last, widen gap (i, i+1) to `max(gap, need_right + box protection)` — use the same shift-right mechanic). For `Note left of` col 0: `margin_left = max(note_w + 12)` shifts ALL columns right. For `Note right of` last col / `Note left/right` of interior columns: contribute to the adjacent gap or right overhang analogously. Process widenings in EVENT ORDER (deterministic single pass; shifting right never un-satisfies an earlier pair).
4. Canvas width = `col_x.last() + box_w.last()/2 + overhang_right + PAD`.
5. `body_top = PAD + head_h + 14.0`.

**Pass 2 — rows:** cursor starts at `body_top`; `autonum: Option<u32>` starts None; frame stack `Vec<(FragmentKind, String, f64 /*top*/, usize /*depth*/, Vec<(f64, String)>)>`; per-participant activation stacks `Vec<Vec<(usize /*depth*/, f64 /*y0*/)>>`. Walk events:
- `Autonumber` → `autonum = Some(1)`.
- `Message` → `text_h = if text.is_empty() { 0.0 } else { text_size(text).1 }`; `row_h = MSG_MIN_H.max(text_h + 14.0) + if self_msg { SELF_EXTRA } else { 0.0 }`; line y = `cursor + row_h - 6.0`; text anchor: self → `(col_x[from] + ACT_W + SELF_STUB + 6.0, cursor + text_h/2.0 + 6.0)` left-aligned by svg; else → midpoint `( (col_x[from]+col_x[to])/2.0, line_y - 6.0 )`. `number = autonum` (then `autonum = autonum.map(|n| n+1)`). Shorthand: `activate_target` opens a span on `to` at `line_y` (depth = current stack len); `deactivate_source` closes the top span of `from` at `line_y`. Advance `cursor += row_h + ROW_GAP`.
- `Activate { p }` → open span at `cursor` (no advance). `Deactivate { p }` → close top span at `cursor` (no advance). (Parser guarantees balance-validity.)
- `Note` → `note_h = text_size(text).1 + NOTE_PAD*2`; rect x/w per placement (left of: right edge at `col_x[p] - ACT_W`, width note_w; right of: left edge at `col_x[p] + ACT_W`; over: centered on the col (or spanning `col_x[a]..col_x[b]` plus half-widths + NOTE_PAD)); rect y = cursor; advance `cursor += note_h + ROW_GAP`.
- `FragmentOpen` → push (kind, label, cursor, stack depth BEFORE push, vec![]); advance `cursor += FRAME_HEAD`.
- `FragmentDivider` → record `(cursor + 4.0, label)` on the stack top; advance `cursor += DIVIDER_H`.
- `FragmentClose` → pop; frame rect = `Rect { x: PAD/2 + depth*FRAME_INSET, y: top, w: size_w - PAD - 2*depth*FRAME_INSET, h: cursor + 6.0 - top }`; advance `cursor += FRAME_BOTTOM_PAD`; push to `frames`.
- After the walk: force-close any open activation spans at `body_bottom = cursor + 6.0`. `size = (width, body_bottom + head_h + PAD)` (bottom participant boxes mirror the top strip).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::parse::parse;

    fn lay(src: &str) -> SeqLayout {
        run(&parse(src).expect("parse"))
    }

    #[test]
    fn columns_ordered_with_min_gap() {
        let l = lay("sequenceDiagram\nA->>B: x\nB->>C: y");
        assert!(l.col_x[0] < l.col_x[1] && l.col_x[1] < l.col_x[2]);
        assert!(l.col_x[1] - l.col_x[0] >= COL_GAP_MIN - 1e-6);
    }

    #[test]
    fn long_message_widens_its_pair() {
        let short = lay("sequenceDiagram\nA->>B: x");
        let long = lay(&format!("sequenceDiagram\nA->>B: {}", "wide ".repeat(20)));
        assert!(long.col_x[1] - long.col_x[0] > short.col_x[1] - short.col_x[0]);
    }

    #[test]
    fn spanning_note_widens() {
        let base = lay("sequenceDiagram\nA->>B: x");
        let noted = lay(&format!("sequenceDiagram\nA->>B: x\nNote over A,B: {}", "n".repeat(60)));
        assert!(noted.col_x[1] - noted.col_x[0] > base.col_x[1] - base.col_x[0]);
    }

    #[test]
    fn left_note_extends_left_margin() {
        let base = lay("sequenceDiagram\nA->>B: x");
        let noted = lay(&format!("sequenceDiagram\nA->>B: x\nNote left of A: {}", "n".repeat(40)));
        assert!(noted.col_x[0] > base.col_x[0]);
    }

    #[test]
    fn rows_monotonic_and_finite() {
        let l = lay("sequenceDiagram\nA->>B: one\nB-->>A: two\nNote over A: n\nA->>A: self");
        let mut prev = l.body_top;
        for m in &l.messages {
            assert!(m.y > prev - 1e-6, "monotone");
            assert!(m.y.is_finite());
            prev = m.y;
        }
        assert!(l.size.0.is_finite() && l.size.1.is_finite());
        assert!(l.body_bottom > l.body_top);
    }

    #[test]
    fn self_message_taller_than_normal() {
        let normal = lay("sequenceDiagram\nA->>B: x\nA->>B: y");
        let selfy = lay("sequenceDiagram\nA->>A: x\nA->>B: y");
        let dn = normal.messages[1].y - normal.messages[0].y;
        let ds = selfy.messages[1].y - selfy.messages[0].y;
        assert!(ds > dn);
    }

    #[test]
    fn activation_spans_wellformed_and_stacked() {
        let l = lay("sequenceDiagram\nA->>+B: a\nB->>+B: nest\nB-->>-B: unnest\nB-->>-A: done");
        assert_eq!(l.activations.len(), 2);
        for a in &l.activations {
            assert!(a.y1 > a.y0);
        }
        let depths: Vec<usize> = l.activations.iter().map(|a| a.depth).collect();
        assert!(depths.contains(&0) && depths.contains(&1));
    }

    #[test]
    fn unclosed_activation_force_closed_at_bottom() {
        let l = lay("sequenceDiagram\nA->>B: x\nactivate B");
        assert_eq!(l.activations.len(), 1);
        assert!((l.activations[0].y1 - l.body_bottom).abs() < 1e-6);
    }

    #[test]
    fn frames_contain_their_rows_and_nest() {
        let l = lay("sequenceDiagram\nloop outer\nA->>B: one\nalt inner\nB-->>A: two\nelse other\nA-xB: three\nend\nend");
        assert_eq!(l.frames.len(), 2);
        let (inner, outer) = {
            let a = &l.frames[0];
            let b = &l.frames[1];
            if a.depth > b.depth { (a, b) } else { (b, a) }
        };
        assert!(inner.rect.x > outer.rect.x);
        assert!(inner.rect.y > outer.rect.y);
        assert!(inner.rect.y + inner.rect.h < outer.rect.y + outer.rect.h + 1e-6);
        // every message row inside the outer frame's y-range
        for m in &l.messages {
            assert!(m.y > outer.rect.y && m.y < outer.rect.y + outer.rect.h);
        }
        assert_eq!(inner.dividers.len(), 1);
    }

    #[test]
    fn autonumber_numbers_messages_from_activation_point() {
        let l = lay("sequenceDiagram\nA->>B: zero\nautonumber\nA->>B: one\nB-->>A: two");
        assert_eq!(l.messages[0].number, None);
        assert_eq!(l.messages[1].number, Some(1));
        assert_eq!(l.messages[2].number, Some(2));
    }

    #[test]
    fn actor_strip_taller_than_plain() {
        let plain = lay("sequenceDiagram\nA->>B: x");
        let actor = lay("sequenceDiagram\nactor A\nA->>B: x");
        assert!(actor.head_h > plain.head_h);
    }

    #[test]
    fn deterministic() {
        let src = "sequenceDiagram\nloop l\nA->>+B: x\nNote over A,B: n\nB-->>-A: y\nend";
        let a = lay(src);
        let b = lay(src);
        assert_eq!(a.col_x, b.col_x);
        assert_eq!(a.size, b.size);
    }
}
```

- [ ] **Step 2: RED** — module missing; uncomment `pub(crate) mod layout;`.

- [ ] **Step 3: Implement `run` per the two-pass algorithm above.** The algorithm text is normative; the tests are the contract. Structure: `fn pass1_columns(d) -> (col_x, box_w, head_h, overhang_right, margin handled via col_x)` and `fn pass2_rows(d, &cols…) -> (messages, notes, activations, frames, body_bottom)`, assembled by `run`. All iteration over `Vec`s in index order — no HashMap anywhere. No `unwrap()` on user-derived values; activation close paths use `if let Some(...)` (parser guarantees validity, but never panic regardless — a mismatch closes nothing).

- [ ] **Step 4: GREEN** — `cargo test -p ogrenotes-mermaid sequence::layout` (12 tests) + full suite + wasm.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/sequence/layout.rs crates/mermaid/src/sequence/mod.rs
git commit -m "feat(mermaid): two-pass sequence lifeline layout

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: SVG assembly (`sequence/svg.rs`)

**Files:**
- Create: `crates/mermaid/src/sequence/svg.rs`
- Modify: `crates/mermaid/src/sequence/mod.rs` (uncomment `pub(crate) mod svg;`, add `render_sequence`)

**Interfaces:**
- Consumes: `SeqDiagram`, `SeqLayout` (Task 5), `crate::escape_xml`, `crate::measure::lines`.
- Produces:
  - `sequence/mod.rs`: `pub(crate) fn render_sequence(source: &str) -> Result<String, crate::ParseError>` — `parse` → `layout::run` → `svg::emit`.
  - `svg.rs`: `pub(crate) fn emit(d: &SeqDiagram, l: &SeqLayout) -> String`.

**Document structure (NORMATIVE — z-order and exact attribute values):**
1. `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" style="font-family:sans-serif;font-size:14px">` (W/H `{:.0}` from `l.size`).
2. `<defs>` — three markers, all `fill`/`stroke` `currentColor`, `orient="auto-start-reverse"`:
   - `mmd-arrow` — same triangle as flowchart's (`<path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>`, viewBox 0 0 10 10, refX 9, refY 5, markerWidth/Height 7).
   - `mmd-cross` — `<path d="M 1 1 L 9 9 M 9 1 L 1 9" stroke="currentColor" stroke-width="1.5" fill="none"/>`, viewBox 0 0 10 10, refX 5, refY 5, markerWidth/Height 8.
   - `mmd-async` — open half-arrow `<path d="M 0 0 L 10 5 L 0 10" stroke="currentColor" stroke-width="1.5" fill="none"/>`, viewBox 0 0 10 10, refX 9, refY 5, markerWidth/Height 8.
3. Fragment frames, OUTER first (sort by `depth` ascending; stable by index): `<rect>` `fill="none" stroke="currentColor" stroke-width="1" rx="3"`; label tab `<rect>` at the frame's top-left, `fill="var(--mermaid-cluster-fill, #7773)"`, height 20, width = `text_size(kind keyword).0 + 16`; kind keyword `<text>` bold inside the tab; the frame LABEL (if non-empty) as `<text>` right of the tab in square brackets `[label]`; each divider: `<line>` dashed (`stroke-dasharray="4 3"`) across the frame at its y + centered bracketed label.
4. Lifelines: per participant, `<line x1="{col}" y1="{head_bottom}" x2="{col}" y2="{body_bottom}" stroke="currentColor" stroke-dasharray="3 3" stroke-width="1"/>` where `head_bottom = PAD + head_h`.
5. Activation rects: `<rect x="{col - ACT_W/2 + depth*ACT_OFFSET}" y="{y0}" width="{ACT_W}" height="{y1-y0}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor"/>`.
6. Messages: normal → `<line>` from `col_x[from]` to `col_x[to]` at `y` (`stroke="currentColor"`, Dotted adds `stroke-dasharray="4 3"`, head ≠ None adds `marker-end="url(#mmd-{arrow|cross|async})"`); self → `<path d="M {col+ACT_W/2} {y - SELF_EXTRA/2} H {col+SELF_STUB} V {y + SELF_EXTRA/2} H {col+ACT_W/2}" fill="none"` (the loop rides inside the row height layout reserved), same marker-end as a normal message of that head. Message text `<text>` at `text_anchor` (`text-anchor="middle"` for normal, `text-anchor="start"` for self), `fill="currentColor"`, one `<tspan>` per `measure::lines()` line; autonumber → text content prefixed `"{n}. "`.
7. Notes: `<rect>` `fill="var(--mermaid-note-fill, #fff5ad)" stroke="currentColor" rx="2"` + centered `<text fill="#333">`… NO — text must stay theme-safe on the light note fill: use `fill="var(--mermaid-note-text, #333)"`. One tspan per line.
8. Participant boxes TOP and BOTTOM (two passes over participants): `<rect>` `fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor" rx="4"` centered on `col_x[i]`, width `box_w[i]`; label `<text text-anchor="middle" fill="currentColor">`. Top box y = `PAD + (ACTOR_EXTRA if actor strip else 0)`; bottom box y = `body_bottom + 8`. For `is_actor`: instead of a rect, a stick figure above the label — `<circle r="7">` head + `<path>` body/arms/legs (`M x y+7 V y+22 M x-8 y+13 H x+8 M x y+22 L x-7 y+32 M x y+22 L x+7 y+32`), `stroke="currentColor" fill="none"`, label text below.
9. `</svg>`.

**Escaping:** display names, message text, note text, fragment labels — ALL through `crate::escape_xml` (per line after `measure::lines`). Numeric/const everything else. Ids never emitted.

- [ ] **Step 1: Write the failing e2e tests** (in `svg.rs`, driving `render_sequence`)

```rust
#[cfg(test)]
mod tests {
    use crate::sequence::render_sequence;

    #[test]
    fn basic_exchange_renders() {
        let svg = render_sequence("sequenceDiagram\nparticipant A as Alice\nA->>B: Hello\nB-->>A: Hi").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Alice") && svg.contains("Hello"));
        assert!(svg.contains("mmd-arrow"));
        assert!(svg.contains("stroke-dasharray=\"3 3\"")); // lifelines
        // top + bottom participant boxes: each display appears twice
        assert!(svg.matches(">Alice<").count() >= 2);
    }

    #[test]
    fn all_heads_get_their_markers() {
        let svg = render_sequence("sequenceDiagram\nA-xB: c\nA-)B: a\nA->>B: n").unwrap();
        assert!(svg.contains("url(#mmd-cross)"));
        assert!(svg.contains("url(#mmd-async)"));
        assert!(svg.contains("url(#mmd-arrow)"));
    }

    #[test]
    fn open_head_has_no_marker_reference_on_its_line() {
        let svg = render_sequence("sequenceDiagram\nA->B: plain").unwrap();
        assert_eq!(svg.matches("marker-end").count(), 0);
    }

    #[test]
    fn actor_renders_stick_figure() {
        let svg = render_sequence("sequenceDiagram\nactor U as User\nU->>S: go").unwrap();
        assert!(svg.contains("<circle")); // head
    }

    #[test]
    fn note_renders_with_note_fill() {
        let svg = render_sequence("sequenceDiagram\nA->>B: x\nNote over A,B: spanning note").unwrap();
        assert!(svg.contains("--mermaid-note-fill"));
        assert!(svg.contains("spanning note"));
    }

    #[test]
    fn fragment_frame_and_divider() {
        let svg = render_sequence("sequenceDiagram\nalt good\nA->>B: y\nelse bad\nA-xB: n\nend").unwrap();
        assert!(svg.contains(">alt<") || svg.contains(">alt</"));
        assert!(svg.contains("[good]"));
        assert!(svg.contains("[bad]"));
        assert!(svg.contains("stroke-dasharray=\"4 3\"")); // divider
    }

    #[test]
    fn activation_rect_present() {
        let svg = render_sequence("sequenceDiagram\nA->>+B: go\nB-->>-A: done").unwrap();
        assert!(svg.contains("--mermaid-node-fill"));
    }

    #[test]
    fn autonumber_prefixes() {
        let svg = render_sequence("sequenceDiagram\nautonumber\nA->>B: first\nB->>A: second").unwrap();
        assert!(svg.contains("1. first") || svg.contains(">1. "));
        assert!(svg.contains("2. second") || svg.contains(">2. "));
    }

    #[test]
    fn user_strings_escaped() {
        let svg = render_sequence("sequenceDiagram\nparticipant A as <b>Bad</b>\nA->>B: <script>x</script>").unwrap();
        assert!(!svg.contains("<script>"));
        assert!(!svg.contains("<b>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_sequence("sequenceDiagram\nend").is_err());
    }
}
```

- [ ] **Step 2: RED** — uncomment `pub(crate) mod svg;`; add to `sequence/mod.rs`:

```rust
pub(crate) fn render_sequence(source: &str) -> Result<String, crate::ParseError> {
    let d = parse::parse(source)?;
    let l = layout::run(&d);
    Ok(svg::emit(&d, &l))
}
```

- [ ] **Step 3: Implement `svg::emit`** following the 9-part normative structure. Every attribute value listed there is exact; the `...`-free rule applies — no scaffolding may survive the commit. Document z-order with brief section comments matching the structure numbering.

- [ ] **Step 4: GREEN** — `cargo test -p ogrenotes-mermaid sequence::` all pass (parse 27 + layout 12 + svg 10); full suite + wasm.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/sequence/svg.rs crates/mermaid/src/sequence/mod.rs
git commit -m "feat(mermaid): sequence SVG assembly + render pipeline

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: `render()` wiring, property test, cleanup

**Files:**
- Modify: `crates/mermaid/src/lib.rs` (Sequence arm + lib tests + doc comment + adversarial additions)
- Modify: `crates/mermaid/src/sequence/mod.rs` (remove `#![allow(dead_code)]` + TODO; fix anything it masked by deletion)
- Create: `crates/mermaid/src/sequence/props.rs` (+ `#[cfg(test)] mod props;` in `sequence/mod.rs`)

**Interfaces:**
- Consumes: `sequence::render_sequence` (Task 6).
- Produces: public behavior — `render()` on a Sequence source returns SVG.

- [ ] **Step 1: Write the failing lib tests** (append to `lib.rs` tests)

```rust
    #[test]
    fn sequence_renders_svg_via_public_render() {
        let out = render("sequenceDiagram\nAlice->>+Bob: Hello\nBob-->>-Alice: Hi");
        assert_eq!(out.kind, DiagramKind::Sequence);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        assert!(out.svg.expect("sequence should render").starts_with("<svg"));
    }

    #[test]
    fn sequence_parse_error_flows_through_render() {
        let out = render("sequenceDiagram\nloop forever\nA->>B: x");
        assert_eq!(out.kind, DiagramKind::Sequence);
        assert!(out.svg.is_none());
        assert_eq!(out.error.expect("error").line, Some(2));
    }

    #[test]
    fn sequence_with_fragments_and_notes_renders() {
        let src = "sequenceDiagram\nautonumber\nactor U as User\nU->>+S: request\nalt cached\nS-->>U: fast\nelse miss\nS->>D: query\nNote over S,D: slow path\nD-->>S: rows\nend\nS-->>-U: response";
        let out = render(src);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }
```

Also EXTEND the adversarial never-panics list (additions only):

```rust
            "sequenceDiagram",
            "sequenceDiagram\nA->>",
            "sequenceDiagram\n->>B: x",
            "sequenceDiagram\nend\nend",
            &format!("sequenceDiagram\n{}", "loop l\n".repeat(50)),
            &format!("sequenceDiagram\n{}", "A->>B: x\n".repeat(2000)),
            &format!("sequenceDiagram\n{}", (0..60).map(|i| format!("participant p{i}\n")).collect::<String>()),
            "sequenceDiagram\nautonumber\nA->>A: 🎭<br/>🎭\nNote left of A: 🎉",
            "sequenceDiagram\nactivate A",
            "sequenceDiagram\nA-->>-B: under",
```

And update the `each_unsupported_kind_error_names_its_label` cases list: remove the `("sequenceDiagram", …)` entry if present — CHECK first: the current list (post-slice-2 merge) contains state/class/er only, sequence was never in it (it was in the slice-1 original but main's coverage-sweep version may differ). Verify with grep and leave untouched if sequence isn't listed.

- [ ] **Step 2: Wire the arm** in `render()`:

```rust
        DiagramKind::Sequence => match sequence::render_sequence(source) {
            Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
```

State/Class/Er remain in the "not yet supported" arm. Update the crate doc comment to mention pie + flowchart + sequence.

- [ ] **Step 3: Property test** — create `sequence/props.rs`:

```rust
//! Property tests for the sequence pipeline (proptest, dev-only).

use proptest::prelude::*;

fn arb_source() -> impl Strategy<Value = String> {
    // Statement soup: valid-ish lines shuffled together — exercises the
    // parser's error paths and, when parse succeeds, the full pipeline.
    let stmt = prop_oneof![
        Just("A->>B: hi".to_string()),
        Just("B-->>-A: bye".to_string()),
        Just("A->>+B: go".to_string()),
        Just("A->>A: self".to_string()),
        Just("Note over A,B: n".to_string()),
        Just("Note left of A: l".to_string()),
        Just("loop lbl".to_string()),
        Just("alt c".to_string()),
        Just("else o".to_string()),
        Just("par p".to_string()),
        Just("and q".to_string()),
        Just("end".to_string()),
        Just("activate A".to_string()),
        Just("deactivate A".to_string()),
        Just("autonumber".to_string()),
        Just("participant Z as Zed".to_string()),
        "[a-zA-Z<>:\\-x)+ ]{0,24}",
    ];
    proptest::collection::vec(stmt, 0..40).prop_map(|v| {
        format!("sequenceDiagram\n{}", v.join("\n"))
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn render_never_panics_and_xor_holds(src in arb_source()) {
        let out = crate::render(&src);
        prop_assert!(out.svg.is_some() != out.error.is_some());
    }

    #[test]
    fn successful_layouts_are_sane(src in arb_source()) {
        if let Ok(d) = crate::sequence::parse::parse(&src) {
            let l = crate::sequence::layout::run(&d);
            let mut prev = f64::NEG_INFINITY;
            for m in &l.messages {
                prop_assert!(m.y.is_finite() && m.y >= prev);
                prev = m.y;
            }
            prop_assert!(l.size.0.is_finite() && l.size.1.is_finite());
            for f in &l.frames {
                prop_assert!(f.rect.w > 0.0 && f.rect.h > 0.0);
            }
        }
    }
}
```

Declare `#[cfg(test)] mod props;` in `sequence/mod.rs`.

- [ ] **Step 4: Cleanup** — remove `#![allow(dead_code)]` + TODO from `sequence/mod.rs`; DELETE anything it masked (nothing expected — every type is consumed). Full battery:
- `cargo test -p ogrenotes-mermaid` → all green (RED first by checking the new lib tests fail before the arm is wired).
- `cargo clippy -p ogrenotes-mermaid --all-targets` → no NEW warnings.
- `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown` → clean.
- `cargo test -p ogrenotes-collab export::` → green (export picks sequence up automatically).
- `cd frontend && cargo build --target wasm32-unknown-unknown` → clean (nested-worktree quirk: temporary `[workspace]` shim in frontend/Cargo.toml, REVERTED before staging).

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/lib.rs crates/mermaid/src/sequence/mod.rs crates/mermaid/src/sequence/props.rs
git commit -m "feat(mermaid): wire sequence rendering into render()

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Final verification (after Task 7)

- [ ] `cargo test -p ogrenotes-mermaid` — every module green (parse/layout/svg/props + all slice-1/2 suites).
- [ ] `cargo clippy -p ogrenotes-mermaid --all-targets` — no new warnings.
- [ ] `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown` — clean.
- [ ] `cargo test -p ogrenotes-collab --lib` — green.
- [ ] `cd frontend && cargo build --target wasm32-unknown-unknown` — clean.
- [ ] Manual smoke: insert a Mermaid block, paste a sequence diagram with fragments + notes + activations, verify live render + modal preview + HTML export; invalid line shows error + raw source.

## Notes for the implementer

- The plan's example code is a starting point — where it conflicts with the compiler or its own tests, fix the code, never the tests, and record the deviation (slices 1–2 precedent: several plan bugs were caught exactly this way).
- Arrow-table ordering (Task 3) and the frame/divider bookkeeping (Tasks 4–5) are the likeliest plan-bug sites — treat test failures there as evidence, trace by hand.
- Determinism and never-panic are hard requirements throughout; all layout iteration is Vec-indexed.
- Keep `sequence/` fully independent of `crate::layout` except the plain `Rect` geometry type.
