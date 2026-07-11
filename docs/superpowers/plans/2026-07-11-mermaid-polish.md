# Mermaid Polish (issue #32 highlights) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the flowchart parser's two silent misparses (circle/cross edge terminators parsed as phantom nodes; edges to subgraph ids parsed as phantom nodes) and close the cheap doc-parity gaps (multi-length arrows, bidirectional, invisible links, no-space labels, subroutine shape, YAML front matter, `classDef default`), per the approved spec `docs/superpowers/specs/2026-07-11-mermaid-polish-design.md`.

**Architecture:** One unified edge-operator scanner replaces the two fixed tables in `flowchart/parse.rs::parse_edge_op`. Edges gain explicit per-end heads (`from_head`/`to_head`); `EdgeKind` keeps its existing variants (existing tests assert them) plus a new `Invisible`. SVG emission derives line style from `EdgeKind` and markers from heads. Everything else is small, independent additions.

**Tech Stack:** Rust (workspace crate `crates/mermaid`, `ogrenotes-mermaid`). Zero runtime dependencies, `#![forbid(unsafe_code)]`, proptest as dev-dependency only.

## Global Constraints

- Worktree: `/home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish` (branch `worktree-mermaid-polish`). Run all commands from there. The shell cwd may reset between Bash calls — `cd` explicitly every time.
- **Existing tests are immutable.** Every task adds new tests only. If an existing test fails, your implementation is wrong — do not edit the test. (The plan was checked: no existing test asserts the old phantom-node parses or the old `-.-`/`===` arrowheads.)
- `render()` never panics; exactly one of `svg`/`error` (XOR); 1-based error lines against the ORIGINAL source; every user string through `escape_xml`; deterministic output; caps unchanged (`MAX_NODES` 400, `MAX_EDGES` 1000, `MAX_SOURCE_LEN` 20_000).
- UTF-8 slice discipline: operator characters are all ASCII (`-.=~<>ox|`), so byte indexing/slicing at operator positions stays on char boundaries — state this in a comment at each new slice site, matching the crate convention.
- NEVER run bare `git stash` (shared stash stack across worktrees). Never `git add -A` / `git add .` — stage files by name.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`
- Test command: `cargo test -p ogrenotes-mermaid` (fast; run the full crate suite at the end of every task).

## Mermaid semantics reference (normative for this plan)

Mermaid's documented terminator binding: after a link's dash/equals run, an immediately following `o`, `x`, or `>` is part of the LINK, not the node — the docs themselves warn that `A---oB` draws a circle-ended edge to `B` (write `A--- oB` or capitalize to get a node). Consequences used throughout:

- `A--oB`, `A--xB`, `A---oB`, `A---xB` = circle/cross-ended edge to `B` (these were our phantom-node misparses).
- `A-->oB` = arrow edge to a node named `oB` — in mermaid too, because `>` already terminated the link. Our current behavior is already correct for this spelling; keep it and test it.
- Minimum runs: solid open is `---` (3+); `--` needs a terminator (`-->`, `--o`, `--x`) or an inline label (`A--text-->B`). Thick open is `===`; `==` needs a terminator or label. Dotted plain is `-` `.`+ `-` (`-.-`, `-..-`) plus optional terminator. Invisible is `~~~` (3+), no terminator, no label.
- `-.-` and `===` are OPEN (no arrowhead). Our current renderer wrongly puts an arrowhead on them (`EdgeKind::Dotted`/`Thick` always emit `marker-end`); the head model fixes this. No existing test locks the wrong behavior.
- Reverse heads: `<`, `o`, `x` immediately before the run (`<-->`, `o--o`, `x--x`, `<-.->`, `<==>`). `o`/`x` count as a reverse head only when the next character starts a run body (`-` or `=`); `<` likewise.

---

### Task 1: Edge model with per-end heads + unified plain-operator scanner

**Files:**
- Modify: `crates/mermaid/src/flowchart/mod.rs` (Head enum, EdgeKind::Invisible, FlowEdge fields)
- Modify: `crates/mermaid/src/flowchart/parse.rs` (replace `parse_edge_op`, adjust `parse_chain`)
- Modify: `crates/mermaid/src/flowchart/svg.rs` (ONLY a minimal `EdgeKind::Invisible => continue` arm so the crate compiles; full marker work is Task 3)
- Tests: new tests in `parse.rs` `mod tests`

**Interfaces:**
- Produces: `pub(crate) enum Head { None, Arrow, Circle, Cross }` in `flowchart/mod.rs`; `FlowEdge` gains `pub from_head: Head, pub to_head: Head`; `EdgeKind` gains `Invisible`; private `struct EdgeOp { kind: EdgeKind, from_head: Head, to_head: Head, label: Option<String> }` in parse.rs; `parse_edge_op` returns `Result<EdgeOp, ParseError>`.
- Consumed by: Task 2 (inline labels return `EdgeOp` too), Task 3 (svg reads `from_head`/`to_head`), Task 8 (props).
- `EdgeKind` mapping rule (keeps every existing test passing): solid body → `Arrow` if either head is `Head::Arrow`, else `Open`; dotted body → `Dotted`; thick body → `Thick`; tilde body → `Invisible`. So `-->`→Arrow, `---`→Open, `--o`→Open+Circle-to, `<-->`→Arrow both heads, `-.-`→Dotted no heads, `-.->`→Dotted+Arrow-to, `==>`→Thick+Arrow-to, `===`→Thick no heads.

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `parse.rs`)

```rust
    // ── Polish slice: unified edge-operator scanner ──────────────────
    // (docs/superpowers/specs/2026-07-11-mermaid-polish-design.md)

    use crate::flowchart::Head;

    #[test]
    fn circle_and_cross_terminators_bind_to_the_edge_not_the_node() {
        // THE silent-misparse regression tests (issue #32): these used to
        // parse an edge to a phantom node named `oB`/`xB`.
        for (src, to_head) in [
            ("A--oB", Head::Circle),
            ("A--xB", Head::Cross),
            ("A---oB", Head::Circle),
            ("A---xB", Head::Cross),
            ("A --o B", Head::Circle),
            ("A --x B", Head::Cross),
        ] {
            let g = p(&format!("graph TD\n{src}"));
            assert_eq!(g.nodes.len(), 2, "exactly A and B for {src}");
            assert_eq!(g.nodes[1].id, "B", "no phantom node for {src}");
            assert_eq!(g.edges[0].to_head, to_head, "for {src}");
            assert_eq!(g.edges[0].from_head, Head::None, "for {src}");
            assert_eq!(g.edges[0].kind, EdgeKind::Open, "for {src}");
        }
    }

    #[test]
    fn arrow_terminator_then_o_is_a_node_like_mermaid() {
        // `>` already terminated the link, so `oB` IS a node — mermaid
        // parses this identically (the docs' o/x warning only covers a
        // terminator directly after the run).
        let g = p("graph TD\nA-->oB");
        assert_eq!(g.nodes[1].id, "oB");
        assert_eq!(g.edges[0].to_head, Head::Arrow);
    }

    #[test]
    fn spaced_o_after_open_run_is_a_node() {
        // mermaid's own documented escape hatch: the space breaks the
        // terminator binding.
        let g = p("graph TD\nA--- oB");
        assert_eq!(g.nodes[1].id, "oB");
        assert_eq!(g.edges[0].kind, EdgeKind::Open);
        assert_eq!(g.edges[0].to_head, Head::None);
    }

    #[test]
    fn multi_length_runs_collapse_to_base_kind() {
        for (src, kind, to_head) in [
            ("A ----> B", EdgeKind::Arrow, Head::Arrow),
            ("A -----> B", EdgeKind::Arrow, Head::Arrow),
            ("A ---- B", EdgeKind::Open, Head::None),
            ("A ====> B", EdgeKind::Thick, Head::Arrow),
            ("A ==== B", EdgeKind::Thick, Head::None),
            ("A -..-> B", EdgeKind::Dotted, Head::Arrow),
            ("A -...- B", EdgeKind::Dotted, Head::None),
        ] {
            let g = p(&format!("graph TD\n{src}"));
            assert_eq!(g.edges[0].kind, kind, "for {src}");
            assert_eq!(g.edges[0].to_head, to_head, "for {src}");
            assert_eq!(g.nodes.len(), 2, "for {src}");
        }
    }

    #[test]
    fn bidirectional_and_reverse_heads() {
        for (src, from_head, to_head) in [
            ("A <--> B", Head::Arrow, Head::Arrow),
            ("A o--o B", Head::Circle, Head::Circle),
            ("A x--x B", Head::Cross, Head::Cross),
            ("A <-.-> B", Head::Arrow, Head::Arrow),
            ("A <==> B", Head::Arrow, Head::Arrow),
            // `<---` is mermaid's reverse-open form; bare `<--` is
            // invalid there (a link needs one more run/terminator char)
            // and is covered by the error test below.
            ("A <--- B", Head::Arrow, Head::None),
        ] {
            let g = p(&format!("graph TD\n{src}"));
            assert_eq!(g.edges[0].from_head, from_head, "for {src}");
            assert_eq!(g.edges[0].to_head, to_head, "for {src}");
            assert_eq!(g.nodes.len(), 2, "for {src}");
        }
    }

    #[test]
    fn plain_dotted_and_thick_are_open_no_heads() {
        // mermaid semantics: `-.-` and `===` have NO arrowhead. (The old
        // fixed table wrongly gave them one; no test locked that.)
        for src in ["A -.- B", "A === B"] {
            let g = p(&format!("graph TD\n{src}"));
            assert_eq!(g.edges[0].to_head, Head::None, "for {src}");
        }
    }

    #[test]
    fn invisible_link_parses_as_invisible_edge() {
        let g = p("graph TD\nA ~~~ B\nA ~~~~ B");
        assert_eq!(g.edges.len(), 2);
        for e in &g.edges {
            assert_eq!(e.kind, EdgeKind::Invisible);
            assert_eq!((e.from_head, e.to_head), (Head::None, Head::None));
            assert!(e.label.is_none());
        }
    }

    #[test]
    fn invisible_link_rejects_labels_and_short_runs() {
        for src in ["A ~~~|x| B", "A ~~ B"] {
            let e = parse(&format!("graph TD\n{src}")).unwrap_err();
            assert_eq!(e.line, Some(2), "for {src}");
        }
    }

    #[test]
    fn two_dash_run_without_terminator_or_label_errors() {
        // `<--` is also invalid in mermaid (reverse-open is `<---`).
        for src in ["A -- B", "A == B", "A <-- B"] {
            let e = parse(&format!("graph TD\n{src}")).unwrap_err();
            assert_eq!(e.line, Some(2), "for {src}");
        }
    }

    #[test]
    fn pipe_label_still_works_on_new_operators() {
        let g = p("graph TD\nA --o|maybe| B\nA <-->|both| B");
        assert_eq!(g.edges[0].label.as_deref(), Some("maybe"));
        assert_eq!(g.edges[1].label.as_deref(), Some("both"));
    }

    #[test]
    fn terminator_binding_inside_would_be_labels_matches_mermaid() {
        // `--o` binds before any label scan, so `A--oops-->B` is a
        // circle-edge to `ops`, then `ops-->B` — mermaid's documented
        // footgun, reproduced deliberately (never-silent: same graph).
        let g = p("graph TD\nA--oops-->B");
        assert_eq!(g.nodes[1].id, "ops");
        assert_eq!(g.edges[0].to_head, Head::Circle);
        assert_eq!(g.edges[1].to_head, Head::Arrow);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid flowchart::parse`
Expected: FAIL — compile errors first (no `Head`, no `from_head`); after the model lands (mid-step-3), assertion failures on the scanner tests.

- [ ] **Step 3: Implement the model + scanner**

In `flowchart/mod.rs`, add after `EdgeKind`:

```rust
/// Per-end edge decoration. `from_head` renders as `marker-start`,
/// `to_head` as `marker-end` (edge paths run from→to after the layout
/// engine restores true direction on reversed edges).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Head {
    None,
    Arrow,
    Circle,
    Cross,
}
```

Extend `EdgeKind` with a variant (existing variants unchanged):

```rust
    /// `~~~` — participates in layout, draws nothing.
    Invisible,
```

Extend `FlowEdge`:

```rust
pub(crate) struct FlowEdge {
    pub from: usize,
    pub to: usize,
    /// Line-style family. `Arrow` is retained (rather than folding into
    /// `Open`) because the parser's pre-polish tests assert it for `-->`;
    /// solid edges map to `Arrow` iff either head is `Head::Arrow`.
    pub kind: EdgeKind,
    pub from_head: Head,
    pub to_head: Head,
    pub label: Option<String>,
}
```

In `parse.rs`, add `use crate::flowchart::Head;` to the imports, define above `impl Parser`:

```rust
/// A fully-resolved edge operator, including any inline label.
struct EdgeOp {
    kind: EdgeKind,
    from_head: Head,
    to_head: Head,
    label: Option<String>,
}
```

Replace `parse_edge_op` entirely:

```rust
    /// Unified edge-operator scanner (polish slice, see
    /// docs/superpowers/specs/2026-07-11-mermaid-polish-design.md):
    ///
    ///   edge-op    := [rev-head] body [terminator] [label]
    ///   rev-head   := '<' | 'o' | 'x'   (only when a body char follows)
    ///   body       := '-'{2,} | '-' '.'{1,} '-' | '='{2,} | '~'{3,}
    ///   terminator := '>' | 'o' | 'x'   (bound IMMEDIATELY after the
    ///                  run — mermaid's documented `A---oB` = circle rule)
    ///   label      := inline (`--text-->`, `-. text .->`, `==text==>`)
    ///                 or `|text|` after a plain operator
    ///
    /// A 2-length solid/thick run with no terminator opens an inline
    /// label; `-.` not followed by dots-then-`-` likewise. Multi-length
    /// runs collapse to the base kind (rank-span hint not honored — a
    /// documented cosmetic divergence). All operator characters are
    /// ASCII, so the byte indexing below always lands on char boundaries.
    fn parse_edge_op(&mut self, rest: &mut &str) -> Result<EdgeOp, ParseError> {
        let r = rest.trim_start();
        let b = r.as_bytes();
        // Optional reverse head. `o`/`x` are also id characters, so they
        // only count when the NEXT byte starts a run body; `<` gets the
        // same guard so a stray `<` falls through to the clean error.
        let (from_head, r) = match (b.first(), b.get(1)) {
            (Some(b'<'), Some(b'-' | b'=')) => (Head::Arrow, &r[1..]),
            (Some(b'o'), Some(b'-' | b'=')) => (Head::Circle, &r[1..]),
            (Some(b'x'), Some(b'-' | b'=')) => (Head::Cross, &r[1..]),
            _ => (Head::None, r),
        };
        let b = r.as_bytes();
        let expected = || format!("expected an edge (e.g. `-->`), found {r:?}");
        // Body run. `body_len` is a byte length over ASCII-only chars.
        let (family, body_len) = match b.first() {
            Some(b'~') => {
                let n = b.iter().take_while(|&&c| c == b'~').count();
                if n < 3 {
                    return Err(self.err("invisible links need at least `~~~`"));
                }
                if from_head != Head::None {
                    return Err(self.err("invisible links cannot have arrow heads"));
                }
                // No terminator, no label: `~~~xB` is an invisible edge
                // to node `xB` (mermaid binds no terminator after `~`).
                *rest = &r[n..];
                if rest.trim_start().starts_with('|') {
                    return Err(self.err("invisible links cannot carry a label"));
                }
                return Ok(EdgeOp {
                    kind: EdgeKind::Invisible,
                    from_head: Head::None,
                    to_head: Head::None,
                    label: None,
                });
            }
            Some(b'=') => {
                let n = b.iter().take_while(|&&c| c == b'=').count();
                if n < 2 {
                    return Err(self.err(expected()));
                }
                (EdgeKind::Thick, n)
            }
            Some(b'-') if b.get(1) == Some(&b'.') => {
                // Dotted plain body is `-` `.`+ `-`; if the dots are not
                // followed by `-`, this is the `-.label.-` inline form.
                let dots = b[1..].iter().take_while(|&&c| c == b'.').count();
                if b.get(1 + dots) == Some(&b'-') {
                    (EdgeKind::Dotted, 1 + dots + 1)
                } else {
                    return self.parse_inline_label(rest, r, from_head, EdgeKind::Dotted);
                }
            }
            Some(b'-') => {
                let n = b.iter().take_while(|&&c| c == b'-').count();
                if n < 2 {
                    return Err(self.err(expected()));
                }
                (EdgeKind::Arrow, n) // family placeholder; final kind below
            }
            _ => return Err(self.err(expected())),
        };
        // Terminator, bound immediately after the run (no whitespace).
        let after = &r[body_len..];
        let (to_head, after) = match after.as_bytes().first() {
            Some(b'>') => (Head::Arrow, &after[1..]),
            Some(b'o') => (Head::Circle, &after[1..]),
            Some(b'x') => (Head::Cross, &after[1..]),
            _ => (Head::None, after),
        };
        // A minimum-length solid/thick run with neither terminator nor
        // reverse head is only valid as an inline-label opener
        // (`A--text-->B`); mermaid's shortest plain open links are `---`
        // and `===`.
        if body_len == 2
            && to_head == Head::None
            && matches!(family, EdgeKind::Arrow | EdgeKind::Thick)
        {
            if from_head == Head::None {
                return self.parse_inline_label(
                    rest,
                    r,
                    from_head,
                    if family == EdgeKind::Thick { EdgeKind::Thick } else { EdgeKind::Arrow },
                );
            }
            return Err(self.err(expected()));
        }
        let kind = match family {
            EdgeKind::Arrow => {
                if to_head == Head::Arrow || from_head == Head::Arrow {
                    EdgeKind::Arrow
                } else {
                    EdgeKind::Open
                }
            }
            other => other,
        };
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
        Ok(EdgeOp { kind, from_head, to_head, label })
    }

    /// Inline-label forms. `r` starts at the opener (`--`, `==`, or
    /// `-.`). Spaced forms (`A-- text -->B`) work as before; Task 2
    /// extends this to the docs' no-space spellings.
    fn parse_inline_label(
        &mut self,
        rest: &mut &str,
        r: &str,
        from_head: Head,
        family: EdgeKind,
    ) -> Result<EdgeOp, ParseError> {
        let (open, close, kind) = match family {
            EdgeKind::Dotted => ("-.", ".-", EdgeKind::Dotted),
            EdgeKind::Thick => ("==", "==", EdgeKind::Thick),
            _ => ("--", "--", EdgeKind::Arrow),
        };
        let Some(after_open) = r.strip_prefix(open) else {
            return Err(self.err(format!("expected an edge (e.g. `-->`), found {r:?}")));
        };
        if !after_open.starts_with(' ') {
            return Err(self.err(format!(
                "expected an edge (e.g. `-->`) or a closed inline label, found {r:?}"
            )));
        }
        let Some(i) = after_open.find(close) else {
            return Err(self.err(format!(
                "unclosed inline edge label (missing `{close}`)"
            )));
        };
        let label = after_open[..i].trim().to_string();
        let after_close = &after_open[i + close.len()..];
        // Closer run may be longer (`---`), then an optional terminator.
        let extra = after_close
            .as_bytes()
            .iter()
            .take_while(|&&c| c == close.as_bytes()[close.len() - 1])
            .count();
        let after_run = &after_close[extra..];
        let (to_head, after_run) = match after_run.as_bytes().first() {
            Some(b'>') => (Head::Arrow, &after_run[1..]),
            Some(b'o') => (Head::Circle, &after_run[1..]),
            Some(b'x') => (Head::Cross, &after_run[1..]),
            _ => (Head::None, after_run),
        };
        let kind = match kind {
            EdgeKind::Arrow => {
                if to_head == Head::Arrow || from_head == Head::Arrow {
                    EdgeKind::Arrow
                } else {
                    EdgeKind::Open
                }
            }
            other => other,
        };
        *rest = after_run;
        Ok(EdgeOp { kind, from_head, to_head, label: Some(label) })
    }
```

(Sanity trace for the pre-existing spellings: `A-- no -->B` → open `--`, label ` no `→"no", `find("--")` hits the closer, extra run consumes the second `-`? No — `find(close)` finds the FIRST `--` of `-->` at the closer, `i + 2` lands on `>`, extra=0, terminator `>` → Arrow. `A-. maybe .->B` → close `.-`, after it `>` → Dotted+Arrow. `A== t ==>B` → Thick+Arrow. `A-- t ---B` → after close `--` comes `-`, extra=1, then `B` → Open with label.)

In `parse_chain`, update the call site:

```rust
            let op = self.parse_edge_op(&mut rest)?;
            let rhs = self.parse_node_group(&mut rest)?;
            for &f in &lhs {
                for &t in &rhs {
                    self.g.edges.push(FlowEdge {
                        from: f,
                        to: t,
                        kind: op.kind,
                        from_head: op.from_head,
                        to_head: op.to_head,
                        label: op.label.clone(),
                    });
```

(keep the in-loop `MAX_EDGES` bail exactly as it is).

In `svg.rs`, make the `match e.kind` compile by adding ONE arm at the top of the existing match (full rework is Task 3):

```rust
            EdgeKind::Invisible => continue, // layout-only; draws nothing (Task 3 reworks markers)
```

— place it as the first arm of the `let attrs = match e.kind { ... }` match; since `match` arms can't `continue` out of a value position directly, restructure minimally:

```rust
        if e.kind == EdgeKind::Invisible {
            continue; // layout-only; draws nothing
        }
        let attrs = match e.kind {
            EdgeKind::Invisible => String::new(), // unreachable: guarded above
            ...existing four arms unchanged...
        };
```

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS — all pre-existing tests (including `edge_kinds`, `inline_label`, `pipe_label`, `open_edge_has_no_arrowhead`) and all new Task-1 tests.

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
git add crates/mermaid/src/flowchart/mod.rs crates/mermaid/src/flowchart/parse.rs crates/mermaid/src/flowchart/svg.rs
git commit -m "feat(mermaid): unified flowchart edge-op scanner — circle/cross heads, multi-length, bidirectional, invisible links

Kills the phantom-node silent misparse for terminator-after-run
spellings (A--oB, A---xB): the terminator now binds to the edge,
matching mermaid's documented rule.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: No-space inline label spellings

**Files:**
- Modify: `crates/mermaid/src/flowchart/parse.rs` (`parse_inline_label` only)
- Tests: new tests in `parse.rs` `mod tests`

**Interfaces:**
- Consumes: `EdgeOp`, `Head`, `parse_inline_label(rest, r, from_head, family)` from Task 1.
- Produces: same signature; drops the space requirement.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn no_space_inline_labels() {
        // The docs' no-space spellings (previously loud errors).
        for (src, kind, to_head, label) in [
            ("A-.text.-B", EdgeKind::Dotted, Head::None, "text"),
            ("A-.text.->B", EdgeKind::Dotted, Head::Arrow, "text"),
            ("A==text==>B", EdgeKind::Thick, Head::Arrow, "text"),
            ("A--text-->B", EdgeKind::Arrow, Head::Arrow, "text"),
            ("A--text---B", EdgeKind::Open, Head::None, "text"),
            ("A-- text --xB", EdgeKind::Open, Head::Cross, "text"),
        ] {
            let g = p(&format!("graph TD\n{src}"));
            assert_eq!(g.edges[0].kind, kind, "for {src}");
            assert_eq!(g.edges[0].to_head, to_head, "for {src}");
            assert_eq!(g.edges[0].label.as_deref(), Some(label), "for {src}");
            assert_eq!(g.nodes.len(), 2, "for {src}");
            assert_eq!(g.nodes[1].id, "B", "for {src}");
        }
    }

    #[test]
    fn multi_length_inline_label_closer() {
        // Docs example: `A --text---- E` (long closer run).
        let g = p("graph TD\nA --text---- E");
        assert_eq!(g.edges[0].label.as_deref(), Some("text"));
        assert_eq!(g.edges[0].kind, EdgeKind::Open);
        assert_eq!(g.nodes[1].id, "E");
    }

    #[test]
    fn unclosed_inline_label_is_line_error() {
        for src in ["A--text", "A-.text", "A==text"] {
            let e = parse(&format!("graph TD\n{src}")).unwrap_err();
            assert_eq!(e.line, Some(2), "for {src}");
        }
    }
```

- [ ] **Step 2: Run to verify the new tests fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid no_space_inline`
Expected: FAIL — `A-.text.-B` errors ("closed inline label"), `A--text-->B` errors.

- [ ] **Step 3: Drop the space requirement**

In `parse_inline_label`, delete this block:

```rust
        if !after_open.starts_with(' ') {
            return Err(self.err(format!(
                "expected an edge (e.g. `-->`) or a closed inline label, found {r:?}"
            )));
        }
```

That is the entire change: the label now runs from immediately after the opener to the first occurrence of the closer, spaced or not. (Trace: `A--text-->B` reaches here because Task 1's scanner only calls `parse_inline_label` when the 2-run has no terminator — `t` is not `>`/`o`/`x`. `A-.text.-B` reaches here because the dots aren't followed by `-`. Labels starting with `o`/`x`/`>` after `--` bind as terminators first — mermaid's documented footgun, covered by `terminator_binding_inside_would_be_labels_matches_mermaid`.)

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS (existing spaced-form tests unaffected — the space is now part of the trimmed label scan).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
git add crates/mermaid/src/flowchart/parse.rs
git commit -m "feat(mermaid): accept the docs' no-space inline edge-label spellings

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: SVG head markers (circle, cross, marker-start) + invisible-edge emission

**Files:**
- Modify: `crates/mermaid/src/flowchart/svg.rs`
- Tests: new tests in `svg.rs` `mod tests`

**Interfaces:**
- Consumes: `FlowEdge.from_head` / `to_head`, `Head`, `EdgeKind::Invisible` from Task 1.
- Produces: flowchart defs contain `mmd-arrow`, `mmd-circle`, `mmd-cross`; edges emit `marker-start`/`marker-end` per head.

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `svg.rs`)

```rust
    #[test]
    fn circle_and_cross_heads_render_their_markers() {
        let svg = render_flowchart("graph TD\nA --o B\nB --x C").unwrap();
        assert!(svg.contains(r##"marker-end="url(#mmd-circle)""##), "{svg}");
        assert!(svg.contains(r##"marker-end="url(#mmd-cross)""##), "{svg}");
        assert!(svg.contains(r#"<marker id="mmd-circle""#));
        assert!(svg.contains(r#"<marker id="mmd-cross""#));
    }

    #[test]
    fn bidirectional_edge_gets_marker_start_and_end() {
        let svg = render_flowchart("graph TD\nA <--> B").unwrap();
        assert!(svg.contains(r##"marker-start="url(#mmd-arrow)""##), "{svg}");
        assert!(svg.contains(r##"marker-end="url(#mmd-arrow)""##), "{svg}");
    }

    #[test]
    fn invisible_edge_emits_no_path_but_constrains_layout() {
        let svg = render_flowchart("graph TD\nA ~~~ B").unwrap();
        // Both nodes render; the edge draws nothing. (Can't assert on
        // `<path` — the always-present marker DEFS contain paths. Edge
        // paths are the only elements carrying this exact attr prefix;
        // the marker defs order their attributes differently.)
        assert!(svg.contains(">A<") && svg.contains(">B<"), "{svg}");
        assert!(
            !svg.contains(r#"stroke="currentColor" fill="none""#),
            "invisible edge drew a path: {svg}"
        );
    }

    #[test]
    fn plain_dotted_edge_has_no_arrowhead() {
        // mermaid semantics fix: `-.-` is open (the old table always
        // arrowed dotted edges).
        let svg = render_flowchart("graph TD\nA -.- B").unwrap();
        assert_eq!(svg.matches("marker-end").count(), 0, "{svg}");
        assert!(svg.contains("stroke-dasharray=\"3 3\""));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid flowchart::svg`
Expected: FAIL — no `mmd-circle` marker exists; `-.-` still emits `marker-end`.

- [ ] **Step 3: Implement**

Replace the single-marker defs line with three markers (`mmd-cross` copied verbatim from `sequence/svg.rs:91`):

```rust
    out.push_str(concat!(
        "<defs>",
        r#"<marker id="mmd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker>"#,
        r#"<marker id="mmd-circle" viewBox="0 0 10 10" refX="5" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><circle cx="5" cy="5" r="3.5" fill="currentColor"/></marker>"#,
        r#"<marker id="mmd-cross" viewBox="0 0 10 10" refX="5" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 1 1 L 9 9 M 9 1 L 1 9" stroke="currentColor" stroke-width="1.5" fill="none"/></marker>"#,
        "</defs>",
    ));
```

Replace the whole `let attrs = match e.kind {...}` block (including Task 1's guard) in the edge loop with:

```rust
        if e.kind == EdgeKind::Invisible {
            continue; // participates in layout; draws nothing
        }
        // Line style from the kind family; heads from the edge ends.
        let line_attrs = match e.kind {
            EdgeKind::Arrow | EdgeKind::Open | EdgeKind::Invisible => "",
            EdgeKind::Dotted => r#" stroke-dasharray="3 3""#,
            EdgeKind::Thick => r#" stroke-width="2.5""#,
        };
        let marker = |h: Head| match h {
            Head::None => None,
            Head::Arrow => Some("mmd-arrow"),
            Head::Circle => Some("mmd-circle"),
            Head::Cross => Some("mmd-cross"),
        };
        let mut attrs = format!(r#"stroke="currentColor" fill="none"{line_attrs}"#);
        if let Some(m) = marker(e.from_head) {
            attrs.push_str(&format!(r#" marker-start="url(#{m})""#));
        }
        if let Some(m) = marker(e.to_head) {
            attrs.push_str(&format!(r#" marker-end="url(#{m})""#));
        }
```

and update the import line to `use crate::flowchart::{shapes, EdgeKind, FlowGraph, FlowNode, Head};`.

(Compat trace against immutable tests: `open_edge_has_no_arrowhead` — `A --- B` → heads None → zero `marker-end` occurrences, and the defs contain none ✓. `dotted_and_thick_edge_styles` — `-.->`/`==>` keep dasharray/width ✓. `simple_chain_renders` — `mmd-arrow` present ✓. Reversed back-edges: `layout/route.rs` restores true point order, so `points[0]` is at the `from` node and `marker-start` lands correctly.)

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
git add crates/mermaid/src/flowchart/svg.rs
git commit -m "feat(mermaid): render circle/cross/bidirectional edge heads; invisible edges draw nothing

Also fixes -.- and === wrongly rendering an arrowhead (mermaid treats
them as open links).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Edges to subgraph ids error loudly

**Files:**
- Modify: `crates/mermaid/src/flowchart/parse.rs` (Parser field + post-parse pass)
- Modify: `crates/mermaid/src/flowchart/mod.rs` (FlowNode.id doc comment + drop `#[allow(dead_code)]`)
- Tests: new tests in `parse.rs` `mod tests`

**Interfaces:**
- Consumes: `Parser`, `FlowGraph` from Task 1 state.
- Produces: `Parser.edge_lines: Vec<usize>` (parallel to `g.edges`, records `self.line` at each push); post-parse validation in `parse()`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn edge_to_subgraph_id_errors_loudly() {
        // Silent-misparse #2 (issue #32): this used to create a phantom
        // node `sgID`. Real edge-to-subgraph routing is deferred; v1
        // errors loudly, naming the subgraph.
        let e = parse("graph TD\nsubgraph sgID[T]\nA --> B\nend\nsgID --> C").unwrap_err();
        assert_eq!(e.line, Some(5));
        assert!(e.message.contains("subgraph"), "got: {}", e.message);
        assert!(e.message.contains("sgID"), "got: {}", e.message);
    }

    #[test]
    fn edge_from_and_to_subgraph_both_error() {
        let e = parse("graph TD\nsubgraph s\nA\nend\nC --> s").unwrap_err();
        assert_eq!(e.line, Some(5));
    }

    #[test]
    fn edge_above_subgraph_declaration_still_errors() {
        // Real mermaid resolves subgraph ids declared LATER; the check
        // must therefore run post-parse, not inline.
        let e = parse("graph TD\ns --> C\nsubgraph s\nA\nend").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("\"s\""), "got: {}", e.message);
    }

    #[test]
    fn bare_subgraph_id_statement_does_not_error() {
        // Only EDGES to subgraph ids are the misparse class; a bare node
        // statement that happens to shadow a subgraph id parses (as
        // today) — no edge, no silent-wrong-graph.
        assert!(parse("graph TD\nsubgraph s\nA\nend\ns").is_ok());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid subgraph_id`
Expected: FAIL — the first three parse Ok (phantom node) instead of erroring.

- [ ] **Step 3: Implement**

Add the field to `Parser`:

```rust
    /// Source line of each pushed edge (parallel to `g.edges`), for the
    /// post-parse edge-to-subgraph check — errors must point at the
    /// edge's own line, but subgraph ids can be declared later.
    edge_lines: Vec<usize>,
```

initialize `edge_lines: Vec::new(),` in `parse()`, and in `parse_chain`'s push loop add `self.edge_lines.push(self.line);` immediately after `self.g.edges.push(...)`.

In `parse()`, after the unclosed-subgraph check and before `Ok(p.g)`:

```rust
    // Post-parse: edges whose endpoint id names a subgraph are real
    // mermaid syntax (edge attaches to the cluster box) that we don't
    // lay out yet — error loudly instead of drawing a phantom node.
    // Post-parse because subgraph ids may be declared below the edge.
    for (i, e) in p.g.edges.iter().enumerate() {
        for end in [e.from, e.to] {
            let id = &p.g.nodes[end].id;
            if p.g.subgraphs.iter().any(|s| &s.id == id) {
                return Err(ParseError {
                    message: format!(
                        "edges to/from subgraph ids are not yet supported (subgraph {id:?})"
                    ),
                    line: Some(p.edge_lines[i]),
                });
            }
        }
    }
```

In `flowchart/mod.rs`, `FlowNode.id` is now read by production code — remove the `#[allow(dead_code)]` attribute and replace its doc comment with:

```rust
    /// Mermaid source identifier. Read by the post-parse
    /// edge-to-subgraph check (parse.rs) and asserted on by parser
    /// tests; downstream render stages address nodes by index.
    pub id: String,
```

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS (no existing test edges a subgraph id — verified while writing this plan).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
git add crates/mermaid/src/flowchart/parse.rs crates/mermaid/src/flowchart/mod.rs
git commit -m "feat(mermaid): edges to subgraph ids error loudly instead of creating a phantom node

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: `classDef default` auto-applies to unclassed nodes

**Files:**
- Modify: `crates/mermaid/src/flowchart/parse.rs`
- Tests: new tests in `parse.rs` and one in `svg.rs`

**Interfaces:**
- Consumes: `FlowGraph.class_defs`, `FlowNode.classes`.
- Produces: post-parse pass in `parse()` (after the Task 4 pass).

- [ ] **Step 1: Write the failing tests** (parse.rs)

```rust
    #[test]
    fn class_def_default_applies_to_unclassed_nodes_only() {
        let g = p("graph TD\nclassDef default fill:#f9f\nclassDef hot fill:#f00\nA --> B:::hot");
        assert_eq!(g.nodes[0].classes, vec!["default"]); // A: auto
        assert_eq!(g.nodes[1].classes, vec!["hot"]);     // B: explicit wins
    }

    #[test]
    fn no_default_class_def_means_no_auto_class() {
        let g = p("graph TD\nclassDef hot fill:#f00\nA");
        assert!(g.nodes[0].classes.is_empty());
    }
```

and (svg.rs):

```rust
    #[test]
    fn class_def_default_styles_every_unclassed_node() {
        let svg = render_flowchart("graph TD\nclassDef default fill:#f9f\nA --> B").unwrap();
        assert_eq!(svg.matches("style=\"fill:#f9f\"").count(), 2, "{svg}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid class_def_default`
Expected: FAIL — classes stay empty; zero styled nodes.

- [ ] **Step 3: Implement** — in `parse()`, after the Task 4 pass, before `Ok(p.g)`:

```rust
    // mermaid auto-applies the class named `default` to every node with
    // no explicit class assignment (explicitly-classed nodes keep their
    // own resolution order untouched).
    if p.g.class_defs.iter().any(|d| d.name == "default") {
        for n in &mut p.g.nodes {
            if n.classes.is_empty() {
                n.classes.push("default".to_string());
            }
        }
    }
```

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS (existing classDef tests never define a class named `default` — verified).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
git add crates/mermaid/src/flowchart/parse.rs crates/mermaid/src/flowchart/svg.rs
git commit -m "feat(mermaid): classDef default auto-applies to unclassed flowchart nodes

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: Subroutine shape `A[[text]]`

**Files:**
- Modify: `crates/mermaid/src/flowchart/mod.rs` (ShapeKind variant)
- Modify: `crates/mermaid/src/flowchart/parse.rs` (bracket TABLE row)
- Modify: `crates/mermaid/src/flowchart/shapes.rs` (`size_for` + `emit` arms)
- Tests: new tests in `parse.rs` and `shapes.rs`

**Interfaces:**
- Produces: `ShapeKind::Subroutine` — rect with two inner vertical bars, the 14th legacy shape.

- [ ] **Step 1: Write the failing tests** (parse.rs)

```rust
    #[test]
    fn subroutine_shape_parses() {
        let g = p("graph TD\nA[[call me]]");
        assert_eq!(g.nodes[0].shape, ShapeKind::Subroutine);
        assert_eq!(g.nodes[0].label, "call me");
        // Quoted form and precedence vs `[` / `[(`:
        let g = p("graph TD\nB[[\"quoted [x]\"]]\nC[(db)]\nD[plain]");
        assert_eq!(g.nodes[0].label, "quoted [x]");
        assert_eq!(g.nodes[1].shape, ShapeKind::Cylinder);
        assert_eq!(g.nodes[2].shape, ShapeKind::Rect);
    }
```

and (shapes.rs):

```rust
    #[test]
    fn subroutine_fits_text_and_emits_rect_with_two_bars() {
        let (w, h) = size_for(ShapeKind::Subroutine, 80.0, 19.0);
        assert!(w >= 80.0 && h >= 19.0);
        let svg = emit(ShapeKind::Subroutine, 100.0, 50.0, 120.0, 40.0);
        assert!(svg.contains("<rect"), "{svg}");
        assert_eq!(svg.matches("<line").count(), 2, "{svg}");
        assert!(svg.contains("currentColor") && !svg.contains("NaN"));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid subroutine`
Expected: FAIL — compile error (`ShapeKind::Subroutine` missing).

- [ ] **Step 3: Implement**

`mod.rs`: add `Subroutine,` to `ShapeKind` (after `Rect`).

`parse.rs` TABLE: insert as a new row ABOVE the `("[(", ...)` row (must precede the plain `("[", ...)` row; order among two-char `[`-openers is irrelevant — distinct second chars):

```rust
            ("[[", &[("]]", ShapeKind::Subroutine)]),
```

`shapes.rs` `size_for`: add arm

```rust
        ShapeKind::Subroutine => (tw + 40.0, th + 16.0),
```

`shapes.rs` `emit`: add arm

```rust
        ShapeKind::Subroutine => {
            let inset = 6.0;
            let (x1, x2, yb) = (x + inset, x + w - inset, y + h);
            format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" {common}/><line x1="{x1:.1}" y1="{y:.1}" x2="{x1:.1}" y2="{yb:.1}" stroke="currentColor" stroke-width="1"/><line x1="{x2:.1}" y1="{y:.1}" x2="{x2:.1}" y2="{yb:.1}" stroke="currentColor" stroke-width="1"/>"#
            )
        }
```

(Do NOT add `Subroutine` to the tests' `ALL` const — existing tests are immutable; the new test covers it.)

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
git add crates/mermaid/src/flowchart/mod.rs crates/mermaid/src/flowchart/parse.rs crates/mermaid/src/flowchart/shapes.rs
git commit -m "feat(mermaid): subroutine shape A[[text]] — the 14th legacy flowchart shape

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: YAML front matter skipped before kind detection

**Files:**
- Modify: `crates/mermaid/src/lib.rs` (`strip_front_matter` helper + `render()` head)
- Tests: new tests in `lib.rs` `mod tests`

**Interfaces:**
- Produces: `fn strip_front_matter(source: &str) -> Result<Option<String>, ParseError>` (private). Blank-line replacement, NOT slicing, so all downstream 1-based error lines still index the original source. `detect_kind` itself is public API and stays unchanged.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn front_matter_is_skipped_for_kind_detection_and_render() {
        let src = "---\ntitle: My chart\nconfig:\n  theme: forest\n---\ngraph TD\nA --> B";
        let out = render(src);
        assert_eq!(out.kind, DiagramKind::Flowchart);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        // Works for a non-flowchart kind too (stripping precedes detection).
        let out = render("---\ntitle: t\n---\npie\n\"A\" : 1");
        assert_eq!(out.kind, DiagramKind::Pie);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }

    #[test]
    fn front_matter_preserves_original_error_lines() {
        // Front matter occupies lines 1-3; the broken statement is on
        // line 5 of the ORIGINAL source and must be reported as 5.
        let out = render("---\ntitle: x\n---\ngraph TD\nA[unclosed");
        assert_eq!(out.error.expect("err").line, Some(5));
    }

    #[test]
    fn unterminated_front_matter_errors_at_line_1() {
        let out = render("---\ntitle: x\ngraph TD\nA");
        assert_eq!(out.kind, DiagramKind::Unknown);
        let e = out.error.expect("err");
        assert_eq!(e.line, Some(1));
        assert!(e.message.contains("front matter"), "got: {}", e.message);
    }

    #[test]
    fn dashes_not_on_line_one_are_not_front_matter() {
        // `---` as an EDGE on a later line must be untouched.
        let out = render("graph TD\nA --- B");
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid front_matter`
Expected: FAIL — first test gets `DiagramKind::Unknown`.

- [ ] **Step 3: Implement** — in `lib.rs`, above `render()`:

```rust
/// Mermaid sources may open with a YAML front-matter block
/// (`---` … `---`) carrying config/theme data we don't consume. When
/// the very first line is `---`, blank the block's lines (delimiters
/// inclusive) rather than slicing them away, so every downstream error
/// still points at the original 1-based line numbers. Contents are
/// ignored in v1.
fn strip_front_matter(source: &str) -> Result<Option<String>, ParseError> {
    match source.lines().next() {
        Some(first) if first.trim() == "---" => {}
        _ => return Ok(None),
    }
    let Some(close_rel) = source.lines().skip(1).position(|l| l.trim() == "---") else {
        return Err(ParseError {
            message: "unterminated front matter: missing closing `---`".into(),
            line: Some(1),
        });
    };
    let close_idx = close_rel + 1; // position() counted from line 2
    let blanked: Vec<&str> = source
        .lines()
        .enumerate()
        .map(|(i, l)| if i <= close_idx { "" } else { l })
        .collect();
    Ok(Some(blanked.join("\n")))
}
```

At the top of `render()`, before `let kind = detect_kind(source);`:

```rust
    let stripped;
    let source = match strip_front_matter(source) {
        Ok(None) => source,
        Ok(Some(s)) => {
            stripped = s;
            stripped.as_str()
        }
        Err(e) => {
            return RenderOutput { kind: DiagramKind::Unknown, svg: None, error: Some(e) }
        }
    };
```

(The `MAX_SOURCE_LEN` gate below now measures the blanked source; blanking only ever shortens, and the collab write-gate still measures the raw source — both stay within the shared cap contract.)

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
git add crates/mermaid/src/lib.rs
git commit -m "feat(mermaid): skip YAML front matter before kind detection, preserving error line numbers

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: Flowchart statement-soup property tests

**Files:**
- Create: `crates/mermaid/src/flowchart/props.rs`
- Modify: `crates/mermaid/src/flowchart/mod.rs` (module declaration)

**Interfaces:**
- Consumes: `crate::render`, `crate::flowchart::parse::parse`.
- Mirrors the per-family pattern of `state/props.rs` (flowchart predates it and never had one; the new operator vocabulary is exactly the input class that needs soup coverage).

- [ ] **Step 1: Create `crates/mermaid/src/flowchart/props.rs`**

```rust
//! Property tests for the flowchart pipeline (proptest, dev-only).
//! Added in the polish slice alongside the unified edge-op scanner —
//! the operator vocabulary (`- . = ~ < > o x`) is the input class most
//! worth fuzzing.

use proptest::prelude::*;

fn arb_source() -> impl Strategy<Value = String> {
    // Statement soup over the full operator vocabulary: every body
    // family, terminator, reverse head, and label spelling, plus
    // shapes, subgraphs, classes, and raw noise drawn from the
    // operator characters themselves.
    let stmt = prop_oneof![
        Just("A --> B".to_string()),
        Just("A --o B".to_string()),
        Just("A --x B".to_string()),
        Just("A---oB".to_string()),
        Just("A <--> B".to_string()),
        Just("A o--o B".to_string()),
        Just("A x--x B".to_string()),
        Just("A ~~~ B".to_string()),
        Just("A ----> B".to_string()),
        Just("A -.-> B".to_string()),
        Just("A -.- B".to_string()),
        Just("A ==> B".to_string()),
        Just("A === B".to_string()),
        Just("A--text-->B".to_string()),
        Just("A-.text.-B".to_string()),
        Just("A==text==>B".to_string()),
        Just("A-->|lbl|B".to_string()),
        Just("A[[sub]] --> B{d}".to_string()),
        Just("subgraph s".to_string()),
        Just("end".to_string()),
        Just("classDef default fill:#f9f".to_string()),
        Just("C:::default".to_string()),
        Just("s --> A".to_string()),
        "[a-zA-Z0-9_ <>ox~=.|&-]{0,24}",
    ];
    proptest::collection::vec(stmt, 0..40)
        .prop_map(|v| format!("flowchart TD\n{}", v.join("\n")))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn render_never_panics_and_xor_holds(src in arb_source()) {
        let out = crate::render(&src);
        prop_assert!(out.svg.is_some() != out.error.is_some());
    }

    /// Any successful parse references only nodes it actually created.
    #[test]
    fn successful_parses_edges_reference_real_nodes(src in arb_source()) {
        if let Ok(g) = crate::flowchart::parse::parse(&src) {
            for e in &g.edges {
                prop_assert!(e.from < g.nodes.len() && e.to < g.nodes.len());
            }
        }
    }
}
```

- [ ] **Step 2: Declare the module** — in `flowchart/mod.rs`, mirror how `state/mod.rs` declares its props module (check it first; it is `#[cfg(test)] mod props;`). Add after the existing module declarations:

```rust
#[cfg(test)]
mod props;
```

- [ ] **Step 3: Run the property tests**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid flowchart::props`
Expected: PASS (2 properties × 256 cases). If a case fails, that is a REAL scanner bug — fix the scanner, never the property.

- [ ] **Step 4: Run the full crate suite, then commit**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS.

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
git add crates/mermaid/src/flowchart/props.rs crates/mermaid/src/flowchart/mod.rs
git commit -m "test(mermaid): flowchart statement-soup property tests over the new operator vocabulary

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 9: Promote the doc gallery + full verification

**Files:**
- Create: `crates/mermaid/examples/doc_gallery.rs` (updated content below; the original untracked working copy lives at `/home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-class-er/crates/mermaid/examples/doc_gallery.rs` — do NOT copy it verbatim; the expectation notes changed)

**Interfaces:** none (dev tool; `std::fs` is fine in an example — the shipped lib stays zero-dependency).

- [ ] **Step 1: Write the updated gallery**

Create `crates/mermaid/examples/doc_gallery.rs` with exactly this content:

```rust
//! Renders the example snippets from the official mermaid flowchart
//! docs (https://mermaid.ai/open-source/syntax/flowchart.html) through
//! our renderer, writing one .svg (success) or .err.txt (error) per
//! example into target/doc-gallery/ for side-by-side visual comparison
//! with the doc's reference images.
//!
//! Run: cargo run -p ogrenotes-mermaid --example doc_gallery
//!
//! Expectation notes reflect the post-polish-slice behavior (issue #32;
//! docs/superpowers/specs/2026-07-11-mermaid-polish-design.md).

use std::fs;
use std::path::Path;

fn main() {
    // (name, source, expectation-note)
    let cases: &[(&str, &str, &str)] = &[
        // ── Basic ────────────────────────────────────────────────
        ("basic_bare_node", "flowchart TD\n    A", "match"),
        ("basic_text_node", "flowchart TD\n    A[This is the text in the box]", "match"),
        ("basic_unicode_quoted", "flowchart TD\n    A[\"This is the text in the box\"]", "match"),
        ("basic_markdown_backticks", "flowchart TD\n    A[\"`This is the text in the box`\"]", "diverge: backticks render literally (no markdown)"),
        ("front_matter", "---\ntitle: Node\n---\nflowchart LR\n    id", "match: front matter skipped (title not rendered in v1)"),
        ("direction_td", "flowchart TD\n    A --> B", "match"),
        ("direction_lr", "flowchart LR\n    A --> B", "match"),
        // ── Legacy shapes ────────────────────────────────────────
        ("shape_round", "flowchart TD\n    A(This is the text in the box)", "match"),
        ("shape_stadium", "flowchart TD\n    A([This is the text in the box])", "match"),
        ("shape_subroutine", "flowchart TD\n    A[[This is the text in the box]]", "match (added in polish slice)"),
        ("shape_cylinder", "flowchart TD\n    A[(Database)]", "match"),
        ("shape_circle", "flowchart TD\n    A((This is the text in the box))", "match"),
        ("shape_asymmetric", "flowchart TD\n    A>This is the text in the box]", "match"),
        ("shape_rhombus", "flowchart TD\n    A{This is the text in the box}", "match"),
        ("shape_hexagon", "flowchart TD\n    A{{This is the text in the box}}", "match"),
        ("shape_parallelogram", "flowchart TD\n    A[/This is the text in the box/]", "match"),
        ("shape_parallelogram_alt", "flowchart TD\n    A[\\This is the text in the box\\]", "match"),
        ("shape_trapezoid", "flowchart TD\n    A[/This is the text in the box\\]", "match"),
        ("shape_trapezoid_alt", "flowchart TD\n    A[\\This is the text in the box/]", "match"),
        ("shape_double_circle", "flowchart TD\n    A(((This is the text in the box)))", "match"),
        // ── v11.3+ @-syntax (expected to error loudly) ───────────
        ("at_syntax_multi", "flowchart TD\n    A@{ shape: rect, label: \"Rectangle\" }\n    B@{ shape: circle, label: \"Circle\" }\n    A --> B", "error (out of scope: @-syntax)"),
        ("at_syntax_icon", "flowchart TD\n    A@{ shape: icon, icon: \"fa:fa-heart\", form: \"circle\", label: \"Heart\" }", "error (out of scope)"),
        // ── Links ────────────────────────────────────────────────
        ("link_arrow", "flowchart TD\n    A-->B", "match"),
        ("link_open", "flowchart TD\n    A---B", "match"),
        ("link_pipe_text", "flowchart TD\n    A-->|text|B", "match"),
        ("link_chain_text_node", "flowchart TD\n    A-->text-->B", "match (creates a node named 'text' — same as mermaid)"),
        ("link_inline_text_spaced", "flowchart TD\n    A-- text -->B", "match"),
        ("link_dotted", "flowchart TD\n    A-.->B", "match"),
        ("link_dotted_open", "flowchart TD\n    A-.-B", "match: dotted open, no arrowhead (fixed in polish slice)"),
        ("link_dotted_text_nospace", "flowchart TD\n    A-.text.-B", "match (no-space spelling added in polish slice)"),
        ("link_dotted_text_spaced", "flowchart TD\n    A-. text .-> B", "match"),
        ("link_thick", "flowchart TD\n    A==>B", "match"),
        ("link_thick_text_nospace", "flowchart TD\n    A==text==>B", "match (no-space spelling added in polish slice)"),
        ("link_thick_text_spaced", "flowchart TD\n    A== text ==>B", "match"),
        ("link_invisible", "flowchart TD\n    A ~~~ B", "match: renders both nodes, no visible edge (layout-only)"),
        // ── Chaining ─────────────────────────────────────────────
        ("chain_simple", "flowchart TD\n    A-->B-->C", "match"),
        ("chain_fanout_mid", "flowchart TD\n    A-->B & C-->D", "match"),
        ("chain_fanout_both", "flowchart TD\n    A & B-->C & D", "match"),
        ("chain_multiline", "flowchart TD\n    A-->B-->C\n    A-->D-->C\n    B-->E", "match"),
        // ── Edge ids / animation (v11.10+) ───────────────────────
        ("edge_id", "flowchart TD\n    e1@-->A & B", "error (out of scope: edge ids)"),
        // ── New arrow types (fixed silent divergences) ───────────
        ("circle_edge", "flowchart LR\n    A --o B", "match: circle-ended edge (was a loud error pre-polish)"),
        ("cross_edge", "flowchart LR\n    A --x B", "match: cross-ended edge"),
        ("circle_edge_nospace", "flowchart TD\n    A---oB", "match: circle edge to B — mermaid's documented terminator binding (was a phantom node 'oB')"),
        ("cross_edge_nospace", "flowchart TD\n    A---xB", "match: cross edge to B (was a phantom node 'xB')"),
        ("arrow_then_o_node", "flowchart TD\n    A-->oB", "match: arrow to a node named 'oB' — the '>' already terminated the link, in mermaid too"),
        // ── Multi-directional / lengths ──────────────────────────
        ("multidir", "flowchart LR\n    A o--o B\n    B <--> C\n    C x--x D", "match: per-end heads via marker-start/marker-end"),
        ("min_length", "flowchart TD\n    A[Start] --> B{Rhombus}\n    B --> C[rect_1]\n    B --> D[rect_2]\n    B --> E[rect_3]\n    C --> F[A]\n    D --> E\n    E --> F[B]\n    F --> G[End]\n    A -----> E", "match: parses; extra length's rank-span hint not honored (quiet cosmetic divergence)"),
        ("length_with_text", "flowchart TD\n    A --text---- E", "match: long closer run with inline label"),
        // ── Special characters ───────────────────────────────────
        ("special_html", "flowchart TD\n    A[\"This is a <strong>test</strong>\"]", "diverge-by-design: we escape (literal text); mermaid renders the HTML"),
        ("entity_codes", "flowchart TD\n    A[\"This is a #35; test\"]", "diverge: entity code renders literally, not decoded to #"),
        // ── Subgraphs ────────────────────────────────────────────
        ("subgraph_titled", "flowchart TD\n    subgraph sgID[A Subgraph]\n        A-->B\n    end", "match"),
        ("subgraph_edge_to_id", "flowchart TD\n    subgraph sgID[A Subgraph]\n        A-->B\n    end\n    sgID-->C\n    C-->D", "LOUD ERROR: edge-to-subgraph routing deferred (was a silent phantom node 'sgID')"),
        ("subgraph_direction", "flowchart TD\n    subgraph sgID[A Subgraph]\n        direction LR\n        A-->B\n    end", "error (direction-in-subgraph out of scope)"),
        // ── Interaction / styling ────────────────────────────────
        ("click_callback", "flowchart TD\n    A-->B\n    click A callback \"Tooltip text\"", "error (out of scope: click)"),
        ("comments", "flowchart TD\n    A[Auslan]\n    %%this is a comment\n    A-->B[\"Christmas\"]", "match"),
        ("link_style", "flowchart LR\n    A-->B-->C-->D\n    linkStyle 3 stroke:#ff3,stroke-width:4px,color:red;", "error (out of scope: linkStyle)"),
        ("node_style", "flowchart TD\n    A-->B\n    style A fill:#f9f,stroke:#333,stroke-width:4px;", "error (out of scope: style)"),
        ("classes_inline", "flowchart TD\n    A:::someclass --> B\n    classDef someclass fill:#f9f,stroke:#333,stroke-width:4px;", "match (allowlisted props apply)"),
        ("classes_two", "flowchart TD\n    A:::first --> B:::second\n    classDef first fill:#f9f,stroke:#333,stroke-width:4px;\n    classDef second fill:#bbf,stroke:#f66,stroke-width:2px,color:#fff;", "match"),
        ("class_default", "flowchart TD\n    A --> B\n    classDef default fill:#f9f,stroke:#333,stroke-width:4px;", "match: default auto-applied to unclassed nodes (added in polish slice)"),
        ("fontawesome", "flowchart TD\n    B[fa:fa-twitter]", "diverge-by-design: renders literal text, no icon"),
        ("spaced_no_semicolons", "flowchart LR\n    A --> B --> C\n    A --> D --> C\n    B --> E", "match"),
    ];

    let out_dir = Path::new("target/doc-gallery");
    fs::create_dir_all(out_dir).expect("create out dir");

    let (mut ok, mut err) = (0usize, 0usize);
    for (name, source, note) in cases {
        let result = ogrenotes_mermaid::render(source);
        match (result.svg, result.error) {
            (Some(svg), None) => {
                fs::write(out_dir.join(format!("{name}.svg")), svg).expect("write svg");
                ok += 1;
                println!("RENDERED  {name:<28} — {note}");
            }
            (None, Some(e)) => {
                let msg = format!(
                    "line {:?}: {}\n\nsource:\n{}\n\nexpectation: {}\n",
                    e.line, e.message, source, note
                );
                fs::write(out_dir.join(format!("{name}.err.txt")), msg).expect("write err");
                err += 1;
                println!("ERRORED   {name:<28} — {note}");
            }
            other => {
                // XOR invariant means this is unreachable; keep loud anyway.
                println!("INVARIANT VIOLATION {name}: {other:?}");
            }
        }
    }
    println!("\n{ok} rendered, {err} errored → target/doc-gallery/");
    println!("Compare SVGs against the doc images; .err.txt files list the loud-error cases.");
}
```

- [ ] **Step 2: Run the gallery and eyeball the tallies**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish && cargo run -p ogrenotes-mermaid --example doc_gallery`
Expected: every case whose note begins with `match` or `diverge` prints RENDERED; every case whose note begins with `error`/`LOUD ERROR` prints ERRORED. If any case lands on the wrong side, a previous task has a bug — fix it there (do not adjust the note to match wrong behavior).

- [ ] **Step 3: Full verification battery**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
cargo test -p ogrenotes-mermaid
cargo test -p ogrenotes-collab          # consumer: export fallback + write gate
cargo fmt -p ogrenotes-mermaid --check
cargo clippy -p ogrenotes-mermaid --all-targets -- -D warnings
cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown
```
Expected: all green. (The wasm build covers the lib only — examples aren't built for wasm — confirming the crate stays wasm-clean.)

- [ ] **Step 4: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-polish
git add crates/mermaid/examples/doc_gallery.rs
git commit -m "docs(mermaid): commit the flowchart doc-example gallery with post-polish expectations

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Plan Self-Review (performed at write time)

- **Spec coverage:** scanner incl. circle/cross + multi-length + bidirectional + invisible (Task 1), no-space labels (Task 2), SVG heads + `-.-`/`===` open fix (Task 3), edge-to-subgraph loud error incl. edge-above ordering (Task 4), classDef default (Task 5), subroutine (Task 6), front matter (Task 7), soup property (Task 8), gallery promotion (Task 9). Deferred list untouched. ✓
- **Immutable-test audit:** `edge_kinds` (`-->`→Arrow, `---`→Open, `-.->`→Dotted, `==>`→Thick) — preserved by the kind-mapping rule; `inline_label` spaced forms — preserved by Task 1's compatible `parse_inline_label`; `open_edge_has_no_arrowhead`, `dotted_and_thick_edge_styles`, `simple_chain_renders` — traced compatible in Task 3; no existing test exercises `--o`/`--x`/subgraph-id edges/`classDef default`/`[[`/front matter. ✓
- **Type consistency:** `EdgeOp`, `Head`, `from_head`/`to_head`, `edge_lines` used with identical names across Tasks 1–4; `parse_inline_label` signature identical in Tasks 1–2. ✓
