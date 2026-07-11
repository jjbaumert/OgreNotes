# Mermaid Slice 4 — State, Class, ER Diagrams Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `render()` renders `stateDiagram(-v2)`, `classDiagram`, and `erDiagram` sources to SVG via the existing slice-2 layered layout engine — the final slice of the approved mermaid scope.

**Architecture:** Three sibling modules (`state/`, `class/`, `er/`), each `parse.rs` + `svg.rs` + a thin `mod.rs` entry, all feeding `crate::layout::run` through one shared ~40-line adapter (`boxgraph.rs`). Composite states map onto the cluster machinery; class/ER nodes are measured multi-compartment boxes; relationship kinds map to new SVG marker defs. No new layout algorithms.

**Tech Stack:** Pure Rust (std only at runtime), proptest dev-dep (present), wasm32-clean.

## Global Constraints

- Crate stays `#![forbid(unsafe_code)]`, zero runtime dependencies, wasm32-clean; deterministic; `render()` never panics (XOR invariant); UTF-8 slice discipline (ASCII-predicate counting or boundary-safe splits ONLY, commented at each site).
- Errors: 1-based lines, first error wins, out-of-scope statements error NAMING their keyword — never silent partial render.
- Caps: layout engine's `MAX_NODES=400`/`MAX_EDGES=1000`/`MAX_DUMMY_SLOTS=20_000` via the adapter; parsers early-bail past `crate::layout::MAX_EDGES` on relationship pushes (flowchart precedent); `MAX_SOURCE_LEN` gate already in `render()`.
- Every user string reaching SVG through `crate::escape_xml`; ids never emitted.
- Tests immutable EXCEPT the three spec-sanctioned changes (Task 8): retire `each_unsupported_kind_error_names_its_label` + `unsupported_kind_returns_error_with_kind_preserved` (their arm empties), switch the collab export fallback fixture to a permanently-invalid source.
- Flowchart is NOT refactored onto the adapter in this slice.
- NEVER bare `git stash`. No `git add -A`. Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Verify per task: `cargo test -p ogrenotes-mermaid` + `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown`.

## File Structure

```
crates/mermaid/src/
  boxgraph.rs      Task 1: shared adapter → crate::layout
  state/mod.rs     Task 2 model + Task 3 render_state
  state/parse.rs   Task 2
  state/svg.rs     Task 3
  class/mod.rs     Task 4 model + Task 5 render_class
  class/parse.rs   Task 4
  class/svg.rs     Task 5
  er/mod.rs        Task 6 model + Task 7 render_er
  er/parse.rs      Task 6
  er/svg.rs        Task 7
  lib.rs           Task 8: three arms + retirements + props declarations
  state/props.rs, class/props.rs, er/props.rs   Task 8
crates/collab/src/export.rs   Task 8: fixture switch
```

Each family's `mod.rs` carries `#![allow(dead_code)]` + `// TODO(slice4): removed in Task 8` until wired (established scaffolding convention; Task 8 removes all three).

---

### Task 1: shared layout adapter (`boxgraph.rs`)

**Files:**
- Create: `crates/mermaid/src/boxgraph.rs`
- Modify: `crates/mermaid/src/lib.rs` (add `pub(crate) mod boxgraph;`)

**Interfaces:**
- Consumes: `crate::layout::{run, LayoutInput, LNode, LEdge, LCluster, Layout, Direction}`, `crate::ParseError`.
- Produces:

```rust
pub(crate) struct BoxNode {
    pub width: f64,
    pub height: f64,
    pub cluster: Option<usize>,
}
pub(crate) struct BoxEdge {
    pub from: usize,
    pub to: usize,
    /// Reserved (w, h) for a mid-edge label, if any.
    pub label: Option<(f64, f64)>,
}
pub(crate) struct BoxCluster {
    pub parent: Option<usize>,
    pub title: (f64, f64),
}
/// Run the layered engine over a generic box graph. The one shared seam
/// between the slice-4 families and crate::layout; errors (caps etc.)
/// map to line-less ParseErrors, matching flowchart's inline precedent.
pub(crate) fn layout_boxgraph(
    nodes: &[BoxNode],
    edges: &[BoxEdge],
    clusters: &[BoxCluster],
    direction: crate::layout::Direction,
) -> Result<crate::layout::Layout, crate::ParseError>
```

- [ ] **Step 1: Write the failing tests**

Create `crates/mermaid/src/boxgraph.rs`:

```rust
//! Shared adapter: generic measured box-graphs → the slice-2 layered
//! layout engine. Used by the state/class/er families (flowchart
//! predates it and keeps its inline equivalent — no churn this slice).

use crate::layout::{self, Direction, LCluster, LEdge, LNode, Layout};
use crate::ParseError;

#[cfg(test)]
mod tests {
    use super::*;

    fn n(w: f64, h: f64) -> BoxNode {
        BoxNode { width: w, height: h, cluster: None }
    }

    #[test]
    fn simple_chain_lays_out() {
        let nodes = vec![n(80.0, 40.0), n(80.0, 40.0)];
        let edges = vec![BoxEdge { from: 0, to: 1, label: Some((30.0, 14.0)) }];
        let l = layout_boxgraph(&nodes, &edges, &[], Direction::TB).unwrap();
        assert_eq!(l.node_centers.len(), 2);
        assert_eq!(l.edge_paths.len(), 1);
        assert!(l.node_centers[1].1 > l.node_centers[0].1);
        assert!(l.edge_paths[0].label_at.is_some());
    }

    #[test]
    fn clusters_pass_through() {
        let nodes = vec![
            BoxNode { width: 60.0, height: 30.0, cluster: Some(0) },
            BoxNode { width: 60.0, height: 30.0, cluster: Some(0) },
            n(60.0, 30.0),
        ];
        let edges = vec![
            BoxEdge { from: 0, to: 1, label: None },
            BoxEdge { from: 1, to: 2, label: None },
        ];
        let clusters = vec![BoxCluster { parent: None, title: (40.0, 16.0) }];
        let l = layout_boxgraph(&nodes, &edges, &clusters, Direction::TB).unwrap();
        assert_eq!(l.cluster_rects.len(), 1);
    }

    #[test]
    fn over_cap_maps_to_parse_error() {
        let nodes: Vec<BoxNode> =
            (0..=layout::MAX_NODES).map(|_| n(1.0, 1.0)).collect();
        let e = layout_boxgraph(&nodes, &[], &[], Direction::TB).unwrap_err();
        assert!(e.message.contains("too large"));
        assert_eq!(e.line, None);
    }
}
```

- [ ] **Step 2: RED** — `cargo test -p ogrenotes-mermaid boxgraph` fails (types missing). Add `pub(crate) mod boxgraph;` to `lib.rs`.

- [ ] **Step 3: Implement**

```rust
#[derive(Debug, Clone)]
pub(crate) struct BoxNode {
    pub width: f64,
    pub height: f64,
    pub cluster: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct BoxEdge {
    pub from: usize,
    pub to: usize,
    pub label: Option<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub(crate) struct BoxCluster {
    pub parent: Option<usize>,
    pub title: (f64, f64),
}

pub(crate) fn layout_boxgraph(
    nodes: &[BoxNode],
    edges: &[BoxEdge],
    clusters: &[BoxCluster],
    direction: Direction,
) -> Result<Layout, ParseError> {
    let input = layout::LayoutInput {
        nodes: nodes
            .iter()
            .map(|b| LNode { width: b.width, height: b.height, cluster: b.cluster })
            .collect(),
        edges: edges
            .iter()
            .map(|e| LEdge { from: e.from, to: e.to, label: e.label })
            .collect(),
        clusters: clusters
            .iter()
            .map(|c| LCluster { parent: c.parent, title: c.title })
            .collect(),
        direction,
    };
    layout::run(&input).map_err(|message| ParseError { message, line: None })
}
```

(If the compiler objects to unused-in-crate warnings before the families land, this module needs NO allow marker — its own tests consume everything.)

- [ ] **Step 4: GREEN** — 3 tests pass; full suite unchanged otherwise; wasm build clean.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/boxgraph.rs crates/mermaid/src/lib.rs
git commit -m "feat(mermaid): shared box-graph layout adapter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: state diagrams — model + parser

**Files:**
- Create: `crates/mermaid/src/state/mod.rs`, `crates/mermaid/src/state/parse.rs`
- Modify: `crates/mermaid/src/lib.rs` (add `mod state;`)

**Interfaces:**
- Produces (in `state/mod.rs`, all `pub(crate)`):

```rust
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
```

- `state/parse.rs`: `pub(crate) fn parse(source: &str) -> Result<StateGraph, ParseError>`.

**Grammar (write exactly):** header `stateDiagram-v2` or `stateDiagram` (first meaningful line, optional trailing `;`). Statements:
- `A --> B` / `A --> B: label` — transitions; endpoints are ids or `[*]`. `[*]` as SOURCE creates a fresh `Start` node (id `__start_{n}`) in the CURRENT composite scope; as TARGET a fresh `End` node. Ids `[A-Za-z0-9_]+` (ASCII predicate, commented).
- `state "Display text" as A` — declares/updates A's display.
- `state A <<choice>>` / `<<fork>>` / `<<join>>` — kind stereotypes (fork and join both map to `ForkJoin`).
- `state A {` opens a composite (display = id; nested allowed; membership-on-CREATION like flowchart subgraphs); bare `}` closes; unclosed at EOF errors at the OPENING line; stray `}` errors. Composite ids share the node id namespace conceptually but composites are NOT nodes (they become clusters); a transition endpoint naming a composite id is a per-line error ("transitions to composite states are not supported") — v1 simplification.
- `note left of A: text` / `note right of A: text` (single-line; case-insensitive `note`; the block form `note … end note` errors "multi-line notes are not supported"). Note targets must be existing-or-implicitly-created states.
- `%%` comments, blank lines, `;` statement separators.
- Out of scope, error naming keyword: `--` (concurrency, a line consisting of exactly `--`), `[H]`/`[H*]` anywhere an endpoint could be, `direction`.
- Caps: implicit/explicit node creation bails past `layout::MAX_NODES` ("diagram too large"); transition pushes bail past `layout::MAX_EDGES`.

- [ ] **Step 1: Write the failing tests** (in `state/parse.rs`)

```rust
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
}
```

- [ ] **Step 2: RED**, then **Step 3: Implement** the parser following the grammar above and the established `Parser { g, ids: HashMap<String, usize>, composites stack Vec<(usize, usize)>, line }` shape from `flowchart/parse.rs` / `sequence/parse.rs` (read both first; reuse their idioms: `err()` helper, ASCII id scan with byte-safety comment, `split_once(':')` for labels, exact-match guards for `}`/`--`). Composite handling mirrors flowchart's subgraph stack (membership-on-creation, `(index, opening_line)` stack, EOF check). `[*]` handling: match the literal token at endpoint positions before the id scan. Stereotype lines: after `state ID`, an optional `<<word>>` where word ∈ {choice, fork, join} — anything else in `<<…>>` errors naming it.

- [ ] **Step 4: GREEN** — 13 new tests; full suite; wasm build.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/state/mod.rs crates/mermaid/src/state/parse.rs crates/mermaid/src/lib.rs
git commit -m "feat(mermaid): state diagram model and parser

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: state diagrams — SVG + `render_state`

**Files:**
- Create: `crates/mermaid/src/state/svg.rs`
- Modify: `crates/mermaid/src/state/mod.rs` (uncomment `pub(crate) mod svg;`, add `render_state`)

**Interfaces:**
- Consumes: `StateGraph` (Task 2), `crate::boxgraph::layout_boxgraph`, `crate::measure`, `crate::escape_xml`, `crate::layout::{Layout, Direction}`.
- Produces: `pub(crate) fn render_state(source: &str) -> Result<String, ParseError>` in `state/mod.rs`; `pub(crate) fn emit(g: &StateGraph, l: &Layout, sizes: &[(f64, f64)]) -> String` in `svg.rs`.

**Pipeline (`render_state`, write exactly):** parse → size each node: Normal = `text_size(display)` + 24/16 padding (min 60×36, rounded rect); Start/End = 18×18; Choice = diamond footprint `tw*1.7+24 × th*2.2+12` (reuse the flowchart diamond formula values); ForkJoin = 44×10 bar → `BoxNode`s (cluster = composite index); transitions → `BoxEdge`s with label sizes (`text_size + 8/4` pad); composites → `BoxCluster`s (title = `text_size(display)`); `layout_boxgraph(..., Direction::TB)` → `svg::emit`.

**Document structure (NORMATIVE):**
1. Standard `<svg …>` header (same shape as flowchart/sequence emitters).
2. `<defs>` — `mmd-arrow` marker (same def string as the other emitters).
3. Cluster (composite) rects + title strips, parents first — same attribute strings as flowchart's cluster rendering (`fill="var(--mermaid-cluster-fill, #7773)"`… read `flowchart/svg.rs` and match its cluster block exactly).
4. Edges: `<path>` polylines from `EdgePath.points`, `stroke="currentColor" fill="none" marker-end="url(#mmd-arrow)"`; labels at `label_at` with the flowchart-style mask rect.
5. Nodes by kind: Normal → `<rect rx="8">` + label tspans (per `measure::lines`); Start → `<circle r="8" fill="currentColor"/>`; End → outer `<circle r="9" fill="none" stroke="currentColor"/>` + inner `<circle r="5" fill="currentColor"/>`; Choice → diamond `<polygon>` (flowchart geometry) + label; ForkJoin → `<rect>` 44×10 `fill="currentColor"`.
6. Notes: for each `StateNote`, a note box (`fill="var(--mermaid-note-fill, #fff5ad)"`, text `var(--mermaid-note-text, #333)`) placed beside the state's laid-out center: left → right edge at `cx - w/2 - 12`, right → left edge at `cx + w/2 + 12`, vertically centered on `cy`; sized `text_size(text) + 16/12` pad. Post-layout placement may overlap other elements — accepted v1 (documented comment).
7. Close.

- [ ] **Step 1: Write the failing e2e tests** (in `svg.rs`, via `render_state`)

```rust
#[cfg(test)]
mod tests {
    use crate::state::render_state;

    #[test]
    fn basic_machine_renders() {
        let svg = render_state("stateDiagram-v2\n[*] --> Idle\nIdle --> Busy: go\nBusy --> [*]").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Idle") && svg.contains("Busy"));
        assert!(svg.contains(">go<") || svg.contains("go</"));
        assert!(svg.contains("mmd-arrow"));
        // start = filled circle, end = ring (2+ circles total)
        assert!(svg.matches("<circle").count() >= 3);
    }

    #[test]
    fn composite_renders_cluster() {
        let svg = render_state("stateDiagram-v2\nstate Machine {\nA --> B\n}\nC --> A").unwrap();
        assert!(svg.contains("--mermaid-cluster-fill"));
        assert!(svg.contains("Machine"));
    }

    #[test]
    fn choice_renders_diamond() {
        let svg = render_state("stateDiagram-v2\nstate C <<choice>>\nA --> C\nC --> B: yes\nC --> D: no").unwrap();
        assert!(svg.contains("<polygon"));
    }

    #[test]
    fn fork_renders_bar() {
        let svg = render_state("stateDiagram-v2\nstate F <<fork>>\nA --> F\nF --> B\nF --> C").unwrap();
        assert!(svg.contains("fill=\"currentColor\""));
    }

    #[test]
    fn note_renders() {
        let svg = render_state("stateDiagram-v2\nA --> B\nnote right of A: important").unwrap();
        assert!(svg.contains("--mermaid-note-fill"));
        assert!(svg.contains("important"));
    }

    #[test]
    fn labels_escaped() {
        let svg = render_state("stateDiagram-v2\nstate \"<script>x</script>\" as s\ns --> t").unwrap();
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_state("stateDiagram-v2\n}").is_err());
    }
}
```

- [ ] **Step 2: RED**, then **Step 3: Implement** `render_state` + `emit` per the pipeline and normative structure (read `flowchart/svg.rs` first and reuse its idioms — cluster block, mask rects, tspan emission; no `...` may survive).

- [ ] **Step 4: GREEN** — full suite; wasm build.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/state/svg.rs crates/mermaid/src/state/mod.rs
git commit -m "feat(mermaid): state diagram SVG rendering

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: class diagrams — model + parser

**Files:**
- Create: `crates/mermaid/src/class/mod.rs`, `crates/mermaid/src/class/parse.rs`
- Modify: `crates/mermaid/src/lib.rs` (add `mod class;` — NOTE: `class` is not a Rust keyword, plain `mod class;` is legal)

**Interfaces:**
- Produces (in `class/mod.rs`):

```rust
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
```

- `class/parse.rs`: `pub(crate) fn parse(source: &str) -> Result<ClassGraph, ParseError>`.

**Grammar (write exactly):** header `classDiagram` (optional `;`). Statements:
- `class Name` — declare. `class Name {` opens a member block: each line until bare `}` is a member (an `<<annotation>>` line sets the annotation; a line containing `(` is a method; else attribute; stored VERBATIM after trim — visibility chars and `~T~` render literally). Nested `{` inside a block errors.
- `Name : member` — dotted member form, same `(` classification.
- Relationships: `A <op> B ["mult"] : label` where the full form is `A "m1" <op> "m2" B : label`. Operator table (match the OPERATOR by scanning for it as a whitespace-delimited token; direction NORMALIZED so `to` is the marker end):
  - `<|--` → from=B, to=A, Inheritance; `--|>` → from=A, to=B, Inheritance
  - `<|..` → from=B, to=A, Realization; `..|>` → from=A, to=B, Realization
  - `*--` → from=B, to=A, Composition; `--*` → from=A, to=B, Composition
  - `o--` → from=B, to=A, Aggregation; `--o` → from=A, to=B, Aggregation
  - `-->` → from=A, to=B, Association (arrow=true); `<--` → from=B, to=A, Association (arrow=true); `--` → from=A, to=B, Association (arrow=false)
  - `..>` → from=A, to=B, Dependency; `<..` → from=B, to=A, Dependency
  - Multiplicities: quoted strings adjacent to each endpoint (`A "1" *-- "0..*" B`), stored on the ORIGINAL side then swapped along with from/to when normalizing.
- Implicit class creation on relationship/dotted-member reference. Ids `[A-Za-z0-9_]+`.
- Out of scope, error naming keyword: `namespace`, `click`, `callback`, `style`, `cssClass`, `link`, `note` (class-diagram notes are out of scope this slice).
- Caps: class creation past `layout::MAX_NODES` and relation pushes past `layout::MAX_EDGES` bail "diagram too large".

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::class::{ClassGraph, RelKind};

    fn p(src: &str) -> ClassGraph {
        parse(src).expect("parse ok")
    }

    fn rel(g: &ClassGraph, i: usize) -> (&str, &str, RelKind) {
        let r = &g.relations[i];
        (&g.classes[r.from].id, &g.classes[r.to].id, r.kind)
    }

    #[test]
    fn header_required() {
        assert!(parse("classDiagram\nclass A").is_ok());
        assert_eq!(parse("class A").unwrap_err().line, Some(1));
    }

    #[test]
    fn member_block_classification() {
        let g = p("classDiagram\nclass Animal {\n<<abstract>>\n+String name\n-int age\n+speak() String\n#walk(int steps)\n}");
        let a = &g.classes[0];
        assert_eq!(a.annotation.as_deref(), Some("abstract"));
        assert_eq!(a.attributes, vec!["+String name", "-int age"]);
        assert_eq!(a.methods, vec!["+speak() String", "#walk(int steps)"]);
    }

    #[test]
    fn dotted_member_form() {
        let g = p("classDiagram\nDuck : +swim()\nDuck : +String beak");
        assert_eq!(g.classes[0].methods, vec!["+swim()"]);
        assert_eq!(g.classes[0].attributes, vec!["+String beak"]);
    }

    #[test]
    fn all_relationship_kinds_normalized() {
        let cases = [
            ("A <|-- B", ("B", "A", RelKind::Inheritance)),
            ("A --|> B", ("A", "B", RelKind::Inheritance)),
            ("A <|.. B", ("B", "A", RelKind::Realization)),
            ("A *-- B", ("B", "A", RelKind::Composition)),
            ("A --* B", ("A", "B", RelKind::Composition)),
            ("A o-- B", ("B", "A", RelKind::Aggregation)),
            ("A --> B", ("A", "B", RelKind::Association)),
            ("A <-- B", ("B", "A", RelKind::Association)),
            ("A -- B", ("A", "B", RelKind::Association)),
            ("A ..> B", ("A", "B", RelKind::Dependency)),
            ("A <.. B", ("B", "A", RelKind::Dependency)),
        ];
        for (src, want) in cases {
            let g = p(&format!("classDiagram\n{src}"));
            let got = rel(&g, 0);
            assert_eq!(got, want, "for {src}");
        }
    }

    #[test]
    fn plain_association_has_no_arrow() {
        let g = p("classDiagram\nA -- B");
        assert!(!g.relations[0].arrow);
        let g2 = p("classDiagram\nA --> B");
        assert!(g2.relations[0].arrow);
    }

    #[test]
    fn multiplicities_and_label_follow_normalization() {
        let g = p("classDiagram\nCustomer \"1\" --> \"0..*\" Order : places");
        let r = &g.relations[0];
        assert_eq!(g.classes[r.from].id, "Customer");
        assert_eq!(r.m_from.as_deref(), Some("1"));
        assert_eq!(r.m_to.as_deref(), Some("0..*"));
        assert_eq!(r.label.as_deref(), Some("places"));
        // Reversed operator swaps multiplicities too.
        let g2 = p("classDiagram\nOrder \"0..*\" <-- \"1\" Customer");
        let r2 = &g2.relations[0];
        assert_eq!(g2.classes[r2.from].id, "Customer");
        assert_eq!(r2.m_from.as_deref(), Some("1"));
        assert_eq!(r2.m_to.as_deref(), Some("0..*"));
    }

    #[test]
    fn unclosed_member_block_errors_at_opening() {
        let e = parse("classDiagram\nclass A {\n+x int").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("unclosed"));
    }

    #[test]
    fn out_of_scope_statements_error_named() {
        for stmt in ["namespace N {", "click A call x()", "callback A \"cb\"",
                     "style A fill:#f00", "cssClass \"A\" cls", "link A \"url\"",
                     "note for A \"text\""] {
            let src = format!("classDiagram\nclass A\n{stmt}");
            let e = parse(&src).unwrap_err();
            assert_eq!(e.line, Some(3), "for {stmt}");
            let kw = stmt.split_whitespace().next().unwrap();
            assert!(e.message.contains(kw), "names {kw}: {}", e.message);
        }
    }

    #[test]
    fn generics_stored_verbatim() {
        let g = p("classDiagram\nclass Box {\n+items List~T~\n}");
        assert_eq!(g.classes[0].attributes, vec!["+items List~T~"]);
    }

    #[test]
    fn multibyte_no_panic() {
        let _ = parse("classDiagram\nclass Émile\nA\u{2003}--> B : héllo 🎉");
    }

    #[test]
    fn relation_cap_enforced() {
        let mut src = String::from("classDiagram\n");
        for i in 0..=crate::layout::MAX_EDGES {
            src.push_str(&format!("A{} --> B{}\n", i % 100, i % 100));
        }
        assert!(parse(&src).unwrap_err().message.contains("too large"));
    }
}
```

- [ ] **Step 2: RED**, then **Step 3: Implement** following the established parser shape (read `flowchart/parse.rs` first). Operator matching: split the statement on whitespace; scan tokens for the FIRST token that exactly matches an operator from the table (quoted multiplicities are the tokens immediately adjacent to the operator; endpoint ids are the outermost tokens; label after `split_once(':')` on the ORIGINAL statement — do the colon split FIRST so labels containing operators don't confuse the scan). Normalization per the table with multiplicity swap. Member-block state machine mirrors composite handling (opening line tracked; `}` exact-match).

- [ ] **Step 4: GREEN** — 12 new tests; full suite; wasm build.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/class/mod.rs crates/mermaid/src/class/parse.rs crates/mermaid/src/lib.rs
git commit -m "feat(mermaid): class diagram model and parser

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: class diagrams — SVG + `render_class`

**Files:**
- Create: `crates/mermaid/src/class/svg.rs`
- Modify: `crates/mermaid/src/class/mod.rs` (uncomment `pub(crate) mod svg;`, add `render_class`)

**Interfaces:**
- Consumes: `ClassGraph`/`RelKind` (Task 4), `crate::boxgraph::layout_boxgraph`, `crate::measure`, `crate::escape_xml`.
- Produces: `pub(crate) fn render_class(source: &str) -> Result<String, ParseError>`; `pub(crate) fn emit(g: &ClassGraph, l: &crate::layout::Layout, sizes: &[(f64, f64)]) -> String`.

**Sizing (`render_class`, write exactly):** per class, lines = [annotation as `«name»` if present, id] + attributes + methods; box width = `max(text_size(line).0) + 24`, min 80; height = `(1 + annotation + attrs.len() + methods.len()) * (LINE_H + 4) + 16` plus 8 for compartment separators. Edge label sizes from `text_size(label)` + 8/4 when present.

**Document structure (NORMATIVE):**
1. Standard `<svg>` header; `<defs>` with FOUR markers, all `orient="auto-start-reverse"`, `viewBox="0 0 12 12"`, `markerWidth/Height="10"`, `refX="11" refY="6"`:
   - `mmd-tri-hollow`: `<path d="M 1 1 L 11 6 L 1 11 z" fill="var(--surface, #fff)" stroke="currentColor" stroke-width="1"/>`
   - `mmd-diamond-filled`: `<path d="M 1 6 L 6 2 L 11 6 L 6 10 z" fill="currentColor"/>`
   - `mmd-diamond-hollow`: same path, `fill="var(--surface, #fff)" stroke="currentColor" stroke-width="1"/>`
   - `mmd-open`: `<path d="M 2 2 L 11 6 L 2 10" stroke="currentColor" stroke-width="1.2" fill="none"/>`
2. Relations: `<path>` per `EdgePath.points`; dash by kind (`Realization`/`Dependency` → `stroke-dasharray="4 3"`); `marker-end` by kind table: Inheritance/Realization → `mmd-tri-hollow`; Composition → `mmd-diamond-filled`; Aggregation → `mmd-diamond-hollow`; Dependency → `mmd-open`; Association → `mmd-open` if `arrow` else NO marker. Mid labels at `label_at` with mask rect (flowchart idiom). Multiplicities: `<text>` placed at 14px along the path inward from each endpoint, offset 10px perpendicular — compute from the first/last segment direction; both escaped.
3. Class boxes: outer `<rect>` (`fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor" rx="4"`), horizontal separator `<line>`s after the name compartment and between attrs/methods (only when the following compartment is non-empty), text lines as left-aligned `<text x="{box_left + 8}">` per line (name compartment centered + `font-weight="600"`; annotation line centered italic `«…»`), everything escaped.
4. Close.

- [ ] **Step 1: Write the failing e2e tests** (via `render_class`)

```rust
#[cfg(test)]
mod tests {
    use crate::class::render_class;

    #[test]
    fn class_with_members_renders_compartments() {
        let svg = render_class("classDiagram\nclass Animal {\n<<abstract>>\n+String name\n+speak() String\n}").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Animal"));
        assert!(svg.contains("«abstract»"));
        assert!(svg.contains("+String name"));
        assert!(svg.contains("+speak() String"));
        assert!(svg.matches("<line").count() >= 2); // separators
    }

    #[test]
    fn marker_per_kind() {
        let svg = render_class("classDiagram\nA <|-- B\nC <|.. D\nE *-- F\nG o-- H\nI --> J\nK ..> L").unwrap();
        assert!(svg.contains("url(#mmd-tri-hollow)"));
        assert!(svg.contains("url(#mmd-diamond-filled)"));
        assert!(svg.contains("url(#mmd-diamond-hollow)"));
        assert!(svg.contains("url(#mmd-open)"));
        assert!(svg.contains("stroke-dasharray")); // realization + dependency
    }

    #[test]
    fn plain_association_no_marker() {
        let svg = render_class("classDiagram\nA -- B").unwrap();
        assert_eq!(svg.matches("marker-end").count(), 0);
    }

    #[test]
    fn multiplicities_and_label_render() {
        let svg = render_class("classDiagram\nCustomer \"1\" --> \"0..*\" Order : places").unwrap();
        assert!(svg.contains(">1<") || svg.contains(">1</"));
        assert!(svg.contains("0..*"));
        assert!(svg.contains("places"));
    }

    #[test]
    fn members_escaped() {
        let svg = render_class("classDiagram\nclass X {\n+bad <script>alert(1)</script>\n}").unwrap();
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_class("classDiagram\nnamespace N {").is_err());
    }
}
```

- [ ] **Step 2: RED**, then **Step 3: Implement** per the normative structure (read `flowchart/svg.rs` idioms first; no `...` survives). Multiplicity placement helper: given `points`, unit vector of the first (resp. last) segment, position = endpoint + 14·(unit toward interior) + 10·(perpendicular); guard zero-length segments (fall back to the endpoint).

- [ ] **Step 4: GREEN**; full suite; wasm build.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/class/svg.rs crates/mermaid/src/class/mod.rs
git commit -m "feat(mermaid): class diagram SVG rendering

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: ER diagrams — model + parser

**Files:**
- Create: `crates/mermaid/src/er/mod.rs`, `crates/mermaid/src/er/parse.rs`
- Modify: `crates/mermaid/src/lib.rs` (add `mod er;`)

**Interfaces:**
- Produces (in `er/mod.rs`):

```rust
// TODO(slice4): removed in Task 8
#![allow(dead_code)]
pub(crate) mod parse;
// pub(crate) mod svg; // Task 7

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cardinality {
    ExactlyOne,  // ||
    ZeroOrOne,   // |o / o|
    OneOrMore,   // }| / |{
    ZeroOrMore,  // }o / o{
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
```

- `er/parse.rs`: `pub(crate) fn parse(source: &str) -> Result<ErGraph, ParseError>`.

**Grammar (write exactly):** header `erDiagram` (optional `;`). Statements:
- `ENTITY {` opens an attribute block; each line until bare `}` is `type name [PK|FK]` (2 or 3 whitespace-separated tokens; a 3rd token other than PK/FK errors naming it; a trailing quoted comment errors "attribute comments are not supported"; >3 tokens errors). Unclosed at EOF errors at opening line.
- `A <lcard><line><rcard> B : label` — e.g. `CUSTOMER ||--o{ ORDER : places`. The relationship token is ONE whitespace-delimited token; parse it as: leading 2-char left-cardinality symbol ∈ {`||`, `|o`, `o|`, `}|`, `}o`}, then `--` (identifying) or `..` (non-identifying), then trailing 2-char right symbol ∈ {`||`, `o|`, `|o`, `|{`, `o{`, `}|`... use this normalization table mapping SYMBOL → Cardinality regardless of side:
  - `||` → ExactlyOne; `|o`/`o|` → ZeroOrOne; `}|`/`|{` → OneOrMore; `}o`/`o{` → ZeroOrMore
  Anything else errors naming the token. Label after `: ` REQUIRED ("relationship needs a `: label`").
- Implicit entity creation on relationship reference. Ids `[A-Za-z0-9_]+`.
- `%%` comments, blanks. Out of scope, error naming keyword: entity aliases (`ENTITY ["alias"]`), `UNIQUE` key marker (a 3rd token `UK` errors as unsupported naming `UK`).
- Caps: entities past `layout::MAX_NODES`, relations past `layout::MAX_EDGES` → "diagram too large".

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::er::{Cardinality, ErGraph};

    fn p(src: &str) -> ErGraph {
        parse(src).expect("parse ok")
    }

    #[test]
    fn header_required() {
        assert!(parse("erDiagram\nA ||--o{ B : has").is_ok());
        assert_eq!(parse("A ||--o{ B : has").unwrap_err().line, Some(1));
    }

    #[test]
    fn attribute_blocks() {
        let g = p("erDiagram\nCUSTOMER {\nstring name\nint id PK\nint org_id FK\n}");
        let c = &g.entities[0];
        assert_eq!(c.attributes.len(), 3);
        assert_eq!(c.attributes[0].ty, "string");
        assert_eq!(c.attributes[0].name, "name");
        assert_eq!(c.attributes[0].key, None);
        assert_eq!(c.attributes[1].key.as_deref(), Some("PK"));
        assert_eq!(c.attributes[2].key.as_deref(), Some("FK"));
    }

    #[test]
    fn all_cardinality_symbols() {
        let cases = [
            ("A ||--|| B : r", Cardinality::ExactlyOne, Cardinality::ExactlyOne),
            ("A |o--o| B : r", Cardinality::ZeroOrOne, Cardinality::ZeroOrOne),
            ("A }|--|{ B : r", Cardinality::OneOrMore, Cardinality::OneOrMore),
            ("A }o--o{ B : r", Cardinality::ZeroOrMore, Cardinality::ZeroOrMore),
            ("A ||--o{ B : r", Cardinality::ExactlyOne, Cardinality::ZeroOrMore),
        ];
        for (src, want_from, want_to) in cases {
            let g = p(&format!("erDiagram\n{src}"));
            assert_eq!(g.relations[0].card_from, want_from, "for {src}");
            assert_eq!(g.relations[0].card_to, want_to, "for {src}");
        }
    }

    #[test]
    fn identifying_vs_non() {
        assert!(p("erDiagram\nA ||--|| B : r").relations[0].identifying);
        assert!(!p("erDiagram\nA ||..|| B : r").relations[0].identifying);
    }

    #[test]
    fn label_required() {
        let e = parse("erDiagram\nA ||--o{ B").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("label"));
    }

    #[test]
    fn bad_cardinality_token_errors() {
        let e = parse("erDiagram\nA xx--oo B : r").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn attribute_comment_errors() {
        let e = parse("erDiagram\nA {\nstring name \"the name\"\n}").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("comment"));
    }

    #[test]
    fn unique_key_errors_named() {
        let e = parse("erDiagram\nA {\nint code UK\n}").unwrap_err();
        assert!(e.message.contains("UK"));
    }

    #[test]
    fn unclosed_block_errors_at_opening() {
        let e = parse("erDiagram\nA {\nstring x").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn multibyte_no_panic() {
        let _ = parse("erDiagram\nA ||--o{ B : héllo\u{2003}🎉\nÉ {\n}");
    }
}
```

- [ ] **Step 2: RED**, then **Step 3: Implement** per the grammar (established parser shape; the relationship token parse: `token.len() >= 6`, guard `is_char_boundary` not needed if you validate the token is ASCII first — check `token.is_ascii()` up front and error otherwise, then slice freely with a comment).

- [ ] **Step 4: GREEN** — 11 new tests; full suite; wasm.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/er/mod.rs crates/mermaid/src/er/parse.rs crates/mermaid/src/lib.rs
git commit -m "feat(mermaid): ER diagram model and parser

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: ER diagrams — SVG + `render_er`

**Files:**
- Create: `crates/mermaid/src/er/svg.rs`
- Modify: `crates/mermaid/src/er/mod.rs` (uncomment `pub(crate) mod svg;`, add `render_er`)

**Interfaces:**
- Consumes: `ErGraph`/`Cardinality` (Task 6), `crate::boxgraph::layout_boxgraph`, `crate::measure`, `crate::escape_xml`.
- Produces: `pub(crate) fn render_er(source: &str) -> Result<String, ParseError>`; `pub(crate) fn emit(g: &ErGraph, l: &crate::layout::Layout, sizes: &[(f64, f64)]) -> String`.

**Sizing:** entity box = title row + one row per attribute; width = `max(text_size(id).0, max over attrs of (type_w + name_w + key_w + 3*12 column gaps)) + 24`, min 100; height = `(1 + attrs.len()) * (LINE_H + 6) + 10`. Relationship label sizes reserved as edge labels.

**Document structure (NORMATIVE):**
1. Standard header; `<defs>` with FOUR crow's-foot markers, each `viewBox="0 0 14 14"`, `markerWidth/Height="12"`, `refX="13" refY="7"`, `orient="auto-start-reverse"`, `stroke="currentColor" fill="none" stroke-width="1.2"`:
   - `mmd-er-one` (ExactlyOne): two bars `<path d="M 9 2 V 12 M 12 2 V 12"/>`
   - `mmd-er-zeroone` (ZeroOrOne): circle + bar `<path d="M 12 2 V 12"/><circle cx="6" cy="7" r="3"/>`
   - `mmd-er-many` (OneOrMore): crow + bar `<path d="M 13 2 L 6 7 L 13 12 M 4 2 V 12"/>`
   - `mmd-er-zeromany` (ZeroOrMore): crow + circle `<path d="M 13 2 L 6 7 L 13 12"/><circle cx="3.5" cy="7" r="3"/>`
2. Relations: `<path>` per `EdgePath.points`, `marker-start="url(#mmd-er-{card_from})" marker-end="url(#mmd-er-{card_to})"`, `stroke-dasharray="4 3"` when NOT identifying; label at `label_at` with mask rect.
3. Entities: outer `<rect fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor"/>`; title row centered bold with a separator `<line>` under it; attribute rows as three left-aligned `<text>` columns (type at `left+8`, name at `left+8+type_col_w`, key at `left+8+type_col_w+name_col_w` — column widths = per-entity max of each field width + 12); alternating row tint optional — NOT included (keep v1 flat).
4. Close. All user strings (ids ARE displayed for entities — they're the title; attribute type/name/key strings) escaped.

- [ ] **Step 1: Write the failing e2e tests** (via `render_er`)

```rust
#[cfg(test)]
mod tests {
    use crate::er::render_er;

    #[test]
    fn entities_and_relationship_render() {
        let svg = render_er("erDiagram\nCUSTOMER ||--o{ ORDER : places\nCUSTOMER {\nstring name\nint id PK\n}").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("CUSTOMER") && svg.contains("ORDER"));
        assert!(svg.contains("places"));
        assert!(svg.contains("url(#mmd-er-one)"));
        assert!(svg.contains("url(#mmd-er-zeromany)"));
        assert!(svg.contains(">PK<") || svg.contains("PK</"));
    }

    #[test]
    fn all_four_markers_exist_in_defs() {
        let svg = render_er("erDiagram\nA ||--|| B : x\nC |o--}| D : y\nE }o--o{ F : z").unwrap();
        for m in ["mmd-er-one", "mmd-er-zeroone", "mmd-er-many", "mmd-er-zeromany"] {
            assert!(svg.contains(&format!("id=\"{m}\"")), "{m} defined");
        }
    }

    #[test]
    fn non_identifying_dashed() {
        let svg = render_er("erDiagram\nA ||..|| B : weak").unwrap();
        assert!(svg.contains("stroke-dasharray"));
        let solid = render_er("erDiagram\nA ||--|| B : strong").unwrap();
        // markers use stroke but relation paths in the solid case carry no dasharray
        assert!(!solid.contains("<path stroke-dasharray"));
    }

    #[test]
    fn attributes_escaped() {
        let svg = render_er("erDiagram\nA {\nstring bad<script>x</script>\n}").unwrap();
        assert!(!svg.contains("<script>"));
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_er("erDiagram\nA {\nint code UK\n}").is_err());
    }
}
```

NOTE on the escaping test: `bad<script>x</script>` as an attribute NAME contains `<`/`>` which the id-charset… attribute type/name tokens are whitespace-split free text, NOT id-validated — confirm the parser stores them verbatim (they are; only ENTITY ids are charset-checked). If the parser errors on this input instead, adjust the fixture to put the script tag where the grammar accepts free text and keep the escaping assertion — do NOT weaken the assertion.

- [ ] **Step 2: RED**, then **Step 3: Implement** per the normative structure. Marker-start orientation relies on `orient="auto-start-reverse"` (already the crate convention).

- [ ] **Step 4: GREEN**; full suite; wasm.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/er/svg.rs crates/mermaid/src/er/mod.rs
git commit -m "feat(mermaid): ER diagram SVG rendering

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: `render()` wiring, sanctioned retirements, properties, cleanup

**Files:**
- Modify: `crates/mermaid/src/lib.rs` (three arms + lib tests + adversarial additions + doc; retire two tests)
- Modify: `crates/collab/src/export.rs` (fallback fixture switch)
- Create: `crates/mermaid/src/state/props.rs`, `crates/mermaid/src/class/props.rs`, `crates/mermaid/src/er/props.rs` (+ `#[cfg(test)] mod props;` in each family mod.rs)
- Modify: all three family `mod.rs` (remove `#![allow(dead_code)]` markers; delete masked dead code; field-scoped exceptions need controller sign-off)

**Interfaces:** public behavior — `render()` covers all six diagram kinds; only `Unknown` errors.

- [ ] **Step 1: Lib tests (RED first)**

```rust
    #[test]
    fn state_renders_svg_via_public_render() {
        let out = render("stateDiagram-v2\n[*] --> A\nA --> [*]");
        assert_eq!(out.kind, DiagramKind::State);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        assert!(out.svg.unwrap().starts_with("<svg"));
    }

    #[test]
    fn class_renders_svg_via_public_render() {
        let out = render("classDiagram\nAnimal <|-- Dog");
        assert_eq!(out.kind, DiagramKind::Class);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }

    #[test]
    fn er_renders_svg_via_public_render() {
        let out = render("erDiagram\nA ||--o{ B : has");
        assert_eq!(out.kind, DiagramKind::Er);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }

    #[test]
    fn unknown_kind_still_errors() {
        let out = render("total gibberish");
        assert_eq!(out.kind, DiagramKind::Unknown);
        assert!(out.svg.is_none() && out.error.is_some());
    }

    #[test]
    fn family_parse_errors_flow_through_render() {
        for (src, line) in [
            ("stateDiagram-v2\n}", 2),
            ("classDiagram\nnamespace N {", 2),
            ("erDiagram\nA ||--o{ B", 2),
        ] {
            let out = render(src);
            assert!(out.svg.is_none(), "for {src:?}");
            assert_eq!(out.error.expect("err").line, Some(line), "for {src:?}");
        }
    }
```

Adversarial additions (EXTEND only): `"stateDiagram-v2"`, `"stateDiagram-v2\n[*] --> [*]"`, `"classDiagram\nclass A {"`, `"classDiagram\nA <|-- A"`, `"erDiagram\nA ||--o{ A : self"`, `&format!("stateDiagram-v2\n{}", "state s {\n".repeat(30))`, `&format!("classDiagram\n{}", "A --> B\n".repeat(2000))`, `"erDiagram\nÉ ||--|| 中 : 🎉"`.

- [ ] **Step 2: Wire the three arms** (replace the `other =>` arm's coverage — the match now handles every kind explicitly; `Unknown` keeps its arm; the generic "not yet supported" arm is DELETED). Update the crate doc to name all six kinds.

- [ ] **Step 3: Sanctioned test changes** (each with a rationale comment citing `docs/superpowers/specs/2026-07-10-mermaid-slice4-state-class-er-design.md`):
1. DELETE `each_unsupported_kind_error_names_its_label` (lib.rs) — its arm has no variants left.
2. DELETE `unsupported_kind_returns_error_with_kind_preserved` (lib.rs) — same arm.
3. `crates/collab/src/export.rs::mermaid_html_falls_back_to_raw_source_on_error`: switch the fixture from `classDiagram…` to a permanently-invalid source (`"not a diagram at all"`), assertions structurally unchanged (raw source escaped + present, no `<svg`, `mermaid-error` class present). This ends the per-slice fixture churn — the test's purpose is "any render error falls back".
No OTHER existing test may change.

- [ ] **Step 4: Property tests** — three files, same statement-soup shape as `sequence/props.rs` (read it first), one strategy per family (state: transitions/[*]/state-decls/composite open/close/notes soup; class: class decls/member blocks/all 11 operators/dotted members; er: entity blocks/relationship tokens/attribute rows), 256 cases each asserting the XOR invariant through `crate::render`, plus a families-specific sanity property (state: successful parses have `nodes.len() >= transitions' max endpoint + 1`; class: relations' indices in bounds; er: same).

- [ ] **Step 5: Cleanup + full battery**
- Remove all three `#![allow(dead_code)]` markers; fix masked warnings by DELETION (field-scoped exceptions → report DONE_WITH_CONCERNS for controller sign-off).
- `cargo test -p ogrenotes-mermaid` (all green incl. 3×256 + existing 512 property cases)
- `cargo clippy -p ogrenotes-mermaid --all-targets` (no NEW warnings)
- `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown`
- `cargo test -p ogrenotes-collab export::` and `--lib`
- `cd frontend && cargo build --target wasm32-unknown-unknown` (nested-worktree `[workspace]` shim, REVERTED before staging)

- [ ] **Step 6: Commit**

```bash
git add crates/mermaid/src/lib.rs crates/mermaid/src/state/mod.rs crates/mermaid/src/state/props.rs crates/mermaid/src/class/mod.rs crates/mermaid/src/class/props.rs crates/mermaid/src/er/mod.rs crates/mermaid/src/er/props.rs crates/collab/src/export.rs
git commit -m "feat(mermaid): wire state/class/er into render(); retire emptied-arm tests

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Final verification (after Task 8)

- [ ] `cargo test -p ogrenotes-mermaid` — every module green.
- [ ] `cargo clippy -p ogrenotes-mermaid --all-targets` — no new warnings.
- [ ] `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown` — clean.
- [ ] `cargo test -p ogrenotes-collab --lib` — green.
- [ ] `cd frontend && cargo build --target wasm32-unknown-unknown` — clean.
- [ ] Manual smoke: one diagram of each of the six kinds through the app (insert block → paste → live render → export).

## Notes for the implementer

- The plan's example code is a starting point — where it conflicts with the compiler or its own tests, fix the code, never the tests, record every deviation (slices 1–3 precedent: multiple plan bugs were caught exactly this way; the reviewers verify deviations independently).
- The relationship-operator scan (Task 4) and the ER relationship-token parse (Task 6) are the likeliest plan-bug sites; the cardinality/operator tables in the tests are the contract.
- Read the sibling family (`flowchart/`, `sequence/`) before writing any parser or emitter — idioms, not just interfaces, must match.
- Determinism, never-panic, UTF-8 discipline, and the escaping table are hard requirements in every task.
