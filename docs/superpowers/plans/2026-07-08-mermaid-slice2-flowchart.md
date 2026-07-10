# Mermaid Slice 2 — Flowchart + Layered Layout Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ogrenotes_mermaid::render()` renders `graph`/`flowchart` sources to SVG via a new mermaid-agnostic layered layout engine, riding slice 1's pipeline with zero schema/frontend/export changes.

**Architecture:** Two independent modules in `crates/mermaid`: `layout/` (abstract digraph in → coordinates/polylines/cluster-rects out; Sugiyama stages one file each; clusters via recursive collapse-expand; all stages TB-only with direction applied as a final transform) and `flowchart/` (parser → measurement → SVG emission). `lib.rs`'s `Flowchart` arm chains parse → measure → layout → emit.

**Tech Stack:** Pure Rust (std only at runtime), `proptest` as dev-dependency, wasm32-clean.

## Global Constraints

- `crates/mermaid` stays `#![forbid(unsafe_code)]`, **zero runtime dependencies**, compiles to `wasm32-unknown-unknown`; `license.workspace = true`.
- `render()` never panics; exactly one of `svg`/`error` set (existing XOR invariant test must stay green).
- Layout safety caps: `MAX_NODES = 400`, `MAX_EDGES = 1000`; over-cap → `ParseError` "diagram too large", never unbounded CPU. All sweep loops have fixed iteration caps.
- Layout is deterministic: stable sorts, index tiebreaks, no randomness.
- Every user string interpolated into SVG goes through the existing `crate::pie::escape_xml` (promote it to `crate::escape_xml` in Task 9 — one definition, both callers).
- Unknown/unsupported statements (`click`, `linkStyle`, `style`, `accTitle`, `accDescr`, `direction` inside a subgraph) are per-line `ParseError`s — no silent partial render.
- Tests immutable: never weaken slice-1 assertions; additions only.
- No `git add -A`; stage explicit paths. Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Verify per task: `cargo test -p ogrenotes-mermaid` and (cheap) `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown`.

## File Structure

```
crates/mermaid/src/
  lib.rs               modify: mod layout; mod flowchart; pub escape_xml; Flowchart arm
  pie.rs               modify: use crate::escape_xml (re-export shim)
  layout/mod.rs        types (LayoutInput, LNode, LEdge, LCluster, Direction, Layout,
                       EdgePath, Rect), caps, run() orchestration, direction transform
  layout/acyclic.rs    cycle breaking (DFS back-edge reversal, self-loop extraction)
  layout/rank.rs       longest-path ranking + source tightening
  layout/order.rs      dummy chains, crossing count, barycenter sweeps
  layout/position.rs   median x-assignment + separation enforcement
  layout/route.rs      polyline assembly + rounded corners + self-loops
  layout/cluster.rs    recursive collapse-expand driver
  flowchart/mod.rs     FlowGraph model types, render pipeline entry
  flowchart/parse.rs   statement parser
  flowchart/measure.rs char-class text measurement
  flowchart/shapes.rs  13 shape emitters + per-shape sizing
  flowchart/svg.rs     document assembly: defs/markers, edges, nodes, clusters, classes
crates/mermaid/Cargo.toml        add [dev-dependencies] proptest
crates/mermaid/tests/layout_props.rs   property tests (dev-only)
```

Tasks 1–8 build `layout/` bottom-up (each stage independently tested); 9–13 build `flowchart/`; 14 integrates and adds end-to-end structural tests.

---

### Task 1: layout types, caps, direction transform

**Files:**
- Create: `crates/mermaid/src/layout/mod.rs`
- Modify: `crates/mermaid/src/lib.rs` (add `mod layout;` — keep `pub(crate)` visibility)

**Interfaces:**
- Produces (all `pub(crate)`, consumed by every later task):
  - `enum Direction { TB, BT, LR, RL }`
  - `struct LNode { pub width: f64, pub height: f64, pub cluster: Option<usize> }`
  - `struct LEdge { pub from: usize, pub to: usize, pub label: Option<(f64, f64)> }`
  - `struct LCluster { pub parent: Option<usize>, pub title: (f64, f64) }`
  - `struct LayoutInput { pub nodes: Vec<LNode>, pub edges: Vec<LEdge>, pub clusters: Vec<LCluster>, pub direction: Direction }`
  - `struct Rect { pub x: f64, pub y: f64, pub w: f64, pub h: f64 }`
  - `struct EdgePath { pub edge: usize, pub points: Vec<(f64, f64)>, pub label_at: Option<(f64, f64)>, pub reversed: bool }`
  - `struct Layout { pub node_centers: Vec<(f64, f64)>, pub edge_paths: Vec<EdgePath>, pub cluster_rects: Vec<Rect>, pub size: (f64, f64) }`
  - `const MAX_NODES: usize = 400; const MAX_EDGES: usize = 1000;`
  - `const NODE_GAP_X: f64 = 40.0; const RANK_GAP_Y: f64 = 50.0;`
  - `fn validate(input: &LayoutInput) -> Result<(), String>` (caps + edge/cluster index bounds + cluster-parent cycle check)
  - `fn apply_direction(layout: &mut Layout, dir: Direction)`

- [ ] **Step 1: Write the failing tests**

Create `crates/mermaid/src/layout/mod.rs` containing only the test module first:

```rust
//! Mermaid-agnostic layered (Sugiyama-style) layout engine.
//! Input: abstract digraph with sized nodes, optional cluster tree, direction.
//! Output: node centers, edge polylines, cluster rects. Deterministic,
//! never panics, all stages operate in TB space (direction is a final
//! coordinate transform). See the slice-2 design spec.

#[cfg(test)]
mod tests {
    use super::*;

    fn node(w: f64, h: f64) -> LNode {
        LNode { width: w, height: h, cluster: None }
    }

    #[test]
    fn validate_accepts_small_graph() {
        let input = LayoutInput {
            nodes: vec![node(10.0, 10.0), node(10.0, 10.0)],
            edges: vec![LEdge { from: 0, to: 1, label: None }],
            clusters: vec![],
            direction: Direction::TB,
        };
        assert!(validate(&input).is_ok());
    }

    #[test]
    fn validate_rejects_over_cap_nodes() {
        let input = LayoutInput {
            nodes: (0..=MAX_NODES).map(|_| node(1.0, 1.0)).collect(),
            edges: vec![],
            clusters: vec![],
            direction: Direction::TB,
        };
        let err = validate(&input).unwrap_err();
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn validate_rejects_out_of_bounds_edge() {
        let input = LayoutInput {
            nodes: vec![node(1.0, 1.0)],
            edges: vec![LEdge { from: 0, to: 7, label: None }],
            clusters: vec![],
            direction: Direction::TB,
        };
        assert!(validate(&input).is_err());
    }

    #[test]
    fn validate_rejects_cluster_parent_cycle() {
        let input = LayoutInput {
            nodes: vec![],
            edges: vec![],
            clusters: vec![
                LCluster { parent: Some(1), title: (0.0, 0.0) },
                LCluster { parent: Some(0), title: (0.0, 0.0) },
            ],
            direction: Direction::TB,
        };
        assert!(validate(&input).is_err());
    }

    fn one_point_layout() -> Layout {
        Layout {
            node_centers: vec![(10.0, 20.0)],
            edge_paths: vec![],
            cluster_rects: vec![Rect { x: 5.0, y: 10.0, w: 10.0, h: 20.0 }],
            size: (100.0, 200.0),
        }
    }

    #[test]
    fn direction_tb_is_identity() {
        let mut l = one_point_layout();
        apply_direction(&mut l, Direction::TB);
        assert_eq!(l.node_centers[0], (10.0, 20.0));
        assert_eq!(l.size, (100.0, 200.0));
    }

    #[test]
    fn direction_bt_flips_y() {
        let mut l = one_point_layout();
        apply_direction(&mut l, Direction::BT);
        assert_eq!(l.node_centers[0], (10.0, 180.0)); // 200 - 20
        assert_eq!(l.size, (100.0, 200.0));
        // rect y flips around its own extent: y' = H - y - h
        assert_eq!(l.cluster_rects[0].y, 200.0 - 10.0 - 20.0);
    }

    #[test]
    fn direction_lr_swaps_axes() {
        let mut l = one_point_layout();
        apply_direction(&mut l, Direction::LR);
        assert_eq!(l.node_centers[0], (20.0, 10.0));
        assert_eq!(l.size, (200.0, 100.0));
        let r = &l.cluster_rects[0];
        assert_eq!((r.x, r.y, r.w, r.h), (10.0, 5.0, 20.0, 10.0));
    }

    #[test]
    fn direction_rl_swaps_then_flips_x() {
        let mut l = one_point_layout();
        apply_direction(&mut l, Direction::RL);
        // after swap: (20,10) in 200x100; flip x: 200-20 = 180
        assert_eq!(l.node_centers[0], (180.0, 10.0));
        assert_eq!(l.size, (200.0, 100.0));
    }
}
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p ogrenotes-mermaid layout::` (after adding `mod layout;` to `lib.rs` below the existing `mod pie;`)
Expected: FAIL — types not defined.

- [ ] **Step 3: Implement types + validate + apply_direction**

Prepend to `crates/mermaid/src/layout/mod.rs` (above the tests):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    TB,
    BT,
    LR,
    RL,
}

#[derive(Debug, Clone)]
pub(crate) struct LNode {
    pub width: f64,
    pub height: f64,
    pub cluster: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct LEdge {
    pub from: usize,
    pub to: usize,
    /// Reserved (w, h) for an edge label, if any.
    pub label: Option<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub(crate) struct LCluster {
    pub parent: Option<usize>,
    /// Title strip size the renderer needs above the content box.
    pub title: (f64, f64),
}

#[derive(Debug, Clone)]
pub(crate) struct LayoutInput {
    pub nodes: Vec<LNode>,
    pub edges: Vec<LEdge>,
    pub clusters: Vec<LCluster>,
    pub direction: Direction,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct EdgePath {
    /// Index into `LayoutInput::edges`.
    pub edge: usize,
    pub points: Vec<(f64, f64)>,
    pub label_at: Option<(f64, f64)>,
    /// True when the edge was reversed for layout; the renderer draws
    /// the arrowhead at the true head (points are already in true order).
    pub reversed: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Layout {
    pub node_centers: Vec<(f64, f64)>,
    pub edge_paths: Vec<EdgePath>,
    pub cluster_rects: Vec<Rect>,
    pub size: (f64, f64),
}

/// Hard caps: layout runs server-side on export from untrusted document
/// content; a pathological diagram must fail fast, not burn CPU.
pub(crate) const MAX_NODES: usize = 400;
pub(crate) const MAX_EDGES: usize = 1000;
/// Minimum horizontal gap between sibling nodes and vertical gap
/// between ranks, in SVG units.
pub(crate) const NODE_GAP_X: f64 = 40.0;
pub(crate) const RANK_GAP_Y: f64 = 50.0;

pub(crate) fn validate(input: &LayoutInput) -> Result<(), String> {
    if input.nodes.len() > MAX_NODES {
        return Err(format!(
            "diagram too large: {} nodes (max {MAX_NODES})",
            input.nodes.len()
        ));
    }
    if input.edges.len() > MAX_EDGES {
        return Err(format!(
            "diagram too large: {} edges (max {MAX_EDGES})",
            input.edges.len()
        ));
    }
    for e in &input.edges {
        if e.from >= input.nodes.len() || e.to >= input.nodes.len() {
            return Err("edge references unknown node".to_string());
        }
    }
    for n in &input.nodes {
        if let Some(c) = n.cluster {
            if c >= input.clusters.len() {
                return Err("node references unknown cluster".to_string());
            }
        }
    }
    // Cluster parent links must form a forest (no cycles, valid indices).
    for (i, c) in input.clusters.iter().enumerate() {
        let mut seen = 0usize;
        let mut cur = c.parent;
        while let Some(p) = cur {
            if p >= input.clusters.len() {
                return Err("cluster references unknown parent".to_string());
            }
            seen += 1;
            if seen > input.clusters.len() {
                return Err(format!("cluster {i} parent chain forms a cycle"));
            }
            cur = input.clusters[p].parent;
        }
    }
    Ok(())
}

/// All stages lay out top-to-bottom. This is the only place direction
/// exists: BT flips y, LR swaps axes, RL swaps then flips x.
pub(crate) fn apply_direction(layout: &mut Layout, dir: Direction) {
    let (w, h) = layout.size;
    let flip_y = |p: (f64, f64)| (p.0, h - p.1);
    let swap = |p: (f64, f64)| (p.1, p.0);
    match dir {
        Direction::TB => {}
        Direction::BT => {
            for p in &mut layout.node_centers {
                *p = flip_y(*p);
            }
            for ep in &mut layout.edge_paths {
                for p in &mut ep.points {
                    *p = flip_y(*p);
                }
                if let Some(l) = &mut ep.label_at {
                    *l = flip_y(*l);
                }
            }
            for r in &mut layout.cluster_rects {
                r.y = h - r.y - r.h;
            }
        }
        Direction::LR | Direction::RL => {
            for p in &mut layout.node_centers {
                *p = swap(*p);
            }
            for ep in &mut layout.edge_paths {
                for p in &mut ep.points {
                    *p = swap(*p);
                }
                if let Some(l) = &mut ep.label_at {
                    *l = swap(*l);
                }
            }
            for r in &mut layout.cluster_rects {
                *r = Rect { x: r.y, y: r.x, w: r.h, h: r.w };
            }
            layout.size = (h, w);
            if dir == Direction::RL {
                let new_w = layout.size.0;
                for p in &mut layout.node_centers {
                    p.0 = new_w - p.0;
                }
                for ep in &mut layout.edge_paths {
                    for p in &mut ep.points {
                        p.0 = new_w - p.0;
                    }
                    if let Some(l) = &mut ep.label_at {
                        l.0 = new_w - l.0;
                    }
                }
                for r in &mut layout.cluster_rects {
                    r.x = new_w - r.x - r.w;
                }
            }
        }
    }
}
```

Add `mod layout;` to `crates/mermaid/src/lib.rs` (after `mod pie;`). Expect `dead_code` warnings until later tasks consume the types — silence with `#![allow(dead_code)]` at the TOP of `layout/mod.rs` with a comment `// TODO(slice2): remove once flowchart consumes layout (Task 14 removes this)`. Task 14 removes it.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p ogrenotes-mermaid layout::`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/layout/mod.rs crates/mermaid/src/lib.rs
git commit -m "feat(mermaid): layout module types, caps, direction transform

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: cycle breaking (`acyclic.rs`)

**Files:**
- Create: `crates/mermaid/src/layout/acyclic.rs`
- Modify: `crates/mermaid/src/layout/mod.rs` (add `pub(crate) mod acyclic;` — declare submodules in mod.rs as plain `mod` lines; all layout submodules follow this pattern)

**Interfaces:**
- Consumes: `LEdge` from Task 1.
- Produces:
  - `pub(crate) struct AcyclicResult { pub edges: Vec<LEdge>, pub reversed: Vec<bool>, pub self_loops: Vec<usize> }` — `edges` has self-loops REMOVED and back-edges flipped (from/to swapped); `reversed[i]` / `self_loops` refer to ORIGINAL edge indices; `edges` keeps a parallel `pub(crate) orig: Vec<usize>` mapping each surviving edge to its original index.
  - `pub(crate) fn make_acyclic(node_count: usize, edges: &[LEdge]) -> AcyclicResult`

- [ ] **Step 1: Write the failing tests**

Create `crates/mermaid/src/layout/acyclic.rs`:

```rust
//! Stage 1: break cycles by reversing DFS back-edges; extract self-loops
//! (they carry no ranking information and are routed specially).

use super::LEdge;

#[cfg(test)]
mod tests {
    use super::*;

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    /// Kahn's-algorithm check: the surviving edge set admits a topological order.
    fn is_acyclic(n: usize, edges: &[LEdge]) -> bool {
        let mut indeg = vec![0usize; n];
        for ed in edges {
            indeg[ed.to] += 1;
        }
        let mut queue: Vec<usize> =
            (0..n).filter(|&v| indeg[v] == 0).collect();
        let mut seen = 0;
        while let Some(v) = queue.pop() {
            seen += 1;
            for ed in edges.iter().filter(|ed| ed.from == v) {
                indeg[ed.to] -= 1;
                if indeg[ed.to] == 0 {
                    queue.push(ed.to);
                }
            }
        }
        seen == n
    }

    #[test]
    fn dag_passes_through_unchanged() {
        let edges = vec![e(0, 1), e(1, 2), e(0, 2)];
        let r = make_acyclic(3, &edges);
        assert!(r.reversed.iter().all(|&b| !b));
        assert!(r.self_loops.is_empty());
        assert_eq!(r.edges.len(), 3);
        assert!(is_acyclic(3, &r.edges));
    }

    #[test]
    fn two_cycle_reverses_one_edge() {
        let edges = vec![e(0, 1), e(1, 0)];
        let r = make_acyclic(2, &edges);
        assert_eq!(r.reversed.iter().filter(|&&b| b).count(), 1);
        assert!(is_acyclic(2, &r.edges));
    }

    #[test]
    fn three_cycle_becomes_acyclic() {
        let edges = vec![e(0, 1), e(1, 2), e(2, 0)];
        let r = make_acyclic(3, &edges);
        assert!(is_acyclic(3, &r.edges));
        // Exactly one reversal suffices for a simple 3-cycle.
        assert_eq!(r.reversed.iter().filter(|&&b| b).count(), 1);
    }

    #[test]
    fn self_loop_extracted_not_reversed() {
        let edges = vec![e(0, 0), e(0, 1)];
        let r = make_acyclic(2, &edges);
        assert_eq!(r.self_loops, vec![0]);
        assert_eq!(r.edges.len(), 1);
        assert_eq!(r.orig, vec![1]);
        assert!(!r.reversed[0]);
    }

    #[test]
    fn disconnected_components_handled() {
        let edges = vec![e(0, 1), e(2, 3), e(3, 2)];
        let r = make_acyclic(4, &edges);
        assert!(is_acyclic(4, &r.edges));
    }

    #[test]
    fn deterministic() {
        let edges = vec![e(0, 1), e(1, 2), e(2, 0), e(1, 0)];
        let a = make_acyclic(3, &edges);
        let b = make_acyclic(3, &edges);
        assert_eq!(a.reversed, b.reversed);
        assert_eq!(a.self_loops, b.self_loops);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ogrenotes-mermaid layout::acyclic`
Expected: FAIL — `make_acyclic` not defined.

- [ ] **Step 3: Implement**

Add above the tests in `acyclic.rs`:

```rust
pub(crate) struct AcyclicResult {
    /// Surviving edges (self-loops removed), back-edges flipped.
    pub edges: Vec<LEdge>,
    /// Parallel to `edges`: original index of each surviving edge.
    pub orig: Vec<usize>,
    /// Indexed by ORIGINAL edge index: was this edge flipped for layout?
    pub reversed: Vec<bool>,
    /// Original indices of self-loop edges (removed from `edges`).
    pub self_loops: Vec<usize>,
}

/// Iterative DFS from every unvisited node in index order (determinism).
/// An edge into a node currently on the DFS stack (gray) is a back-edge:
/// flip it. Self-loops are extracted first.
pub(crate) fn make_acyclic(node_count: usize, edges: &[LEdge]) -> AcyclicResult {
    let mut reversed = vec![false; edges.len()];
    let mut self_loops = Vec::new();

    // Adjacency of non-self-loop edges, by original index.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for (i, e) in edges.iter().enumerate() {
        if e.from == e.to {
            self_loops.push(i);
        } else {
            adj[e.from].push(i);
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color = vec![Color::White; node_count];

    for start in 0..node_count {
        if color[start] != Color::White {
            continue;
        }
        // Stack of (node, next adjacency slot to try).
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        color[start] = Color::Gray;
        while let Some(&mut (v, ref mut slot)) = stack.last_mut() {
            if *slot < adj[v].len() {
                let ei = adj[v][*slot];
                *slot += 1;
                let to = edges[ei].to;
                match color[to] {
                    Color::White => {
                        color[to] = Color::Gray;
                        stack.push((to, 0));
                    }
                    Color::Gray => reversed[ei] = true, // back-edge
                    Color::Black => {}
                }
            } else {
                color[v] = Color::Black;
                stack.pop();
            }
        }
    }

    let mut out_edges = Vec::new();
    let mut orig = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        if e.from == e.to {
            continue;
        }
        let mut e2 = e.clone();
        if reversed[i] {
            std::mem::swap(&mut e2.from, &mut e2.to);
        }
        out_edges.push(e2);
        orig.push(i);
    }
    AcyclicResult { edges: out_edges, orig, reversed, self_loops }
}
```

Add `pub(crate) mod acyclic;` to `layout/mod.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p ogrenotes-mermaid layout::acyclic`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/layout/acyclic.rs crates/mermaid/src/layout/mod.rs
git commit -m "feat(mermaid): layout cycle breaking via DFS back-edge reversal

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: ranking (`rank.rs`)

**Files:**
- Create: `crates/mermaid/src/layout/rank.rs`
- Modify: `crates/mermaid/src/layout/mod.rs` (add `pub(crate) mod rank;`)

**Interfaces:**
- Consumes: acyclic edge list (`&[LEdge]`, guaranteed DAG) from Task 2.
- Produces: `pub(crate) fn assign_ranks(node_count: usize, edges: &[LEdge]) -> Vec<usize>` — rank per node; every edge satisfies `rank[to] > rank[from]`; ranks start at 0; isolated nodes get rank 0; source nodes are tightened to `min(child rank) - 1`.

- [ ] **Step 1: Write the failing tests**

Create `crates/mermaid/src/layout/rank.rs`:

```rust
//! Stage 2: longest-path ranking over the acyclic graph, then pull
//! sources down toward their children so single-edge chains from a
//! late-declared source don't stretch the whole diagram.

use super::LEdge;

#[cfg(test)]
mod tests {
    use super::*;

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    #[test]
    fn chain_ranks_increase() {
        let ranks = assign_ranks(3, &[e(0, 1), e(1, 2)]);
        assert_eq!(ranks, vec![0, 1, 2]);
    }

    #[test]
    fn diamond_takes_longest_path() {
        // 0 -> 1 -> 3, 0 -> 2 -> 3 plus 0 -> 3 direct: 3 must sit at rank 2.
        let ranks = assign_ranks(4, &[e(0, 1), e(0, 2), e(1, 3), e(2, 3), e(0, 3)]);
        assert_eq!(ranks[0], 0);
        assert_eq!(ranks[3], 2);
        assert!(ranks[1] == 1 && ranks[2] == 1);
    }

    #[test]
    fn every_edge_is_monotone() {
        let edges = vec![e(0, 2), e(1, 2), e(2, 3), e(1, 3)];
        let ranks = assign_ranks(4, &edges);
        for ed in &edges {
            assert!(ranks[ed.to] > ranks[ed.from]);
        }
    }

    #[test]
    fn source_tightened_to_children() {
        // 0->1->2->3 is the long chain; 4->3 is a lone source into the sink.
        // Without tightening 4 sits at rank 0; tightened it sits at rank 2.
        let ranks = assign_ranks(5, &[e(0, 1), e(1, 2), e(2, 3), e(4, 3)]);
        assert_eq!(ranks[4], ranks[3] - 1);
    }

    #[test]
    fn isolated_node_rank_zero() {
        let ranks = assign_ranks(2, &[]);
        assert_eq!(ranks, vec![0, 0]);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ogrenotes-mermaid layout::rank`
Expected: FAIL — `assign_ranks` not defined.

- [ ] **Step 3: Implement**

```rust
/// Longest-path ranks via Kahn topological order, then one tightening
/// pass: any node with NO incoming edges moves down to
/// `min(rank of successors) - 1`.
pub(crate) fn assign_ranks(node_count: usize, edges: &[LEdge]) -> Vec<usize> {
    let mut indeg = vec![0usize; node_count];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for e in edges {
        indeg[e.to] += 1;
        adj[e.from].push(e.to);
    }
    let mut ranks = vec![0usize; node_count];
    // Deterministic Kahn: process in ascending index order per wave.
    let mut queue: Vec<usize> = (0..node_count).filter(|&v| indeg[v] == 0).collect();
    let mut head = 0;
    while head < queue.len() {
        let v = queue[head];
        head += 1;
        for &to in &adj[v] {
            ranks[to] = ranks[to].max(ranks[v] + 1);
            indeg[to] -= 1;
            if indeg[to] == 0 {
                queue.push(to);
            }
        }
    }
    // Tighten sources (recompute indegree; the loop above consumed it).
    let mut indeg2 = vec![0usize; node_count];
    for e in edges {
        indeg2[e.to] += 1;
    }
    for v in 0..node_count {
        if indeg2[v] == 0 && !adj[v].is_empty() {
            let min_child = adj[v].iter().map(|&t| ranks[t]).min().unwrap_or(1);
            if min_child > 0 {
                ranks[v] = min_child - 1;
            }
        }
    }
    ranks
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p ogrenotes-mermaid layout::rank`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/layout/rank.rs crates/mermaid/src/layout/mod.rs
git commit -m "feat(mermaid): longest-path ranking with source tightening

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: ordering (`order.rs`) — dummy chains, crossing count, barycenter

**Files:**
- Create: `crates/mermaid/src/layout/order.rs`
- Modify: `crates/mermaid/src/layout/mod.rs` (add `pub(crate) mod order;`)

**Interfaces:**
- Consumes: acyclic edges + ranks (Tasks 2–3).
- Produces:
  - `pub(crate) struct OrderGraph { pub ranks: Vec<Vec<Slot>>, pub chains: Vec<Vec<SlotKind>> }` where
    - `Slot { pub kind: SlotKind, pub size: (f64, f64) }`, `enum SlotKind { Real(usize), Dummy { edge: usize, rank: usize } }` — positions within a rank change across reorders, so slots are identified by `SlotKind` VALUE (a real node's index, or a dummy's `(edge, rank)` pair), never by position.
    - `chains[i]` = for surviving edge `i` (acyclic index), the slot identities it passes through in order, endpoints included; consumers re-locate a slot's current position via `pub(crate) fn find(ranks: &[Vec<Slot>], kind: SlotKind) -> Option<(usize, usize)>`.
  - `pub(crate) fn build_order_graph(nodes: &[super::LNode], edges: &[LEdge], ranks: &[usize]) -> OrderGraph` — one `Real` slot per node in its rank; for each edge spanning >1 rank, `Dummy` slots on every intermediate rank; an edge with a label gets its FIRST dummy sized to the label (or, for single-span labeled edges, no dummy — label handling at midpoint in route).
  - `pub(crate) fn count_crossings(g: &OrderGraph, edges: &[LEdge]) -> usize`
  - `pub(crate) fn minimize_crossings(g: &mut OrderGraph, edges: &[LEdge])` — barycenter down/up sweeps, max 8 iterations, keeps the best-seen ordering, deterministic.

- [ ] **Step 1: Write the failing tests**

Create `crates/mermaid/src/layout/order.rs` with tests:

```rust
//! Stage 3: per-rank ordering. Multi-rank edges are split through dummy
//! slots (edge labels reserve space as sized dummies); barycenter sweeps
//! reduce crossings; iteration capped; best-seen ordering wins.

use super::{LEdge, LNode};

#[cfg(test)]
mod tests {
    use super::*;

    fn n() -> LNode {
        LNode { width: 20.0, height: 10.0, cluster: None }
    }

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    #[test]
    fn real_slots_land_on_their_ranks() {
        let nodes = vec![n(), n(), n()];
        let edges = vec![e(0, 1), e(1, 2)];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 2]);
        assert_eq!(g.ranks.len(), 3);
        assert!(matches!(g.ranks[0][0].kind, SlotKind::Real(0)));
        assert!(matches!(g.ranks[1][0].kind, SlotKind::Real(1)));
    }

    #[test]
    fn long_edge_gets_dummies_on_intermediate_ranks() {
        let nodes = vec![n(), n(), n(), n()];
        // 0 (rank 0) -> 3 (rank 3): dummies on ranks 1 and 2.
        let edges = vec![e(0, 1), e(1, 2), e(2, 3), e(0, 3)];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 2, 3]);
        let dummies: Vec<_> = g.ranks[1]
            .iter()
            .filter(|s| matches!(s.kind, SlotKind::Dummy { edge: 3, .. }))
            .collect();
        assert_eq!(dummies.len(), 1);
        assert_eq!(g.chains[3].len(), 4); // from, d1, d2, to
    }

    #[test]
    fn labeled_long_edge_dummy_carries_label_size() {
        let nodes = vec![n(), n(), n()];
        let edges = vec![
            e(0, 1),
            e(1, 2),
            LEdge { from: 0, to: 2, label: Some((30.0, 12.0)) },
        ];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 2]);
        let d = g.ranks[1]
            .iter()
            .find(|s| matches!(s.kind, SlotKind::Dummy { edge: 2, .. }))
            .expect("dummy");
        assert_eq!(d.size, (30.0, 12.0));
    }

    #[test]
    fn crossing_count_detects_the_classic_x() {
        // rank0: [0, 1], rank1: [2, 3]; edges 0->3 and 1->2 cross.
        let nodes = vec![n(), n(), n(), n()];
        let edges = vec![e(0, 3), e(1, 2)];
        let g = build_order_graph(&nodes, &edges, &[0, 0, 1, 1]);
        assert_eq!(count_crossings(&g, &edges), 1);
    }

    #[test]
    fn barycenter_removes_removable_crossings() {
        let nodes = vec![n(), n(), n(), n()];
        let edges = vec![e(0, 3), e(1, 2)];
        let mut g = build_order_graph(&nodes, &edges, &[0, 0, 1, 1]);
        minimize_crossings(&mut g, &edges);
        assert_eq!(count_crossings(&g, &edges), 0);
    }

    #[test]
    fn minimize_never_increases_crossings() {
        // K2,2 has an unavoidable crossing; must not get worse than input.
        let nodes = vec![n(), n(), n(), n()];
        let edges = vec![e(0, 2), e(0, 3), e(1, 2), e(1, 3)];
        let mut g = build_order_graph(&nodes, &edges, &[0, 0, 1, 1]);
        let before = count_crossings(&g, &edges);
        minimize_crossings(&mut g, &edges);
        assert!(count_crossings(&g, &edges) <= before);
    }

    #[test]
    fn deterministic_ordering() {
        let nodes = vec![n(), n(), n(), n(), n(), n()];
        let edges = vec![e(0, 4), e(1, 3), e(2, 5), e(0, 5), e(1, 4)];
        let ranks = vec![0, 0, 0, 1, 1, 1];
        let mut a = build_order_graph(&nodes, &edges, &ranks);
        let mut b = build_order_graph(&nodes, &edges, &ranks);
        minimize_crossings(&mut a, &edges);
        minimize_crossings(&mut b, &edges);
        let ka: Vec<Vec<String>> = a
            .ranks
            .iter()
            .map(|r| r.iter().map(|s| format!("{:?}", s.kind)).collect())
            .collect();
        let kb: Vec<Vec<String>> = b
            .ranks
            .iter()
            .map(|r| r.iter().map(|s| format!("{:?}", s.kind)).collect())
            .collect();
        assert_eq!(ka, kb);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ogrenotes-mermaid layout::order`
Expected: FAIL — types/functions not defined.

- [ ] **Step 3: Implement**

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SlotKind {
    Real(usize),
    /// Dummy waypoint for `edge` (acyclic index) at `rank`.
    Dummy { edge: usize, rank: usize },
}

#[derive(Debug, Clone)]
pub(crate) struct Slot {
    pub kind: SlotKind,
    pub size: (f64, f64),
}

#[derive(Debug, Clone)]
pub(crate) struct OrderGraph {
    /// ranks[r] = ordered slots on rank r.
    pub ranks: Vec<Vec<Slot>>,
    /// chains[edge] = slot identities the edge passes through, in order
    /// from source to target (both endpoints included).
    pub chains: Vec<Vec<SlotKind>>,
}

pub(crate) fn find(ranks: &[Vec<Slot>], kind: SlotKind) -> Option<(usize, usize)> {
    for (r, row) in ranks.iter().enumerate() {
        for (i, s) in row.iter().enumerate() {
            if s.kind == kind {
                return Some((r, i));
            }
        }
    }
    None
}

/// Default footprint a dummy occupies so parallel long edges don't fuse.
const DUMMY_SIZE: (f64, f64) = (8.0, 8.0);

pub(crate) fn build_order_graph(
    nodes: &[LNode],
    edges: &[LEdge],
    ranks: &[usize],
) -> OrderGraph {
    let max_rank = ranks.iter().copied().max().unwrap_or(0);
    let mut rows: Vec<Vec<Slot>> = vec![Vec::new(); max_rank + 1];
    for (i, node) in nodes.iter().enumerate() {
        rows[ranks[i]].push(Slot {
            kind: SlotKind::Real(i),
            size: (node.width, node.height),
        });
    }
    let mut chains: Vec<Vec<SlotKind>> = Vec::with_capacity(edges.len());
    for (ei, e) in edges.iter().enumerate() {
        let (rf, rt) = (ranks[e.from], ranks[e.to]);
        let mut chain = vec![SlotKind::Real(e.from)];
        if rt > rf + 1 {
            for (step, r) in (rf + 1..rt).enumerate() {
                // First dummy of a labeled edge is sized to the label so
                // ordering/positioning reserve room for it.
                let size = if step == 0 {
                    e.label.unwrap_or(DUMMY_SIZE)
                } else {
                    DUMMY_SIZE
                };
                let kind = SlotKind::Dummy { edge: ei, rank: r };
                rows[r].push(Slot { kind, size });
                chain.push(kind);
            }
        }
        chain.push(SlotKind::Real(e.to));
        chains.push(chain);
    }
    OrderGraph { ranks: rows, chains }
}

/// Positions of each slot's chain-neighbors on the adjacent rank.
fn neighbor_positions(
    g: &OrderGraph,
    rank: usize,
    upstream: bool,
) -> Vec<Vec<usize>> {
    // For each slot on `rank`, collect current index positions of the
    // slots adjacent to it (previous rank if upstream, next if not)
    // along every edge chain that passes through it.
    let adj_rank = if upstream { rank.wrapping_sub(1) } else { rank + 1 };
    let mut pos_of: std::collections::HashMap<SlotKind, usize> =
        std::collections::HashMap::new();
    if adj_rank < g.ranks.len() {
        for (i, s) in g.ranks[adj_rank].iter().enumerate() {
            pos_of.insert(s.kind, i);
        }
    }
    g.ranks[rank]
        .iter()
        .map(|slot| {
            let mut out = Vec::new();
            for chain in &g.chains {
                for w in chain.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let (here, there) = if upstream { (b, a) } else { (a, b) };
                    if here == slot.kind {
                        if let Some(&p) = pos_of.get(&there) {
                            out.push(p);
                        }
                    }
                }
            }
            out
        })
        .collect()
}

pub(crate) fn count_crossings(g: &OrderGraph, _edges: &[LEdge]) -> usize {
    // For each adjacent rank pair, count inversions among edge segment
    // endpoint pairs — O(k^2) per pair, fine at our caps.
    let mut total = 0;
    for r in 0..g.ranks.len().saturating_sub(1) {
        let pos_hi: std::collections::HashMap<SlotKind, usize> = g.ranks[r]
            .iter()
            .enumerate()
            .map(|(i, s)| (s.kind, i))
            .collect();
        let pos_lo: std::collections::HashMap<SlotKind, usize> = g.ranks[r + 1]
            .iter()
            .enumerate()
            .map(|(i, s)| (s.kind, i))
            .collect();
        let mut segs: Vec<(usize, usize)> = Vec::new();
        for chain in &g.chains {
            for w in chain.windows(2) {
                if let (Some(&a), Some(&b)) = (pos_hi.get(&w[0]), pos_lo.get(&w[1])) {
                    segs.push((a, b));
                }
            }
        }
        for i in 0..segs.len() {
            for j in i + 1..segs.len() {
                let (a1, b1) = segs[i];
                let (a2, b2) = segs[j];
                if (a1 < a2 && b1 > b2) || (a1 > a2 && b1 < b2) {
                    total += 1;
                }
            }
        }
    }
    total
}

const MAX_SWEEPS: usize = 8;

pub(crate) fn minimize_crossings(g: &mut OrderGraph, edges: &[LEdge]) {
    let mut best = g.ranks.clone();
    let mut best_cross = count_crossings(g, edges);
    for sweep in 0..MAX_SWEEPS {
        let downward = sweep % 2 == 0;
        let rank_iter: Vec<usize> = if downward {
            (1..g.ranks.len()).collect()
        } else {
            (0..g.ranks.len().saturating_sub(1)).rev().collect()
        };
        for r in rank_iter {
            let nbrs = neighbor_positions(g, r, downward);
            // Barycenter with stable tiebreak on current index.
            let mut keyed: Vec<(f64, usize, Slot)> = g.ranks[r]
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, s)| {
                    let bc = if nbrs[i].is_empty() {
                        i as f64 // keep childless slots where they are
                    } else {
                        nbrs[i].iter().sum::<usize>() as f64 / nbrs[i].len() as f64
                    };
                    (bc, i, s)
                })
                .collect();
            keyed.sort_by(|a, b| {
                a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1))
            });
            g.ranks[r] = keyed.into_iter().map(|(_, _, s)| s).collect();
        }
        let c = count_crossings(g, edges);
        if c < best_cross {
            best_cross = c;
            best = g.ranks.clone();
        }
        if c == 0 {
            break;
        }
    }
    g.ranks = best;
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p ogrenotes-mermaid layout::order`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/layout/order.rs crates/mermaid/src/layout/mod.rs
git commit -m "feat(mermaid): rank ordering — dummy chains, crossings, barycenter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: coordinates (`position.rs`)

**Files:**
- Create: `crates/mermaid/src/layout/position.rs`
- Modify: `crates/mermaid/src/layout/mod.rs` (add `pub(crate) mod position;`)

**Interfaces:**
- Consumes: ordered `OrderGraph` (Task 4), `NODE_GAP_X`, `RANK_GAP_Y`.
- Produces: `pub(crate) fn assign_coords(g: &OrderGraph) -> Coords` where `pub(crate) struct Coords { pub centers: std::collections::HashMap<SlotKind, (f64, f64)>, pub size: (f64, f64) }` — every slot (real + dummy) gets a center; invariants: same-rank neighbors never overlap horizontally (gap ≥ `NODE_GAP_X`), rank y = cumulative max heights + `RANK_GAP_Y`, all coordinates finite and ≥ half the slot's own extent (fully on-canvas).

- [ ] **Step 1: Write the failing tests**

Create `crates/mermaid/src/layout/position.rs`:

```rust
//! Stage 4: x-coordinates via median-of-neighbors with separation
//! enforcement (3 bounded sweeps: down, up, down). Deliberately simpler
//! than Brandes-Köpf; replaceable behind this API.

use super::order::{OrderGraph, SlotKind};
use super::{NODE_GAP_X, RANK_GAP_Y};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::order::build_order_graph;
    use crate::layout::{LEdge, LNode};

    fn n(w: f64) -> LNode {
        LNode { width: w, height: 20.0, cluster: None }
    }

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    fn no_overlap(g: &OrderGraph, c: &Coords) {
        for row in &g.ranks {
            for w in row.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                let ax = c.centers[&a.kind].0;
                let bx = c.centers[&b.kind].0;
                assert!(
                    bx - b.size.0 / 2.0 - (ax + a.size.0 / 2.0) >= NODE_GAP_X - 1e-6,
                    "overlap: {:?} at {ax} vs {:?} at {bx}",
                    a.kind,
                    b.kind
                );
            }
        }
    }

    #[test]
    fn chain_is_vertically_stacked_and_aligned() {
        let nodes = vec![n(40.0), n(40.0), n(40.0)];
        let edges = vec![e(0, 1), e(1, 2)];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 2]);
        let c = assign_coords(&g);
        let x0 = c.centers[&SlotKind::Real(0)].0;
        let x1 = c.centers[&SlotKind::Real(1)].0;
        let x2 = c.centers[&SlotKind::Real(2)].0;
        assert!((x0 - x1).abs() < 1e-6 && (x1 - x2).abs() < 1e-6);
        let y0 = c.centers[&SlotKind::Real(0)].1;
        let y1 = c.centers[&SlotKind::Real(1)].1;
        assert!(y1 - y0 >= 20.0 + RANK_GAP_Y - 1e-6);
    }

    #[test]
    fn siblings_never_overlap() {
        let nodes = vec![n(120.0), n(120.0), n(120.0), n(40.0)];
        let edges = vec![e(3, 0), e(3, 1), e(3, 2)];
        let g = build_order_graph(&nodes, &edges, &[1, 1, 1, 0]);
        let c = assign_coords(&g);
        no_overlap(&g, &c);
    }

    #[test]
    fn parent_centers_over_children() {
        let nodes = vec![n(40.0), n(40.0), n(40.0)];
        let edges = vec![e(0, 1), e(0, 2)];
        let g = build_order_graph(&nodes, &edges, &[0, 1, 1]);
        let c = assign_coords(&g);
        let px = c.centers[&SlotKind::Real(0)].0;
        let c1 = c.centers[&SlotKind::Real(1)].0;
        let c2 = c.centers[&SlotKind::Real(2)].0;
        let mid = (c1 + c2) / 2.0;
        assert!((px - mid).abs() < 1.0, "parent {px} vs children mid {mid}");
    }

    #[test]
    fn all_coords_finite_and_on_canvas() {
        let nodes = vec![n(40.0), n(40.0), n(40.0), n(40.0), n(40.0)];
        let edges = vec![e(0, 2), e(1, 2), e(2, 3), e(2, 4), e(0, 4)];
        let g = build_order_graph(&nodes, &edges, &[0, 0, 1, 2, 2]);
        let c = assign_coords(&g);
        for row in &g.ranks {
            for s in row {
                let (x, y) = c.centers[&s.kind];
                assert!(x.is_finite() && y.is_finite());
                assert!(x - s.size.0 / 2.0 >= -1e-6);
                assert!(y - s.size.1 / 2.0 >= -1e-6);
                assert!(x + s.size.0 / 2.0 <= c.size.0 + 1e-6);
                assert!(y + s.size.1 / 2.0 <= c.size.1 + 1e-6);
            }
        }
        no_overlap(&g, &c);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ogrenotes-mermaid layout::position`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
pub(crate) struct Coords {
    pub centers: std::collections::HashMap<SlotKind, (f64, f64)>,
    pub size: (f64, f64),
}

const COORD_SWEEPS: usize = 3;
const CANVAS_PAD: f64 = 20.0;

pub(crate) fn assign_coords(g: &OrderGraph) -> Coords {
    // y: rank tops from cumulative max heights.
    let rank_h: Vec<f64> = g
        .ranks
        .iter()
        .map(|row| row.iter().map(|s| s.size.1).fold(0.0, f64::max))
        .collect();
    let mut rank_y = Vec::with_capacity(g.ranks.len());
    let mut y = CANVAS_PAD;
    for h in &rank_h {
        rank_y.push(y + h / 2.0);
        y += h + RANK_GAP_Y;
    }

    // x: initial packing left-to-right per rank.
    let mut xs: Vec<Vec<f64>> = g
        .ranks
        .iter()
        .map(|row| {
            let mut out = Vec::with_capacity(row.len());
            let mut x = CANVAS_PAD;
            for s in row {
                out.push(x + s.size.0 / 2.0);
                x += s.size.0 + NODE_GAP_X;
            }
            out
        })
        .collect();

    // Median sweeps with separation enforcement.
    for sweep in 0..COORD_SWEEPS {
        let downward = sweep % 2 == 0;
        let ranks_iter: Vec<usize> = if downward {
            (1..g.ranks.len()).collect()
        } else {
            (0..g.ranks.len().saturating_sub(1)).rev().collect()
        };
        for r in ranks_iter {
            let nbrs = super::order::neighbor_positions(g, r, downward);
            let adj = if downward { r - 1 } else { r + 1 };
            // Desired x = median of neighbor centers on the adjacent rank.
            let desired: Vec<f64> = g.ranks[r]
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    if nbrs[i].is_empty() {
                        xs[r][i]
                    } else {
                        let mut vals: Vec<f64> =
                            nbrs[i].iter().map(|&p| xs[adj][p]).collect();
                        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        vals[vals.len() / 2]
                    }
                })
                .collect();
            xs[r] = desired;
            enforce_separation(&g.ranks[r], &mut xs[r]);
        }
    }

    // Normalize: shift so min left edge = CANVAS_PAD; compute canvas size.
    let mut min_left = f64::INFINITY;
    let mut max_right = f64::NEG_INFINITY;
    for (r, row) in g.ranks.iter().enumerate() {
        for (i, s) in row.iter().enumerate() {
            min_left = min_left.min(xs[r][i] - s.size.0 / 2.0);
            max_right = max_right.max(xs[r][i] + s.size.0 / 2.0);
        }
    }
    if !min_left.is_finite() {
        min_left = 0.0;
        max_right = 0.0;
    }
    let shift = CANVAS_PAD - min_left;

    let mut centers = std::collections::HashMap::new();
    for (r, row) in g.ranks.iter().enumerate() {
        for (i, s) in row.iter().enumerate() {
            centers.insert(s.kind, (xs[r][i] + shift, rank_y[r]));
        }
    }
    let total_h = y - RANK_GAP_Y + CANVAS_PAD; // y overshoots by one gap
    let size = (max_right + shift + CANVAS_PAD, total_h.max(2.0 * CANVAS_PAD));
    Coords { centers, size }
}

/// Two-pass min-gap enforcement that preserves order: push right, then
/// push left, then average toward desired where slack allows. Guarantees
/// gap >= NODE_GAP_X between horizontal extents.
fn enforce_separation(row: &[super::order::Slot], xs: &mut [f64]) {
    let n = row.len();
    if n < 2 {
        return;
    }
    let mut right = xs.to_vec();
    for i in 1..n {
        let min_x =
            right[i - 1] + row[i - 1].size.0 / 2.0 + NODE_GAP_X + row[i].size.0 / 2.0;
        if right[i] < min_x {
            right[i] = min_x;
        }
    }
    let mut left = xs.to_vec();
    for i in (0..n - 1).rev() {
        let max_x =
            left[i + 1] - row[i + 1].size.0 / 2.0 - NODE_GAP_X - row[i].size.0 / 2.0;
        if left[i] > max_x {
            left[i] = max_x;
        }
    }
    for i in 0..n {
        xs[i] = (right[i] + left[i]) / 2.0;
    }
    // Final hard pass so float averaging can't reintroduce an overlap.
    for i in 1..n {
        let min_x = xs[i - 1] + row[i - 1].size.0 / 2.0 + NODE_GAP_X + row[i].size.0 / 2.0;
        if xs[i] < min_x {
            xs[i] = min_x;
        }
    }
}
```

Also change `fn neighbor_positions` in `order.rs` from private to `pub(crate) fn neighbor_positions` (position.rs reuses it).

- [ ] **Step 4: Run tests**

Run: `cargo test -p ogrenotes-mermaid layout::position` — Expected: PASS (4 tests).
Run: `cargo test -p ogrenotes-mermaid layout::order` — Expected: still PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/layout/position.rs crates/mermaid/src/layout/order.rs crates/mermaid/src/layout/mod.rs
git commit -m "feat(mermaid): median x-assignment with separation enforcement

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: routing + flat-graph orchestration (`route.rs`, `run()`)

**Files:**
- Create: `crates/mermaid/src/layout/route.rs`
- Modify: `crates/mermaid/src/layout/mod.rs` (add `pub(crate) mod route;` and the `run()` orchestrator + its tests)

**Interfaces:**
- Consumes: everything from Tasks 1–5.
- Produces:
  - `route.rs`: `pub(crate) fn route_edges(input: &LayoutInput, ac: &acyclic::AcyclicResult, g: &order::OrderGraph, coords: &position::Coords) -> Vec<EdgePath>` — one `EdgePath` per ORIGINAL edge (self-loops included); polyline endpoints clipped to node bounding boxes; reversed edges emitted with points in TRUE direction; label position = labeled dummy center, or segment midpoint for single-span labeled edges; self-loops get a 4-point loop stub on the node's right side.
  - `mod.rs`: `pub(crate) fn run(input: &LayoutInput) -> Result<Layout, String>` — validate → (clusters handled in Task 7; for now an input with non-empty `clusters` returns `Err("clusters not yet supported")`) → acyclic → rank → order → minimize → coords → route → apply_direction.

- [ ] **Step 1: Write the failing tests** (append to `layout/mod.rs`'s test module)

```rust
    fn simple_input(dir: Direction) -> LayoutInput {
        LayoutInput {
            nodes: vec![node(60.0, 24.0), node(60.0, 24.0), node(60.0, 24.0)],
            edges: vec![
                LEdge { from: 0, to: 1, label: None },
                LEdge { from: 1, to: 2, label: Some((30.0, 12.0)) },
            ],
            clusters: vec![],
            direction: dir,
        }
    }

    #[test]
    fn run_lays_out_a_chain() {
        let l = run(&simple_input(Direction::TB)).unwrap();
        assert_eq!(l.node_centers.len(), 3);
        assert_eq!(l.edge_paths.len(), 2);
        // Chain stacks downward in TB.
        assert!(l.node_centers[1].1 > l.node_centers[0].1);
        assert!(l.node_centers[2].1 > l.node_centers[1].1);
        // Edge endpoints clip at node boxes, not centers.
        let p0 = &l.edge_paths[0].points;
        assert!(p0.first().unwrap().1 > l.node_centers[0].1);
        assert!(p0.last().unwrap().1 < l.node_centers[1].1);
        // Labeled single-span edge gets a midpoint label anchor.
        assert!(l.edge_paths[1].label_at.is_some());
    }

    #[test]
    fn run_lr_flows_rightward() {
        let l = run(&simple_input(Direction::LR)).unwrap();
        assert!(l.node_centers[1].0 > l.node_centers[0].0);
        assert!(l.node_centers[2].0 > l.node_centers[1].0);
    }

    #[test]
    fn run_handles_cycle_with_true_direction_points() {
        let input = LayoutInput {
            nodes: vec![node(40.0, 20.0), node(40.0, 20.0)],
            edges: vec![
                LEdge { from: 0, to: 1, label: None },
                LEdge { from: 1, to: 0, label: None },
            ],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        let back = l.edge_paths.iter().find(|p| p.reversed).expect("one reversed");
        // True direction: starts near node 1, ends near node 0.
        let start = back.points.first().unwrap();
        let end = back.points.last().unwrap();
        let d_start_1 = (start.1 - l.node_centers[1].1).abs();
        let d_end_0 = (end.1 - l.node_centers[0].1).abs();
        assert!(d_start_1 < d_end_0.max(60.0));
        assert!(end.1 < start.1, "back edge should flow upward in TB");
    }

    #[test]
    fn run_self_loop_has_points() {
        let input = LayoutInput {
            nodes: vec![node(40.0, 20.0)],
            edges: vec![LEdge { from: 0, to: 0, label: None }],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        assert!(l.edge_paths[0].points.len() >= 4);
        for p in &l.edge_paths[0].points {
            assert!(p.0.is_finite() && p.1.is_finite());
        }
    }

    #[test]
    fn run_rejects_over_cap() {
        let input = LayoutInput {
            nodes: (0..=MAX_NODES)
                .map(|_| node(1.0, 1.0))
                .collect(),
            edges: vec![],
            clusters: vec![],
            direction: Direction::TB,
        };
        assert!(run(&input).unwrap_err().contains("too large"));
    }

    #[test]
    fn run_empty_graph_ok() {
        let input = LayoutInput {
            nodes: vec![],
            edges: vec![],
            clusters: vec![],
            direction: Direction::TB,
        };
        let l = run(&input).unwrap();
        assert!(l.node_centers.is_empty());
        assert!(l.size.0 > 0.0 && l.size.1 > 0.0);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ogrenotes-mermaid layout::tests::run_`
Expected: FAIL — `run` not defined.

- [ ] **Step 3: Implement `route.rs`**

```rust
//! Stage 5: assemble edge polylines through dummy waypoints, clip
//! endpoints at node bounding boxes, restore true direction for
//! reversed edges, and stub self-loops.

use super::acyclic::AcyclicResult;
use super::order::{OrderGraph, SlotKind};
use super::position::Coords;
use super::{EdgePath, LayoutInput};

/// Clip point `toward` -> `center` at the bounding box of a node with
/// the given half-extents, returning the border intersection.
fn clip_to_box(center: (f64, f64), half: (f64, f64), toward: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (toward.0 - center.0, toward.1 - center.1);
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return center;
    }
    let tx = if dx.abs() < 1e-9 { f64::INFINITY } else { half.0 / dx.abs() };
    let ty = if dy.abs() < 1e-9 { f64::INFINITY } else { half.1 / dy.abs() };
    let t = tx.min(ty).min(1.0);
    (center.0 + dx * t, center.1 + dy * t)
}

pub(crate) fn route_edges(
    input: &LayoutInput,
    ac: &AcyclicResult,
    g: &OrderGraph,
    coords: &Coords,
) -> Vec<EdgePath> {
    let mut out: Vec<EdgePath> = Vec::with_capacity(input.edges.len());

    // Surviving (non-self-loop) edges, chain index == acyclic edge index.
    for (ai, chain) in g.chains.iter().enumerate() {
        let orig = ac.orig[ai];
        let mut pts: Vec<(f64, f64)> = Vec::with_capacity(chain.len());
        let mut label_at = None;
        for kind in chain {
            let c = coords.centers[kind];
            if let SlotKind::Dummy { edge, .. } = kind {
                if label_at.is_none() && input.edges[ac.orig[*edge]].label.is_some() {
                    label_at = Some(c);
                }
            }
            pts.push(c);
        }
        // Clip first/last segment at the endpoint node boxes.
        let first_kind = chain.first().copied().unwrap();
        let last_kind = chain.last().copied().unwrap();
        if let (SlotKind::Real(a), SlotKind::Real(b)) = (first_kind, last_kind) {
            let na = &input.nodes[a];
            let nb = &input.nodes[b];
            let a_c = pts[0];
            let b_c = *pts.last().unwrap();
            let a_next = pts.get(1).copied().unwrap_or(b_c);
            let b_prev = pts[pts.len().saturating_sub(2)];
            pts[0] = clip_to_box(a_c, (na.width / 2.0, na.height / 2.0), a_next);
            let n = pts.len();
            pts[n - 1] = clip_to_box(b_c, (nb.width / 2.0, nb.height / 2.0), b_prev);
        }
        // Single-span labeled edge: label at segment midpoint.
        if label_at.is_none() && input.edges[orig].label.is_some() {
            let a = pts[0];
            let b = *pts.last().unwrap();
            label_at = Some(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
        }
        let reversed = ac.reversed[orig];
        if reversed {
            pts.reverse(); // restore true direction
        }
        out.push(EdgePath { edge: orig, points: pts, label_at, reversed });
    }

    // Self-loops: small rectangle stub off the node's right edge.
    for &orig in &ac.self_loops {
        let v = input.edges[orig].from;
        // Node centers for real slots come from coords.
        let c = coords.centers[&SlotKind::Real(v)];
        let hw = input.nodes[v].width / 2.0;
        let hh = input.nodes[v].height / 2.0;
        let stub = 18.0;
        let pts = vec![
            (c.0 + hw, c.1 - hh * 0.5),
            (c.0 + hw + stub, c.1 - hh * 0.5),
            (c.0 + hw + stub, c.1 + hh * 0.5),
            (c.0 + hw, c.1 + hh * 0.5),
        ];
        let label_at = input.edges[orig]
            .label
            .map(|_| (c.0 + hw + stub + 4.0, c.1));
        out.push(EdgePath { edge: orig, points: pts, label_at, reversed: false });
    }

    // Emit in original-edge order for determinism.
    out.sort_by_key(|p| p.edge);
    out
}
```

- [ ] **Step 4: Implement `run()` in `layout/mod.rs`**

```rust
pub(crate) mod acyclic;
pub(crate) mod order;
pub(crate) mod position;
pub(crate) mod route;

/// Full pipeline for FLAT graphs. Cluster inputs are dispatched to the
/// collapse-expand driver (Task 7); until then they error.
pub(crate) fn run(input: &LayoutInput) -> Result<Layout, String> {
    validate(input)?;
    if !input.clusters.is_empty() {
        return Err("clusters not yet supported".to_string()); // Task 7 replaces
    }
    run_flat(input)
}

pub(crate) fn run_flat(input: &LayoutInput) -> Result<Layout, String> {
    let ac = acyclic::make_acyclic(input.nodes.len(), &input.edges);
    let ranks = rank::assign_ranks(input.nodes.len(), &ac.edges);
    // Order-graph nodes are the ORIGINAL nodes; edges are the surviving
    // acyclic edges (labels travel with them).
    let surviving: Vec<LEdge> = ac
        .edges
        .iter()
        .zip(&ac.orig)
        .map(|(e, &o)| LEdge { from: e.from, to: e.to, label: input.edges[o].label })
        .collect();
    let mut g = order::build_order_graph(&input.nodes, &surviving, &ranks);
    order::minimize_crossings(&mut g, &surviving);
    let coords = position::assign_coords(&g);
    let mut node_centers = vec![(0.0, 0.0); input.nodes.len()];
    for (kind, c) in &coords.centers {
        if let order::SlotKind::Real(i) = kind {
            node_centers[*i] = *c;
        }
    }
    let edge_paths = route::route_edges(input, &ac, &g, &coords);
    let mut layout = Layout {
        node_centers,
        edge_paths,
        cluster_rects: vec![],
        size: coords.size,
    };
    apply_direction(&mut layout, input.direction);
    Ok(layout)
}
```

Also add `pub(crate) mod rank;` if not already declared.

- [ ] **Step 5: Run all layout tests**

Run: `cargo test -p ogrenotes-mermaid layout::`
Expected: PASS (all tasks 1–6 tests, 30+).

- [ ] **Step 6: Commit**

```bash
git add crates/mermaid/src/layout/route.rs crates/mermaid/src/layout/mod.rs
git commit -m "feat(mermaid): edge routing + flat-graph layout orchestration

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: clusters via collapse-expand (`cluster.rs`)

**Files:**
- Create: `crates/mermaid/src/layout/cluster.rs`
- Modify: `crates/mermaid/src/layout/mod.rs` (`run()` dispatches cluster inputs to `cluster::run_clustered`; add `pub(crate) mod cluster;`)

**Interfaces:**
- Consumes: `run_flat` (Task 6), all types.
- Produces: `pub(crate) fn run_clustered(input: &LayoutInput) -> Result<Layout, String>` — recursive collapse-expand as specified: lay out each cluster's members as a subgraph (in TB), collapse to a super-node sized to the sub-layout + title strip + padding, lay out the parent graph, expand in place, re-route cross-boundary edge endpoints to true inner nodes. `cluster_rects[i]` = cluster `i`'s final rect INCLUDING its title strip. Direction applied once at the very end (inner layouts are computed in TB and translated; `run_clustered` calls `apply_direction` itself; `run_flat`'s internal call is factored so it isn't applied twice — split `run_flat` into `layout_tb(input) -> Layout` (no direction) and keep `run_flat` = `layout_tb` + `apply_direction`).

**Algorithm (write exactly this):**
1. Build the cluster forest; process bottom-up (children before parents).
2. For each cluster `c` (deepest first): collect member nodes (direct members; nested clusters already collapsed to their super-node placeholders), extract the induced subgraph (members + intra-member edges), `layout_tb` it, and record the sub-layout. Replace members with ONE placeholder node of size `(sub.size.0, sub.size.1 + title.1 + CLUSTER_PAD)` in the parent graph. Maintain a mapping original-node → representative placeholder for edge collapsing.
3. Top level: `layout_tb` the top graph (top-level real nodes + top-level cluster placeholders). Edges between different representatives collapse to representative endpoints (dedupe is NOT needed; parallel edges are allowed); intra-cluster edges were consumed by the sub-layouts.
4. Expand recursively: each cluster's sub-layout is translated so its content box sits inside the placeholder's rect below the title strip; inner node centers/edge paths translate rigidly.
5. Cross-boundary edges (endpoints in different clusters/top level): take the TOP-level routed path between representatives, then replace the first/last point with the border-clip toward the true inner node's absolute center (`clip_to_box` on the inner node).
6. `apply_direction` once on the assembled whole.

`const CLUSTER_PAD: f64 = 12.0;`

- [ ] **Step 1: Write the failing tests** (in `cluster.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Direction, LCluster, LEdge, LNode, LayoutInput};

    fn node_in(c: Option<usize>) -> LNode {
        LNode { width: 60.0, height: 24.0, cluster: c }
    }

    fn e(from: usize, to: usize) -> LEdge {
        LEdge { from, to, label: None }
    }

    fn input_one_cluster() -> LayoutInput {
        // 0 top-level -> 1 (in cluster) -> 2 (in cluster) -> 3 top-level
        LayoutInput {
            nodes: vec![node_in(None), node_in(Some(0)), node_in(Some(0)), node_in(None)],
            edges: vec![e(0, 1), e(1, 2), e(2, 3)],
            clusters: vec![LCluster { parent: None, title: (50.0, 16.0) }],
            direction: Direction::TB,
        }
    }

    fn inside(p: (f64, f64), r: &crate::layout::Rect) -> bool {
        p.0 >= r.x && p.0 <= r.x + r.w && p.1 >= r.y && p.1 <= r.y + r.h
    }

    #[test]
    fn members_inside_cluster_rect() {
        let l = run_clustered(&input_one_cluster()).unwrap();
        let r = &l.cluster_rects[0];
        assert!(inside(l.node_centers[1], r));
        assert!(inside(l.node_centers[2], r));
        assert!(!inside(l.node_centers[0], r));
        assert!(!inside(l.node_centers[3], r));
    }

    #[test]
    fn cross_boundary_edges_reach_inner_nodes() {
        let l = run_clustered(&input_one_cluster()).unwrap();
        // Edge 0 (0 -> 1): last point should be at node 1's box border,
        // i.e. within half-extents of node 1's center.
        let end = *l.edge_paths[0].points.last().unwrap();
        let c1 = l.node_centers[1];
        assert!((end.0 - c1.0).abs() <= 30.0 + 1e-6);
        assert!((end.1 - c1.1).abs() <= 12.0 + 1e-6);
    }

    #[test]
    fn nested_clusters_nest_rects() {
        let input = LayoutInput {
            nodes: vec![node_in(Some(0)), node_in(Some(1))],
            edges: vec![e(0, 1)],
            clusters: vec![
                LCluster { parent: None, title: (40.0, 16.0) },
                LCluster { parent: Some(0), title: (40.0, 16.0) },
            ],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        let outer = &l.cluster_rects[0];
        let inner = &l.cluster_rects[1];
        assert!(inner.x >= outer.x && inner.y >= outer.y);
        assert!(inner.x + inner.w <= outer.x + outer.w + 1e-6);
        assert!(inner.y + inner.h <= outer.y + outer.h + 1e-6);
        assert!(inside(l.node_centers[1], inner));
    }

    #[test]
    fn lr_direction_applies_to_whole() {
        let mut input = input_one_cluster();
        input.direction = Direction::LR;
        let l = run_clustered(&input).unwrap();
        // Chain flows rightward overall.
        assert!(l.node_centers[3].0 > l.node_centers[0].0);
    }

    #[test]
    fn empty_cluster_is_harmless() {
        let input = LayoutInput {
            nodes: vec![node_in(None)],
            edges: vec![],
            clusters: vec![LCluster { parent: None, title: (40.0, 16.0) }],
            direction: Direction::TB,
        };
        let l = run_clustered(&input).unwrap();
        // Rect exists (title-sized minimum), no panic.
        assert_eq!(l.cluster_rects.len(), 1);
        assert!(l.cluster_rects[0].w > 0.0);
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p ogrenotes-mermaid layout::cluster` → FAIL.

- [ ] **Step 3: Implement `run_clustered` following the 6-step algorithm above.**

Key implementation notes (the implementer writes the body; the algorithm is fully specified above and the tests are the contract):
- Represent the recursion explicitly: compute cluster depth from parent chains, process clusters in descending depth order; maintain `rep: Vec<usize>` mapping each original node to its current representative in the "current" shrinking graph, and a `Vec<Option<SubLayout>>` per cluster storing `{ layout: Layout (TB, unrotated), members: Vec<usize>, placeholder: usize }`.
- Placeholders are appended as synthetic node indices past the real ones inside a working node list (`Vec<LNode>` clone); real node indices stay stable.
- The final assembly walks clusters top-down translating sub-layouts by their placeholder's top-left + title offset, writing absolute centers for real members and absolute rects for cluster placeholders (`cluster_rects`).
- Cross-boundary edge fix-up uses `route::clip_to_box` — promote it to `pub(crate)`.
- Split `run_flat` per the interface note: `pub(crate) fn layout_tb(input: &LayoutInput) -> Result<Layout, String>` (no direction) and `run_flat` wraps it. `run_clustered` uses `layout_tb` for sub-layouts and the top level.
- Update `run()` in `mod.rs`: `if input.clusters.is_empty() { run_flat(input) } else { cluster::run_clustered(input) }`.

- [ ] **Step 4: Run tests** — `cargo test -p ogrenotes-mermaid layout::` → all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/layout/cluster.rs crates/mermaid/src/layout/route.rs crates/mermaid/src/layout/mod.rs
git commit -m "feat(mermaid): cluster layout via recursive collapse-expand

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: layout property tests (`proptest` dev-dep)

**Files:**
- Modify: `crates/mermaid/Cargo.toml` (add `[dev-dependencies] proptest = "1"`)
- Modify: `crates/mermaid/src/lib.rs` + `crates/mermaid/src/layout/mod.rs` — property tests live OUTSIDE the crate in `crates/mermaid/tests/layout_props.rs`, which needs access: add `#[doc(hidden)] pub mod layout_public { ... }`? NO — instead make `layout` module `pub` gated: `#[cfg_attr(test, allow(unused))] pub mod layout;` is wrong too. DECISION: keep property tests INSIDE the crate as `crates/mermaid/src/layout/props.rs` declared `#[cfg(test)] mod props;` inside `layout/mod.rs` — no visibility change needed, dev-dep still applies to unit tests.
- Create: `crates/mermaid/src/layout/props.rs`

**Interfaces:** Consumes `run`, `LayoutInput`, all types. Produces test coverage only.

- [ ] **Step 1: Add the dev-dependency**

In `crates/mermaid/Cargo.toml`:

```toml
[dev-dependencies]
proptest = "1"
```

(Zero runtime deps preserved — dev-deps don't ship and don't touch the wasm bundle. Confirm `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown` stays clean.)

- [ ] **Step 2: Write the property tests**

Create `crates/mermaid/src/layout/props.rs` and declare `#[cfg(test)] mod props;` in `layout/mod.rs`:

```rust
//! Property tests: the full layout pipeline holds its invariants for
//! arbitrary graphs. Uses proptest (dev-dependency only).

use super::*;
use proptest::prelude::*;

fn arb_input() -> impl Strategy<Value = LayoutInput> {
    // Up to 12 nodes, 20 edges, 3 clusters — small enough to run fast,
    // shaped enough to hit cycles, self-loops, fan-in/out, clusters.
    (1usize..12).prop_flat_map(|n| {
        let nodes = proptest::collection::vec(
            ((10.0f64..120.0), (10.0f64..40.0), proptest::option::of(0usize..3)),
            n,
        );
        let edges = proptest::collection::vec((0usize..n, 0usize..n), 0..20);
        (nodes, edges, 0usize..4).prop_map(|(nodes, edges, dir)| {
            let cluster_count = nodes
                .iter()
                .filter_map(|(_, _, c)| *c)
                .max()
                .map(|m| m + 1)
                .unwrap_or(0);
            LayoutInput {
                nodes: nodes
                    .into_iter()
                    .map(|(w, h, c)| LNode { width: w, height: h, cluster: c })
                    .collect(),
                edges: edges
                    .into_iter()
                    .map(|(f, t)| LEdge { from: f, to: t, label: None })
                    .collect(),
                clusters: (0..cluster_count)
                    .map(|_| LCluster { parent: None, title: (30.0, 14.0) })
                    .collect(),
                direction: match dir {
                    0 => Direction::TB,
                    1 => Direction::BT,
                    2 => Direction::LR,
                    _ => Direction::RL,
                },
            }
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn pipeline_never_panics_and_no_nan(input in arb_input()) {
        if let Ok(l) = run(&input) {
            for (x, y) in &l.node_centers {
                prop_assert!(x.is_finite() && y.is_finite());
            }
            for ep in &l.edge_paths {
                for (x, y) in &ep.points {
                    prop_assert!(x.is_finite() && y.is_finite());
                }
            }
            prop_assert!(l.size.0.is_finite() && l.size.1.is_finite());
            prop_assert_eq!(l.edge_paths.len(), input.edges.len());
        }
    }

    #[test]
    fn pipeline_deterministic(input in arb_input()) {
        let a = run(&input);
        let b = run(&input);
        match (a, b) {
            (Ok(la), Ok(lb)) => {
                prop_assert_eq!(la.node_centers, lb.node_centers);
                prop_assert_eq!(la.size, lb.size);
            }
            (Err(ea), Err(eb)) => prop_assert_eq!(ea, eb),
            _ => prop_assert!(false, "one Ok one Err"),
        }
    }

    #[test]
    fn flat_no_node_overlaps(input in arb_input()) {
        // Overlap invariant asserted on flat graphs only (cluster
        // expansion translates sub-layouts; cross-cluster spacing is a
        // looser guarantee).
        let mut input = input;
        for n in &mut input.nodes { n.cluster = None; }
        input.clusters.clear();
        if let Ok(l) = run(&input) {
            for i in 0..input.nodes.len() {
                for j in i + 1..input.nodes.len() {
                    let (ci, cj) = (l.node_centers[i], l.node_centers[j]);
                    let (ni, nj) = (&input.nodes[i], &input.nodes[j]);
                    let x_sep = (ci.0 - cj.0).abs()
                        >= (ni.width + nj.width) / 2.0 - 1e-6;
                    let y_sep = (ci.1 - cj.1).abs()
                        >= (ni.height + nj.height) / 2.0 - 1e-6;
                    prop_assert!(
                        x_sep || y_sep,
                        "nodes {i} and {j} overlap: {ci:?} {cj:?}"
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 3: Run** — `cargo test -p ogrenotes-mermaid layout::props` → PASS (3 properties × 256 cases). If a case fails, FIX THE PIPELINE, not the property; commit any regression seed file proptest writes (`crates/mermaid/proptest-regressions/`).

- [ ] **Step 4: wasm sanity** — `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/Cargo.toml Cargo.lock crates/mermaid/src/layout/props.rs crates/mermaid/src/layout/mod.rs
git commit -m "test(mermaid): property tests for the layout pipeline

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

(Also `git add crates/mermaid/proptest-regressions/` if created.)

---

### Task 9: flowchart model, shared `escape_xml`, text measurement

**Files:**
- Create: `crates/mermaid/src/flowchart/mod.rs`
- Create: `crates/mermaid/src/flowchart/measure.rs`
- Modify: `crates/mermaid/src/lib.rs` (add `mod flowchart;`; promote `escape_xml`)
- Modify: `crates/mermaid/src/pie.rs` (use the promoted `escape_xml`)

**Interfaces:**
- Produces (consumed by Tasks 10–14):
  - `crate::escape_xml(s: &str) -> String` — moved from `pie.rs` verbatim (`pub(crate)` in `lib.rs`); `pie.rs` calls `crate::escape_xml`; its own definition is deleted. No behavior change; pie tests stay green.
  - In `flowchart/mod.rs` (all `pub(crate)`):
    ```rust
    pub(crate) mod measure;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum ShapeKind {
        Rect, Rounded, Stadium, Circle, DoubleCircle, Diamond, Hexagon,
        Parallelogram, ParallelogramRev, Trapezoid, TrapezoidRev, Cylinder, Flag,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum EdgeKind { Arrow, Open, Dotted, Thick }

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
    ```
  - In `measure.rs`:
    - `pub(crate) const FONT_PX: f64 = 14.0;`
    - `pub(crate) const LINE_H: f64 = 19.0;`
    - `pub(crate) fn text_size(s: &str) -> (f64, f64)` — splits on `<br/>` (also accepts `<br>` and `<br />`), width = max line width via per-char class table × `FONT_PX`, height = line count × `LINE_H`. Empty string → `(0.0, LINE_H)`.

- [ ] **Step 1: Write the failing measurement tests**

Create `crates/mermaid/src/flowchart/measure.rs`:

```rust
//! Heuristic text measurement — there is no DOM/canvas in pure Rust, so
//! widths come from a char-class table with generous padding downstream.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wider_text_measures_wider() {
        assert!(text_size("wide text here").0 > text_size("hi").0);
    }

    #[test]
    fn narrow_chars_narrower_than_wide() {
        // 4 narrow chars vs 4 normal-width chars.
        assert!(text_size("ilil").0 < text_size("wood").0);
    }

    #[test]
    fn cjk_wider_than_ascii() {
        assert!(text_size("图表").0 > text_size("ab").0);
    }

    #[test]
    fn br_splits_lines() {
        let (w1, h1) = text_size("hello world");
        let (w2, h2) = text_size("hello<br/>world");
        assert!(h2 > h1);
        assert!(w2 < w1);
        assert_eq!(h2, 2.0 * LINE_H);
        // <br> and <br /> variants also split.
        assert_eq!(text_size("a<br>b").1, 2.0 * LINE_H);
        assert_eq!(text_size("a<br />b").1, 2.0 * LINE_H);
    }

    #[test]
    fn empty_is_one_line_high() {
        assert_eq!(text_size(""), (0.0, LINE_H));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ogrenotes-mermaid flowchart::measure`
Expected: FAIL (module not declared / fn missing). Add `mod flowchart;` to `lib.rs` and create `flowchart/mod.rs` with the model types above plus `pub(crate) mod measure;`. Put `#![allow(dead_code)]` at the top of `flowchart/mod.rs` with the same `// TODO(slice2): removed in Task 14` comment.

- [ ] **Step 3: Implement `measure.rs`**

```rust
pub(crate) const FONT_PX: f64 = 14.0;
pub(crate) const LINE_H: f64 = 19.0;

/// Relative advance width per char class, multiplied by FONT_PX.
fn char_w(c: char) -> f64 {
    match c {
        'i' | 'l' | 'j' | 't' | 'f' | 'r' | '.' | ',' | ':' | ';' | '!'
        | '|' | '\'' | '`' | ' ' | '(' | ')' | '[' | ']' => 0.45,
        'm' | 'w' | 'M' | 'W' | '@' | '%' => 0.95,
        'A'..='Z' | '0'..='9' | '#' | '&' | '$' => 0.72,
        c if (c as u32) > 0x2E7F => 1.05, // CJK & wide scripts
        _ => 0.58,
    }
}

fn split_br(s: &str) -> Vec<&str> {
    // Accept <br/>, <br>, <br /> — case-insensitive is overkill; mermaid
    // docs use lowercase.
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        let hit = ["<br/>", "<br />", "<br>"]
            .iter()
            .filter_map(|t| rest.find(t).map(|i| (i, t.len())))
            .min();
        match hit {
            Some((i, tl)) => {
                out.push(&rest[..i]);
                rest = &rest[i + tl..];
            }
            None => {
                out.push(rest);
                return out;
            }
        }
    }
}

pub(crate) fn text_size(s: &str) -> (f64, f64) {
    let lines = split_br(s);
    let w = lines
        .iter()
        .map(|l| l.chars().map(char_w).sum::<f64>() * FONT_PX)
        .fold(0.0, f64::max);
    (w, lines.len() as f64 * LINE_H)
}

/// Lines after <br/> splitting — svg.rs emits one tspan per line.
pub(crate) fn lines(s: &str) -> Vec<&str> {
    split_br(s)
}
```

- [ ] **Step 4: Move `escape_xml` to `lib.rs`**

In `lib.rs`, add (near the top, below the type definitions):

```rust
/// XML-escape a user-supplied string before interpolating into SVG.
/// Order matters: `&` first so earlier escapes aren't double-escaped.
pub(crate) fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
```

In `pie.rs`: delete its private `escape_xml` and add `use crate::escape_xml;`. The pie tests referencing `escape_xml` behavior (e.g. `svg_escapes_label_markup`) must stay green unchanged.

- [ ] **Step 5: Run** — `cargo test -p ogrenotes-mermaid` → all green (pie + layout + measure).

- [ ] **Step 6: Commit**

```bash
git add crates/mermaid/src/flowchart/mod.rs crates/mermaid/src/flowchart/measure.rs crates/mermaid/src/lib.rs crates/mermaid/src/pie.rs
git commit -m "feat(mermaid): flowchart model, shared escape_xml, text measurement

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 10: parser core — header, nodes, shapes, edge chains, labels

**Files:**
- Create: `crates/mermaid/src/flowchart/parse.rs`
- Modify: `crates/mermaid/src/flowchart/mod.rs` (add `pub(crate) mod parse;`)

**Interfaces:**
- Consumes: model types (Task 9), `crate::ParseError`.
- Produces: `pub(crate) fn parse(source: &str) -> Result<FlowGraph, ParseError>` — Task 10 covers: header (`graph`/`flowchart` + optional direction, default TD), node statements with ALL 13 shape brackets, edge chains with the 4 edge kinds and both label forms, `&` fan-out, `%%` comments, blank lines, `;` statement separators, quoted labels. Subgraphs/classDef/`:::`/out-of-scope statements are Task 11 (until then any such line is a generic "unsupported statement" error — Task 11 refines).

**Parsing strategy (write exactly this):** line-oriented; each line trimmed, `;`-split into statements. A statement is either a keyword statement (Task 11) or a **chain statement**: `noderef (edgeop noderef)*` where `noderef` = `group ('&' group)*` — each `group` is `id bracket?` with `:::class` suffix (Task 11). Edge ops and brackets are matched by a longest-first token scan — no regexes (std only).

**Bracket table** (longest opener first; parallelogram/trapezoid share openers and are disambiguated by CLOSER):

| opener | closer | shape |
|---|---|---|
| `(((` | `)))` | DoubleCircle |
| `((` | `))` | Circle |
| `([` | `])` | Stadium |
| `[(` | `)]` | Cylinder |
| `[/` | `/]` | Parallelogram |
| `[/` | `\]` | Trapezoid |
| `[\` | `\]` | ParallelogramRev |
| `[\` | `/]` | TrapezoidRev |
| `{{` | `}}` | Hexagon |
| `{` | `}` | Diamond |
| `[` | `]` | Rect |
| `(` | `)` | Rounded |
| `>` | `]` | Flag |

**Edge-op table** (longest first): `-.->` Dotted, `-.-` Dotted, `==>` Thick, `===` Thick, `-->` Arrow, `---` Open. Inline label forms: `-- text -->` (Arrow), `-. text .->` (Dotted), `== text ==>` (Thick); post-op form: `-->|text|`. Node ids: `[A-Za-z0-9_]+` (`-` excluded: it would be ambiguous with edge ops mid-scan; mermaid ids in practice are word-like).

- [ ] **Step 1: Write the failing parser tests**

Create `crates/mermaid/src/flowchart/parse.rs` with this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::flowchart::{EdgeKind, ShapeKind};
    use crate::layout::Direction;

    fn p(src: &str) -> crate::flowchart::FlowGraph {
        parse(src).expect("parse ok")
    }

    #[test]
    fn header_directions() {
        assert_eq!(p("graph TD\nA").direction, Direction::TB);
        assert_eq!(p("graph TB\nA").direction, Direction::TB);
        assert_eq!(p("flowchart LR\nA").direction, Direction::LR);
        assert_eq!(p("graph RL\nA").direction, Direction::RL);
        assert_eq!(p("graph BT\nA").direction, Direction::BT);
        assert_eq!(p("graph\nA").direction, Direction::TB); // default
    }

    #[test]
    fn missing_header_is_line_error() {
        let e = parse("A --> B").unwrap_err();
        assert_eq!(e.line, Some(1));
    }

    #[test]
    fn all_shapes_parse() {
        let cases = [
            ("A[text]", ShapeKind::Rect),
            ("A(text)", ShapeKind::Rounded),
            ("A([text])", ShapeKind::Stadium),
            ("A((text))", ShapeKind::Circle),
            ("A(((text)))", ShapeKind::DoubleCircle),
            ("A{text}", ShapeKind::Diamond),
            ("A{{text}}", ShapeKind::Hexagon),
            ("A[/text/]", ShapeKind::Parallelogram),
            ("A[\\text\\]", ShapeKind::ParallelogramRev),
            ("A[/text\\]", ShapeKind::Trapezoid),
            ("A[\\text/]", ShapeKind::TrapezoidRev),
            ("A[(text)]", ShapeKind::Cylinder),
            ("A>text]", ShapeKind::Flag),
        ];
        for (src, want) in cases {
            let g = p(&format!("graph TD\n{src}"));
            assert_eq!(g.nodes[0].shape, want, "for {src}");
            assert_eq!(g.nodes[0].label, "text", "for {src}");
        }
    }

    #[test]
    fn bare_id_defaults_rect_with_id_label() {
        let g = p("graph TD\nfoo_1");
        assert_eq!(g.nodes[0].id, "foo_1");
        assert_eq!(g.nodes[0].label, "foo_1");
        assert_eq!(g.nodes[0].shape, ShapeKind::Rect);
    }

    #[test]
    fn quoted_label_strips_quotes() {
        let g = p("graph TD\nA[\"has [brackets] inside\"]");
        assert_eq!(g.nodes[0].label, "has [brackets] inside");
    }

    #[test]
    fn edge_kinds() {
        let cases = [
            ("A --> B", EdgeKind::Arrow),
            ("A --- B", EdgeKind::Open),
            ("A -.-> B", EdgeKind::Dotted),
            ("A ==> B", EdgeKind::Thick),
        ];
        for (src, want) in cases {
            let g = p(&format!("graph TD\n{src}"));
            assert_eq!(g.edges[0].kind, want, "for {src}");
        }
    }

    #[test]
    fn pipe_label() {
        let g = p("graph TD\nA -->|yes| B");
        assert_eq!(g.edges[0].label.as_deref(), Some("yes"));
    }

    #[test]
    fn inline_label() {
        let g = p("graph TD\nA -- no --> B");
        assert_eq!(g.edges[0].label.as_deref(), Some("no"));
        let g = p("graph TD\nA -. maybe .-> B");
        assert_eq!(g.edges[0].label.as_deref(), Some("maybe"));
        assert_eq!(g.edges[0].kind, EdgeKind::Dotted);
    }

    #[test]
    fn chains_create_all_edges() {
        let g = p("graph TD\nA --> B --> C --> D");
        assert_eq!(g.edges.len(), 3);
        assert_eq!(g.nodes.len(), 4);
        assert_eq!((g.edges[1].from, g.edges[1].to), (1, 2));
    }

    #[test]
    fn ampersand_fanout() {
        let g = p("graph TD\nA & B --> C");
        assert_eq!(g.edges.len(), 2);
        let g = p("graph TD\nA --> B & C");
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn inline_shape_in_chain() {
        let g = p("graph TD\nA[Start] --> B{Choice}");
        assert_eq!(g.nodes[1].shape, ShapeKind::Diamond);
    }

    #[test]
    fn later_bracket_updates_earlier_bare_ref() {
        let g = p("graph TD\nA --> B\nB{Decide}");
        assert_eq!(g.nodes[1].shape, ShapeKind::Diamond);
        assert_eq!(g.nodes[1].label, "Decide");
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn comments_blanks_and_semicolons() {
        let g = p("graph TD\n%% a comment\n\nA --> B; B --> C;");
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn self_loop_parses() {
        let g = p("graph TD\nA --> A");
        assert_eq!((g.edges[0].from, g.edges[0].to), (0, 0));
    }

    #[test]
    fn unclosed_bracket_is_line_error() {
        let e = parse("graph TD\nA[oops").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn garbage_after_node_is_line_error() {
        let e = parse("graph TD\nA[ok] ???").unwrap_err();
        assert_eq!(e.line, Some(2));
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p ogrenotes-mermaid flowchart::parse` → FAIL.

- [ ] **Step 3: Implement the parser core**

Structure (write exactly this shape; ~250 lines):

```rust
//! Flowchart parser. Line-oriented; each line splits on `;` into
//! statements; chain statements are scanned left-to-right with
//! longest-first token matching (std only, no regex).

use crate::flowchart::{EdgeKind, FlowEdge, FlowGraph, FlowNode, ShapeKind};
use crate::layout::Direction;
use crate::ParseError;
use std::collections::HashMap;

struct Parser {
    g: FlowGraph,
    ids: HashMap<String, usize>,
    line: usize, // 1-based, for errors
}

pub(crate) fn parse(source: &str) -> Result<FlowGraph, ParseError> {
    let mut p = Parser {
        g: FlowGraph {
            direction: Direction::TB,
            nodes: vec![],
            edges: vec![],
            subgraphs: vec![],
            class_defs: vec![],
        },
        ids: HashMap::new(),
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
            p.parse_header(line)?;
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
            message: "flowchart must start with `graph` or `flowchart`".into(),
            line: Some(1),
        });
    }
    Ok(p.g)
}

impl Parser {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { message: msg.into(), line: Some(self.line) }
    }

    fn parse_header(&mut self, line: &str) -> Result<(), ParseError> {
        let mut toks = line.split_whitespace();
        match toks.next() {
            Some("graph") | Some("flowchart") => {}
            _ => return Err(self.err("flowchart must start with `graph` or `flowchart`")),
        }
        self.g.direction = match toks.next() {
            None | Some("TD") | Some("TB") => Direction::TB,
            Some("BT") => Direction::BT,
            Some("LR") => Direction::LR,
            Some("RL") => Direction::RL,
            Some(other) => return Err(self.err(format!("unknown direction {other:?}"))),
        };
        Ok(())
    }

    fn parse_statement(&mut self, stmt: &str) -> Result<(), ParseError> {
        // Task 11 adds keyword statements here (subgraph/end/classDef/
        // class + explicit out-of-scope errors). Task 10: chains only.
        self.parse_chain(stmt)
    }

    /// `noderef (edgeop noderef)*` where noderef = group ('&' group)*.
    fn parse_chain(&mut self, stmt: &str) -> Result<(), ParseError> {
        let mut rest = stmt;
        let mut lhs = self.parse_node_group(&mut rest)?;
        loop {
            let r = rest.trim_start();
            if r.is_empty() {
                return Ok(());
            }
            rest = r;
            let (kind, label) = self.parse_edge_op(&mut rest)?;
            let rhs = self.parse_node_group(&mut rest)?;
            for &f in &lhs {
                for &t in &rhs {
                    self.g.edges.push(FlowEdge { from: f, to: t, kind, label: label.clone() });
                }
            }
            lhs = rhs;
        }
    }

    /// One or more `id bracket?` joined by `&`.
    fn parse_node_group(&mut self, rest: &mut &str) -> Result<Vec<usize>, ParseError> {
        let mut out = vec![self.parse_node_ref(rest)?];
        loop {
            let r = rest.trim_start();
            if let Some(after) = r.strip_prefix('&') {
                *rest = after.trim_start();
                out.push(self.parse_node_ref(rest)?);
            } else {
                *rest = r;
                return Ok(out);
            }
        }
    }

    fn parse_node_ref(&mut self, rest: &mut &str) -> Result<usize, ParseError> {
        let r = rest.trim_start();
        let id_len = r.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
        if id_len == 0 {
            return Err(self.err(format!("expected a node id, found {r:?}")));
        }
        let id: String = r[..id_len].to_string();
        let mut after = &r[id_len..];
        let shape_label = self.try_parse_bracket(&mut after)?;
        *rest = after;
        let idx = match self.ids.get(&id) {
            Some(&i) => i,
            None => {
                let i = self.g.nodes.len();
                self.g.nodes.push(FlowNode {
                    id: id.clone(),
                    label: id.clone(),
                    shape: ShapeKind::Rect,
                    classes: vec![],
                    subgraph: None, // Task 11 sets from subgraph stack
                });
                self.ids.insert(id, i);
                i
            }
        };
        if let Some((shape, label)) = shape_label {
            self.g.nodes[idx].shape = shape;
            self.g.nodes[idx].label = label;
        }
        Ok(idx)
    }

    /// Longest-first bracket match. Returns None if no opener follows.
    fn try_parse_bracket(
        &self,
        rest: &mut &str,
    ) -> Result<Option<(ShapeKind, String)>, ParseError> {
        // (opener, &[(closer, shape)]) — closers tried longest-first.
        const TABLE: &[(&str, &[(&str, ShapeKind)])] = &[
            ("(((", &[(")))", ShapeKind::DoubleCircle)]),
            ("((", &[("))", ShapeKind::Circle)]),
            ("([", &[("])", ShapeKind::Stadium)]),
            ("[(", &[(")]", ShapeKind::Cylinder)]),
            ("[/", &[("/]", ShapeKind::Parallelogram), ("\\]", ShapeKind::Trapezoid)]),
            ("[\\", &[("\\]", ShapeKind::ParallelogramRev), ("/]", ShapeKind::TrapezoidRev)]),
            ("{{", &[("}}", ShapeKind::Hexagon)]),
            ("{", &[("}", ShapeKind::Diamond)]),
            ("[", &[("]", ShapeKind::Rect)]),
            ("(", &[(")", ShapeKind::Rounded)]),
            (">", &[("]", ShapeKind::Flag)]),
        ];
        for (opener, closers) in TABLE {
            if let Some(body_start) = rest.strip_prefix(opener) {
                // Find the EARLIEST closer occurrence among this opener's
                // closers; the closer that matches at that position wins.
                let mut best: Option<(usize, &str, ShapeKind)> = None;
                for (closer, shape) in *closers {
                    if let Some(i) = body_start.find(closer) {
                        if best.map_or(true, |(bi, _, _)| i < bi) {
                            best = Some((i, closer, *shape));
                        }
                    }
                }
                let Some((i, closer, shape)) = best else {
                    return Err(self.err(format!("unclosed {opener:?} bracket")));
                };
                let body = &body_start[..i];
                *rest = &body_start[i + closer.len()..];
                let label = body.trim();
                let label = label
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(label);
                return Ok(Some((shape, label.to_string())));
            }
        }
        Ok(None)
    }

    /// Edge operator with optional label (inline or |pipe| form).
    fn parse_edge_op(&mut self, rest: &mut &str) -> Result<(EdgeKind, Option<String>), ParseError> {
        let r = rest.trim_start();
        // Inline label forms first: `-- text -->`, `-. text .->`, `== text ==>`.
        for (open, close, kind) in [
            ("--", "-->", EdgeKind::Arrow),
            ("-.", ".->", EdgeKind::Dotted),
            ("==", "==>", EdgeKind::Thick),
        ] {
            if let Some(after_open) = r.strip_prefix(open) {
                // Inline form requires a space after the opener (else it's
                // the plain operator like `-->` sharing the prefix).
                if after_open.starts_with(' ') {
                    if let Some(i) = after_open.find(close) {
                        let label = after_open[..i].trim().to_string();
                        *rest = &after_open[i + close.len()..];
                        return Ok((kind, Some(label)));
                    }
                }
            }
        }
        // Plain operators, longest first.
        for (op, kind) in [
            ("-.->", EdgeKind::Dotted),
            ("-.-", EdgeKind::Dotted),
            ("==>", EdgeKind::Thick),
            ("===", EdgeKind::Thick),
            ("-->", EdgeKind::Arrow),
            ("---", EdgeKind::Open),
        ] {
            if let Some(after) = r.strip_prefix(op) {
                let mut rest2 = after;
                // Optional |label|.
                let label = {
                    let r2 = rest2.trim_start();
                    if let Some(after_pipe) = r2.strip_prefix('|') {
                        let Some(i) = after_pipe.find('|') else {
                            return Err(self.err("unclosed `|` edge label"));
                        };
                        let l = after_pipe[..i].trim().to_string();
                        rest2 = &after_pipe[i + 1..];
                        Some(l)
                    } else {
                        None
                    }
                };
                *rest = rest2;
                return Ok((kind, label));
            }
        }
        Err(self.err(format!("expected an edge (e.g. `-->`), found {r:?}")))
    }
}
```

Add `pub(crate) mod parse;` to `flowchart/mod.rs`.

- [ ] **Step 4: Run tests** — `cargo test -p ogrenotes-mermaid flowchart::parse` → PASS (16 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/flowchart/parse.rs crates/mermaid/src/flowchart/mod.rs
git commit -m "feat(mermaid): flowchart parser core — shapes, edges, chains, labels

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 11: parser — subgraphs, classDef/class/`:::`, out-of-scope errors

**Files:**
- Modify: `crates/mermaid/src/flowchart/parse.rs`

**Interfaces:**
- Consumes/extends Task 10's `Parser`.
- Produces: full `parse()` coverage per the spec — `subgraph id[title]` / `subgraph title` / nested / `end`; `classDef name k:v,k:v`; `class n1,n2 name`; `:::name` suffix on node refs; explicit per-line errors for `click`, `linkStyle`, `style`, `accTitle`, `accDescr`, and `direction` (inside subgraphs AND at top level — the header is the only direction source).

**Behavioral decisions (write exactly these):**
- Subgraph membership: a node joins the subgraph on top of the stack **when first created** (`FlowNode.subgraph`); an existing node referenced inside a subgraph does NOT move (mermaid.js behaves this way).
- `end` with an empty stack → error "found `end` outside a subgraph".
- EOF with a non-empty stack → error "unclosed subgraph" with the OPENING line number (track it on a stack of `(index, line)`).
- `classDef` styles are sanitized: split on `,`, each `prop:value`; property must be in `["fill", "stroke", "stroke-width", "stroke-dasharray", "color", "font-weight", "font-style", "opacity"]` and value chars in `[A-Za-z0-9# .,%-]`; **non-allowlisted properties are dropped silently** (styling is cosmetic; erroring on mermaid's huge style vocabulary would be hostile — the allowlist is the XSS/CSS-injection boundary, documented in a comment).
- `class` / `:::` referencing an UNDEFINED class name is allowed (mermaid tolerates it; the class just has no styles — emit nothing for it).
- Subgraph title: `subgraph ident` → id=ident title=ident; `subgraph ident [Title text]` or `subgraph ident[Title text]` → title from brackets (quoted strip applies); `subgraph Title with spaces` (no bracket, multiple tokens) → id = first token, title = whole rest… NO: mermaid uses the whole text as both. DECISION: tokens after `subgraph`: if the remainder contains `[`, split id/title at it; otherwise the WHOLE trimmed remainder is both id and title.

- [ ] **Step 1: Write the failing tests** (append to the test module in `parse.rs`)

```rust
    #[test]
    fn subgraph_membership_and_title() {
        let g = p("graph TD\nsubgraph one[Group One]\nA --> B\nend\nC --> A");
        assert_eq!(g.subgraphs.len(), 1);
        assert_eq!(g.subgraphs[0].title, "Group One");
        assert_eq!(g.nodes[0].subgraph, Some(0)); // A created inside
        assert_eq!(g.nodes[1].subgraph, Some(0)); // B created inside
        let c = g.nodes.iter().find(|n| n.id == "C").unwrap();
        assert_eq!(c.subgraph, None);
    }

    #[test]
    fn subgraph_without_bracket_title() {
        let g = p("graph TD\nsubgraph My Group\nA\nend");
        assert_eq!(g.subgraphs[0].title, "My Group");
        assert_eq!(g.subgraphs[0].id, "My Group");
    }

    #[test]
    fn nested_subgraphs() {
        let g = p("graph TD\nsubgraph outer\nsubgraph inner\nA\nend\nB\nend");
        assert_eq!(g.subgraphs.len(), 2);
        assert_eq!(g.subgraphs[1].parent, Some(0));
        assert_eq!(g.nodes[0].subgraph, Some(1)); // A in inner
        assert_eq!(g.nodes[1].subgraph, Some(0)); // B in outer
    }

    #[test]
    fn existing_node_does_not_move_into_subgraph() {
        let g = p("graph TD\nA\nsubgraph s\nA --> B\nend");
        assert_eq!(g.nodes[0].subgraph, None);
        assert_eq!(g.nodes[1].subgraph, Some(0));
    }

    #[test]
    fn end_without_subgraph_errors() {
        let e = parse("graph TD\nend").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn unclosed_subgraph_errors_at_opening_line() {
        let e = parse("graph TD\nA\nsubgraph s\nB").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("unclosed subgraph"));
    }

    #[test]
    fn class_def_and_assignment() {
        let g = p("graph TD\nA\nB\nclassDef hot fill:#f00,stroke-width:2px\nclass A,B hot");
        assert_eq!(g.class_defs.len(), 1);
        assert!(g.class_defs[0].style.contains("fill:#f00"));
        assert!(g.class_defs[0].style.contains("stroke-width:2px"));
        assert_eq!(g.nodes[0].classes, vec!["hot"]);
        assert_eq!(g.nodes[1].classes, vec!["hot"]);
    }

    #[test]
    fn inline_class_suffix() {
        let g = p("graph TD\nclassDef hot fill:#f00\nA[Hi]:::hot --> B");
        assert_eq!(g.nodes[0].classes, vec!["hot"]);
        assert!(g.nodes[1].classes.is_empty());
    }

    #[test]
    fn class_def_sanitizes_disallowed_properties() {
        let g = p("graph TD\nA\nclassDef bad background-image:url(x),fill:#0f0");
        // Disallowed property dropped; allowlisted one kept.
        assert!(!g.class_defs[0].style.contains("url"));
        assert!(g.class_defs[0].style.contains("fill:#0f0"));
    }

    #[test]
    fn class_def_rejects_hostile_value_chars() {
        let g = p("graph TD\nA\nclassDef x fill:#0f0;evil");
        // `;` splits statements, so `evil` becomes a separate (bare-node)
        // statement — fill survives, no injection into the style string.
        assert!(g.class_defs[0].style.contains("fill:#0f0"));
        assert!(!g.class_defs[0].style.contains("evil"));
    }

    #[test]
    fn out_of_scope_statements_error_with_line() {
        for stmt in ["click A callback", "linkStyle 0 stroke:red", "style A fill:#f00",
                     "accTitle: x", "accDescr: y", "direction LR"] {
            let src = format!("graph TD\nA\n{stmt}");
            let e = parse(&src).unwrap_err();
            assert_eq!(e.line, Some(3), "for {stmt}");
            let kw = stmt.split([' ', ':']).next().unwrap();
            assert!(e.message.contains(kw), "message names the keyword: {}", e.message);
        }
    }

    #[test]
    fn direction_inside_subgraph_errors() {
        let e = parse("graph TD\nsubgraph s\ndirection LR\nend").unwrap_err();
        assert_eq!(e.line, Some(3));
    }
```

- [ ] **Step 2: Run to verify failure** — the subgraph/class tests FAIL (statements parse as chains or error wrongly).

- [ ] **Step 3: Implement**

Extend `Parser` with `stack: Vec<(usize, usize)>` (subgraph index, opening line) and rewrite `parse_statement`:

```rust
    fn parse_statement(&mut self, stmt: &str) -> Result<(), ParseError> {
        let first = stmt.split_whitespace().next().unwrap_or("");
        match first {
            "subgraph" => return self.parse_subgraph_open(stmt),
            "end" if stmt == "end" => return self.parse_subgraph_end(),
            "classDef" => return self.parse_class_def(stmt),
            "class" => return self.parse_class_assign(stmt),
            "click" | "linkStyle" | "style" | "direction" => {
                return Err(self.err(format!("`{first}` statements are not supported")));
            }
            _ if stmt.starts_with("accTitle") || stmt.starts_with("accDescr") => {
                let kw = if stmt.starts_with("accTitle") { "accTitle" } else { "accDescr" };
                return Err(self.err(format!("`{kw}` statements are not supported")));
            }
            _ => {}
        }
        self.parse_chain(stmt)
    }
```

`parse_subgraph_open`: remainder after `subgraph `; if it contains `[`, split at the first `[`, trim id, closer `]` required (else error), quoted-strip title; otherwise id = title = whole trimmed remainder (error if empty). Push `(index, self.line)`; `parent` = current stack top's index before pushing.

`parse_subgraph_end`: pop or error "found `end` outside a subgraph".

At the end of `parse()` (before `Ok`): if the stack is non-empty, return "unclosed subgraph `<id>`" with the recorded opening line.

In `parse_node_ref`: after creating a NEW node set `subgraph: self.stack.last().map(|&(i, _)| i)`; also, after the bracket parse, check for a `:::name` suffix:

```rust
        // Optional :::className suffix (after any bracket).
        let r2 = after.trim_start();
        let mut classes = Vec::new();
        let mut after2 = after;
        if let Some(rest_c) = r2.strip_prefix(":::") {
            let n = rest_c.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
            if n == 0 {
                return Err(self.err("expected a class name after `:::`"));
            }
            classes.push(rest_c[..n].to_string());
            after2 = &rest_c[n..];
        }
```

and push those classes onto the node (dedup not needed).

`parse_class_def` (the sanitizing boundary — keep this comment in the code):

```rust
    /// The style allowlist is the CSS-injection boundary: only these
    /// properties, and only benign value characters, survive into the
    /// emitted `style` attribute. Everything else is dropped silently —
    /// styling is cosmetic and mermaid's style vocabulary is huge, so
    /// erroring here would be hostile to real-world diagrams.
    const STYLE_PROPS: &[&str] = &[
        "fill", "stroke", "stroke-width", "stroke-dasharray",
        "color", "font-weight", "font-style", "opacity",
    ];

    fn parse_class_def(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("classDef").unwrap().trim();
        let Some((name, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("classDef needs a name and styles"));
        };
        let mut kept = Vec::new();
        for pair in styles.split(',') {
            let Some((prop, value)) = pair.split_once(':') else { continue };
            let (prop, value) = (prop.trim(), value.trim());
            let value_ok = value.chars().all(|c| {
                c.is_ascii_alphanumeric() || " #.,%-".contains(c)
            });
            if Self::STYLE_PROPS.contains(&prop) && value_ok && !value.is_empty() {
                kept.push(format!("{prop}:{value}"));
            }
        }
        self.g.class_defs.push(crate::flowchart::ClassDef {
            name: name.to_string(),
            style: kept.join(";"),
        });
        Ok(())
    }
```

`parse_class_assign`: `class n1,n2 name` — split remainder on last whitespace token as the class name, comma-split the node list, node ids must exist (error names the missing id).

- [ ] **Step 4: Run all parser tests** — `cargo test -p ogrenotes-mermaid flowchart::parse` → PASS (28 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/flowchart/parse.rs
git commit -m "feat(mermaid): flowchart parser — subgraphs, classes, scope errors

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 12: shape library (`shapes.rs`)

**Files:**
- Create: `crates/mermaid/src/flowchart/shapes.rs`
- Modify: `crates/mermaid/src/flowchart/mod.rs` (add `pub(crate) mod shapes;`)

**Interfaces:**
- Consumes: `ShapeKind` (Task 9).
- Produces:
  - `pub(crate) fn size_for(shape: ShapeKind, text_w: f64, text_h: f64) -> (f64, f64)` — the node's layout footprint for a given label size (per-shape padding; diamond/hexagon inscribe the label so they inflate more).
  - `pub(crate) fn emit(shape: ShapeKind, cx: f64, cy: f64, w: f64, h: f64) -> String` — the shape's SVG geometry ONLY (no label; `svg.rs` overlays text). Every emitter uses `fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor"` so classDef fills can override via a style attr on the wrapping `<g>` and the theme drives strokes/text.

**Geometry table (write exactly these):**

| shape | size_for (w, h) | emit |
|---|---|---|
| Rect | `(tw+24, th+16)` | `<rect x y w h/>` |
| Rounded | `(tw+24, th+16)` | `<rect rx="8"/>` |
| Stadium | `(tw+32, th+16)` | `<rect rx="{h/2}"/>` |
| Circle | `d = max(tw,th)+28`, `(d, d)` | `<circle r="{d/2}"/>` |
| DoubleCircle | `d = max(tw,th)+36` | two `<circle>`s, radii `d/2` and `d/2-5` |
| Diamond | `(tw*1.7+24, th*2.2+12)` | `<polygon>` of the 4 midpoints |
| Hexagon | `(tw+48, th+16)` | 6-point polygon, 25%-width side cuts |
| Parallelogram | `(tw+44, th+16)` | 4-point polygon, +15px x-skew (top edge shifted right) |
| ParallelogramRev | same | skew mirrored |
| Trapezoid | `(tw+52, th+16)` | top edge inset 15px both sides |
| TrapezoidRev | same | bottom edge inset |
| Cylinder | `(tw+28, th+28)` | `<path>`: side walls + bottom arc + top `<ellipse>` (ry 7) |
| Flag | `(tw+36, th+16)` | 5-point polygon: rectangular right side, `>` notch on the left |

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::flowchart::ShapeKind;

    const ALL: &[ShapeKind] = &[
        ShapeKind::Rect, ShapeKind::Rounded, ShapeKind::Stadium,
        ShapeKind::Circle, ShapeKind::DoubleCircle, ShapeKind::Diamond,
        ShapeKind::Hexagon, ShapeKind::Parallelogram, ShapeKind::ParallelogramRev,
        ShapeKind::Trapezoid, ShapeKind::TrapezoidRev, ShapeKind::Cylinder,
        ShapeKind::Flag,
    ];

    #[test]
    fn every_shape_fits_its_text() {
        for &s in ALL {
            let (w, h) = size_for(s, 80.0, 19.0);
            assert!(w >= 80.0 && h >= 19.0, "{s:?} smaller than its text");
        }
    }

    #[test]
    fn diamond_inflates_more_than_rect() {
        assert!(size_for(ShapeKind::Diamond, 80.0, 19.0).0
            > size_for(ShapeKind::Rect, 80.0, 19.0).0);
    }

    #[test]
    fn circle_is_square_footprint() {
        let (w, h) = size_for(ShapeKind::Circle, 60.0, 19.0);
        assert_eq!(w, h);
    }

    #[test]
    fn every_shape_emits_geometry() {
        for &s in ALL {
            let svg = emit(s, 100.0, 50.0, 120.0, 40.0);
            assert!(
                svg.contains("<rect") || svg.contains("<circle")
                    || svg.contains("<polygon") || svg.contains("<path")
                    || svg.contains("<ellipse"),
                "{s:?} emitted no geometry: {svg}"
            );
            assert!(svg.contains("currentColor"), "{s:?} not theme-aware");
            assert!(!svg.contains("NaN"), "{s:?} produced NaN");
        }
    }

    #[test]
    fn double_circle_has_two_circles() {
        let svg = emit(ShapeKind::DoubleCircle, 0.0, 0.0, 60.0, 60.0);
        assert_eq!(svg.matches("<circle").count(), 2);
    }

    #[test]
    fn polygon_shapes_have_expected_point_counts() {
        let poly_points = |s: ShapeKind| {
            let svg = emit(s, 0.0, 0.0, 100.0, 40.0);
            let pts = svg.split("points=\"").nth(1).unwrap()
                .split('"').next().unwrap();
            pts.split_whitespace().count()
        };
        assert_eq!(poly_points(ShapeKind::Diamond), 4);
        assert_eq!(poly_points(ShapeKind::Hexagon), 6);
        assert_eq!(poly_points(ShapeKind::Parallelogram), 4);
        assert_eq!(poly_points(ShapeKind::Trapezoid), 4);
        assert_eq!(poly_points(ShapeKind::Flag), 5);
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p ogrenotes-mermaid flowchart::shapes` → FAIL.

- [ ] **Step 3: Implement** per the geometry table. Representative emitters (repeat the pattern for all 13; every number comes from the table):

```rust
use crate::flowchart::ShapeKind;

pub(crate) fn size_for(shape: ShapeKind, tw: f64, th: f64) -> (f64, f64) {
    match shape {
        ShapeKind::Rect | ShapeKind::Rounded => (tw + 24.0, th + 16.0),
        ShapeKind::Stadium => (tw + 32.0, th + 16.0),
        ShapeKind::Circle => {
            let d = tw.max(th) + 28.0;
            (d, d)
        }
        ShapeKind::DoubleCircle => {
            let d = tw.max(th) + 36.0;
            (d, d)
        }
        ShapeKind::Diamond => (tw * 1.7 + 24.0, th * 2.2 + 12.0),
        ShapeKind::Hexagon => (tw + 48.0, th + 16.0),
        ShapeKind::Parallelogram | ShapeKind::ParallelogramRev => (tw + 44.0, th + 16.0),
        ShapeKind::Trapezoid | ShapeKind::TrapezoidRev => (tw + 52.0, th + 16.0),
        ShapeKind::Cylinder => (tw + 28.0, th + 28.0),
        ShapeKind::Flag => (tw + 36.0, th + 16.0),
    }
}

const FILL: &str = "var(--mermaid-node-fill, #ececff)";

pub(crate) fn emit(shape: ShapeKind, cx: f64, cy: f64, w: f64, h: f64) -> String {
    let (x, y) = (cx - w / 2.0, cy - h / 2.0);
    let common = format!(r#"fill="{FILL}" stroke="currentColor" stroke-width="1""#);
    match shape {
        ShapeKind::Rect => format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" {common}/>"#
        ),
        ShapeKind::Rounded => format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="8" {common}/>"#
        ),
        ShapeKind::Diamond => {
            let pts = format!(
                "{cx:.1},{y:.1} {:.1},{cy:.1} {cx:.1},{:.1} {:.1},{cy:.1}",
                x + w, y + h, x
            );
            format!(r#"<polygon points="{pts}" {common}/>"#)
        }
        // ... Stadium (rx = h/2), Circle, DoubleCircle (two circles),
        // Hexagon (cut = w*0.25), Parallelogram/Rev (skew 15.0),
        // Trapezoid/Rev (inset 15.0), Cylinder (path + ellipse),
        // Flag (notch depth 12.0) — same structure, numbers from the table.
        _ => unimplemented!("filled in for every variant by this task"),
    }
}
```

(The `unimplemented!` is scaffolding DURING the task only — the task is not
complete while any arm remains; the match must be exhaustive with all 13 real
arms and no wildcard when committed. The `every_shape_emits_geometry` test
enforces this: `unimplemented!` panics.)

- [ ] **Step 4: Run tests** — `cargo test -p ogrenotes-mermaid flowchart::shapes` → PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/flowchart/shapes.rs crates/mermaid/src/flowchart/mod.rs
git commit -m "feat(mermaid): flowchart shape library (13 shapes)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 13: SVG assembly + flowchart pipeline (`svg.rs`, `render_flowchart`)

**Files:**
- Create: `crates/mermaid/src/flowchart/svg.rs`
- Modify: `crates/mermaid/src/flowchart/mod.rs` (add `pub(crate) mod svg;` + `render_flowchart`)

**Interfaces:**
- Consumes: everything from Tasks 1–12.
- Produces:
  - `flowchart/mod.rs`: `pub(crate) fn render_flowchart(source: &str) -> Result<String, crate::ParseError>` — parse → measure (`shapes::size_for(node.shape, text_size(label))`; subgraph title sizes via `text_size`; edge label sizes via `text_size` + 8px pad) → build `LayoutInput` (map `FlowNode.subgraph` → `LNode.cluster`, `FlowSubgraph.parent` → `LCluster.parent`) → `layout::run` (a layout `Err(String)` becomes `ParseError { message, line: None }`) → `svg::emit`.
  - `svg.rs`: `pub(crate) fn emit(g: &FlowGraph, l: &crate::layout::Layout) -> String`.

**SVG document structure (write exactly this order — z-order matters):**
1. `<svg xmlns viewBox="0 0 {W} {H}" width height style="font-family:sans-serif;font-size:14px">`
2. `<defs>` — ONE arrowhead marker: `<marker id="mmd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker>`
3. Cluster rects (OUTERMOST FIRST so children paint on top): `<rect>` with `fill="var(--mermaid-cluster-fill, #7773)" stroke="currentColor" stroke-dasharray="4 2" rx="4"` + `<text>` title at the rect's top strip, `text-anchor="middle"`, `fill="currentColor"`, `font-weight="600"`. Ordering: sort cluster indices by tree depth ascending (parents first).
4. Edges: one `<path d="M x0 y0 L x1 y1 …">` per `EdgePath` (polyline points; rounded corners deferred — straight segments are correct and the corner-rounding refinement is inside route.rs already if present). Attributes per kind: Arrow `stroke="currentColor" fill="none" marker-end="url(#mmd-arrow)"`; Open — same minus marker; Dotted adds `stroke-dasharray="3 3"` + marker; Thick `stroke-width="2.5"` + marker. Edge labels: `<text>` at `label_at` with `text-anchor="middle"`, plus a `<rect>` behind sized `text_size(label)+4px` with `fill="var(--surface, #fff)"` so the label masks the line under it.
5. Nodes: `<g>` per node — `shapes::emit(...)` + label `<text x="{cx}" text-anchor="middle" fill="currentColor">` with one `<tspan x="{cx}" dy="...">` per `measure::lines()` line (first dy centers the block: `cy - (n-1)/2*LINE_H + 4.0`, subsequent `dy="{LINE_H}"`). If the node's classes resolve to a non-empty `ClassDef.style`, put `style="{style}"` on the `<g>` (styles were sanitized at parse; still escape with `escape_xml` — defense in depth).
6. `</svg>`.

**Escaping rule:** every label/title string goes through `crate::escape_xml`. The style attr also goes through `escape_xml` (allowlist already excludes `"` and `<`, this is belt-and-braces).

- [ ] **Step 1: Write the failing tests** (in `svg.rs`, driving through `render_flowchart` so the whole pipeline is exercised)

```rust
#[cfg(test)]
mod tests {
    use crate::flowchart::render_flowchart;

    #[test]
    fn simple_chain_renders() {
        let svg = render_flowchart("graph TD\nA[Start] --> B{Choice} -->|yes| C(End)").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("viewBox"));
        assert!(svg.contains("Start") && svg.contains("Choice") && svg.contains("End"));
        assert!(svg.contains("mmd-arrow"));
        assert!(svg.contains("yes"));
        assert!(svg.contains("<polygon")); // the diamond
    }

    #[test]
    fn subgraph_renders_cluster_box_with_title() {
        let svg = render_flowchart("graph TD\nsubgraph grp[The Group]\nA --> B\nend\nC --> A").unwrap();
        assert!(svg.contains("stroke-dasharray=\"4 2\"")); // cluster rect
        assert!(svg.contains("The Group"));
    }

    #[test]
    fn class_def_styles_node_group() {
        let svg = render_flowchart("graph TD\nclassDef hot fill:#f00\nA[Hi]:::hot").unwrap();
        assert!(svg.contains("style=\"fill:#f00\""));
    }

    #[test]
    fn labels_are_escaped() {
        let svg = render_flowchart("graph TD\nA[\"<script>alert(1)</script>\"]").unwrap();
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn dotted_and_thick_edge_styles() {
        let svg = render_flowchart("graph TD\nA -.-> B\nA ==> C").unwrap();
        assert!(svg.contains("stroke-dasharray=\"3 3\""));
        assert!(svg.contains("stroke-width=\"2.5\""));
    }

    #[test]
    fn open_edge_has_no_arrowhead() {
        let svg = render_flowchart("graph TD\nA --- B").unwrap();
        // The marker is defined in defs but referenced by no edge.
        assert_eq!(svg.matches("marker-end").count(), 0);
    }

    #[test]
    fn multiline_label_gets_tspans() {
        let svg = render_flowchart("graph TD\nA[line one<br/>line two]").unwrap();
        assert!(svg.matches("<tspan").count() >= 2);
        assert!(svg.contains("line one") && svg.contains("line two"));
    }

    #[test]
    fn lr_direction_wider_than_tall() {
        let svg = render_flowchart("graph LR\nA --> B --> C --> D").unwrap();
        let vb: Vec<f64> = svg.split("viewBox=\"").nth(1).unwrap()
            .split('"').next().unwrap()
            .split(' ').map(|v| v.parse().unwrap()).collect();
        assert!(vb[2] > vb[3], "LR chain should be wider than tall: {vb:?}");
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_flowchart("graph TD\nA[unclosed").is_err());
    }

    #[test]
    fn too_large_graph_errors() {
        let mut src = String::from("graph TD\n");
        for i in 0..=crate::layout::MAX_NODES {
            src.push_str(&format!("n{i}\n"));
        }
        let e = render_flowchart(&src).unwrap_err();
        assert!(e.message.contains("too large"));
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p ogrenotes-mermaid flowchart::svg` → FAIL.

- [ ] **Step 3: Implement `render_flowchart` in `flowchart/mod.rs`**

```rust
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
```

- [ ] **Step 4: Implement `svg.rs::emit`** following the 6-part document structure above. Compact skeleton (fill in per the structure — every attribute value listed there is normative):

```rust
use crate::escape_xml;
use crate::flowchart::{measure, shapes, EdgeKind, FlowGraph};
use crate::layout::Layout;

pub(crate) fn emit(g: &FlowGraph, l: &Layout) -> String {
    let (w, h) = l.size;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(r#"<defs><marker id="mmd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#);

    // 3. clusters, parents first (depth ascending)
    let mut order: Vec<usize> = (0..g.subgraphs.len()).collect();
    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = g.subgraphs[i].parent { d += 1; i = p; }
        d
    };
    order.sort_by_key(|&i| depth(i));
    for i in order {
        let r = &l.cluster_rects[i];
        // rect + title text per the normative attribute list
        // (title y sits in the strip: r.y + 14.0)
        ...
    }

    // 4. edges (+ label mask rects + label texts)
    for ep in &l.edge_paths {
        let e = &g.edges[ep.edge];
        let d: String = ep.points.iter().enumerate()
            .map(|(i, p)| if i == 0 { format!("M {:.1} {:.1}", p.0, p.1) }
                 else { format!(" L {:.1} {:.1}", p.0, p.1) })
            .collect();
        ...
    }

    // 5. nodes
    for (i, n) in g.nodes.iter().enumerate() {
        let (cx, cy) = l.node_centers[i];
        ...
    }
    out.push_str("</svg>");
    out
}
```

(The `...` bodies follow the normative structure section verbatim; the tests
in Step 1 pin every externally observable property. No `...` may survive into
the commit.)

- [ ] **Step 5: Run tests** — `cargo test -p ogrenotes-mermaid flowchart::` → PASS (all flowchart tests).

- [ ] **Step 6: Commit**

```bash
git add crates/mermaid/src/flowchart/svg.rs crates/mermaid/src/flowchart/mod.rs
git commit -m "feat(mermaid): flowchart SVG assembly + full render pipeline

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 14: `render()` integration, adversarial tests, cleanup

**Files:**
- Modify: `crates/mermaid/src/lib.rs` (Flowchart arm + lib tests + doc comment)
- Modify: `crates/mermaid/src/layout/mod.rs`, `crates/mermaid/src/flowchart/mod.rs` (remove the two `#![allow(dead_code)]` TODOs)

**Interfaces:**
- Consumes: `flowchart::render_flowchart` (Task 13).
- Produces: the public behavior change — `render()` on a Flowchart source returns SVG.

- [ ] **Step 1: Write the failing lib tests** (append to `lib.rs`'s test module)

```rust
    #[test]
    fn flowchart_renders_svg_via_public_render() {
        let out = render("graph TD\nA[Start] --> B{Go?} -->|yes| C(Done)");
        assert_eq!(out.kind, DiagramKind::Flowchart);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        let svg = out.svg.expect("flowchart should render");
        assert!(svg.starts_with("<svg"));
    }

    #[test]
    fn flowchart_parse_error_flows_through_render() {
        let out = render("graph TD\nA[unclosed");
        assert_eq!(out.kind, DiagramKind::Flowchart);
        assert!(out.svg.is_none());
        let e = out.error.expect("error");
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn flowchart_with_subgraph_and_classes_renders() {
        let src = "flowchart LR\nclassDef hot fill:#f00\nsubgraph s[Sub]\nA:::hot --> B\nend\nB --> C";
        let out = render(src);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }
```

Also EXTEND (never modify existing entries) the `render_never_panics_on_adversarial_input` input list with:

```rust
            "graph TD",
            "graph XX",
            "flowchart LR\nA --> ",
            "graph TD\nA[",
            "graph TD\nsubgraph s\nsubgraph t\nA",
            "graph TD\nend\nend",
            &format!("graph TD\n{}", "A --> B\n".repeat(2000)),
            &format!("graph LR\n{}", (0..300).map(|i| format!("n{i} --> n{} \n", (i * 7) % 300)).collect::<String>()),
            "graph TD\nA --> A --> A",
            "graph TD\nA[🥧<br/>🥧] -->|🥧| B",
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p ogrenotes-mermaid flowchart_renders` → FAIL ("not yet supported").

- [ ] **Step 3: Wire the arm**

In `lib.rs`'s `render()`, replace the `DiagramKind::Flowchart` case (currently falling into the `other => not yet supported` arm) with:

```rust
        DiagramKind::Flowchart => match flowchart::render_flowchart(source) {
            Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
```

Keep Sequence/State/Class/Er in the "not yet supported" arm. Update the crate doc comment to mention pie + flowchart. Remove both `#![allow(dead_code)]` markers; fix any dead-code warnings they were masking (delete genuinely dead helpers rather than re-allowing).

- [ ] **Step 4: Full verification**

Run: `cargo test -p ogrenotes-mermaid` → ALL green (pie, layout, props, flowchart, lib).
Run: `cargo clippy -p ogrenotes-mermaid --all-targets` → clean.
Run: `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown` → clean.
Run: `cargo test -p ogrenotes-collab export::` → still green (server export path picks up flowchart automatically; no assertions there depend on flowchart being unsupported).
Run: `cd frontend && cargo build --target wasm32-unknown-unknown` → clean (bundle-size gate runs in CI; if it trips there, that is a CI conversation, not a local fix).

- [ ] **Step 5: Commit**

```bash
git add crates/mermaid/src/lib.rs crates/mermaid/src/layout/mod.rs crates/mermaid/src/flowchart/mod.rs
git commit -m "feat(mermaid): wire flowchart rendering into render()

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Final verification (after Task 14)

- [ ] `cargo test -p ogrenotes-mermaid` — every module green.
- [ ] `cargo clippy -p ogrenotes-mermaid --all-targets` — clean.
- [ ] `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown` — clean.
- [ ] `cargo test -p ogrenotes-collab --lib` — green except the 8 pre-existing `redis_pubsub` env-dependent tests.
- [ ] `cd frontend && cargo build --target wasm32-unknown-unknown` — clean.
- [ ] Manual smoke via the app: insert a Mermaid block, paste a flowchart with a subgraph + classDef, verify live render, edit-modal preview, HTML export contains the SVG, and an invalid line shows the error + raw source.

## Notes for the implementer

- Tasks 1–8 are pure algorithm work with no mermaid knowledge; if a layout
  test won't pass, fix the stage, never loosen the test.
- The plan's example code is a starting point — where it conflicts with the
  compiler or its own tests, fix the code and record the deviation in your
  report (slice 1 precedent: the plan had bugs; the tests are the contract).
- Determinism is a hard requirement everywhere: no HashMap iteration order
  may leak into output. Where iteration order matters (e.g. emitting nodes,
  collecting chains), iterate by index or sort first. `Coords.centers` is a
  HashMap but is only ever read by key.
- Anything unclear about cluster collapse-expand: the five cluster tests in
  Task 7 are the semantics; the 6-step algorithm text is normative.
