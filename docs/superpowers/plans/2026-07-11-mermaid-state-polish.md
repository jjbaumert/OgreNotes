# Mermaid State Polish (issue #47) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kill the state renderer's two silent misparses (`:::` transition targets mislabeling edges; notes drawn off-canvas) and close the cheap doc-parity gaps, per the approved spec `docs/superpowers/specs/2026-07-11-mermaid-state-polish-design.md`.

**Architecture:** Parser work in `state/parse.rs` (a `:::` tail guard, a bare-id/colon-description fallthrough branch, named keyword errors, trailing-`%%` stripping, synthetic-id note rejection); SVG work in `state/svg.rs` only (a hoisted `note_rect` helper feeding both a canvas-extent pre-pass and the emission loop, plus a single optional `<g transform>` wrapper). No layout-engine or model changes.

**Tech Stack:** Rust workspace crate `crates/mermaid`; zero runtime dependencies; proptest dev-only.

## Global Constraints

- Worktree: `/home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish` (branch `worktree-mermaid-state-polish`, base `9b862c4`). Shell cwd may reset — `cd` explicitly every time.
- **Existing tests are immutable.** Additions only. Notably: `transitions_and_labels`, `out_of_scope_statements_error_named`, `note_renders`, `basic_machine_renders`, `labels_escaped`, `transition_to_composite_id_errors`.
- Contract: `render()` never panics; XOR svg/error; 1-based error lines; unsupported constructs error naming their keyword; every user string through `escape_xml`; deterministic; caps unchanged.
- UTF-8 discipline: new probes (`:::`, `%%`, `__`, `:`) are ASCII; boundary-safe slicing commented per crate convention.
- NEVER bare `git stash`; never `git add -A` / `git add .` — stage files by name.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`
- Test command: `cargo test -p ogrenotes-mermaid` (full crate suite at the end of every task).

---

### Task 1: `:::` transition-target loud error

**Files:**
- Modify: `crates/mermaid/src/state/parse.rs` (`parse_transition` tail handling)
- Test: new tests in `parse.rs` `mod tests`

**Interfaces:**
- Consumes: `parse_transition`'s label branch (`tail.strip_prefix(':')` at the end of the function).
- Produces: a tail beginning `:::` errors before the label branch can eat it.

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run to verify RED**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish && cargo test -p ogrenotes-mermaid triple_colon`
Expected: `triple_colon_target_errors_naming_the_operator` FAILS (parse succeeds today); the other two pass already (they pin current behavior).

- [ ] **Step 3: Implement** — in `parse_transition`, replace the `let label = match tail.strip_prefix(':')` block's opening with a guard before it:

```rust
        let tail = rest.trim_start();
        // `X:::class` — the label branch below would eat the first colon
        // and render an edge labeled `::class` (silent misparse, issue
        // #47). Styling application is out of scope; error naming it.
        if tail.starts_with(":::") {
            return Err(self.err("`:::` class styling is not supported"));
        }
        let label = match tail.strip_prefix(':') {
```

- [ ] **Step 4: Full suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish
git add crates/mermaid/src/state/parse.rs
git commit -m "fix(mermaid): state \`:::\` transition targets error loudly instead of mislabeling the edge

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Named keyword errors for `classDef` / `class` / `accTitle` / `accDescr`

**Files:**
- Modify: `crates/mermaid/src/state/parse.rs` (`parse_statement` dispatch)
- Test: new test in `parse.rs` `mod tests`

**Interfaces:**
- Consumes: the `match first { … }` dispatch in `parse_statement`.

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run to verify RED**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish && cargo test -p ogrenotes-mermaid styling_and_acc`
Expected: FAIL — messages say `expected a transition …`.

- [ ] **Step 3: Implement** — in `parse_statement`, extend the `match first` (after the `"direction"` arm):

```rust
            "classDef" | "class" => {
                return Err(self.err(format!("`{first}` statements are not supported")));
            }
            _ if stmt.starts_with("accTitle") || stmt.starts_with("accDescr") => {
                let kw = if stmt.starts_with("accTitle") { "accTitle" } else { "accDescr" };
                return Err(self.err(format!("`{kw}` statements are not supported")));
            }
```

(The `accTitle`/`accDescr` arm mirrors flowchart's — their keyword is glued to `:` so a first-token match misses it.)

- [ ] **Step 4: Full suite** — `cargo test -p ogrenotes-mermaid` → PASS (the immutable `out_of_scope_statements_error_named` is unaffected; `class`-prefixed ids like `classA` still parse as ids because `first` is whole-token).

Wait — verify that trace in Step 2's RED run: `classA --> b` must still parse. Add this assertion to the Step 1 test (part of the same new test, shown here complete):

```rust
    #[test]
    fn keyword_prefixed_ids_still_parse() {
        // `first` is a whole-token match: ids that merely start with a
        // keyword are not captured by the named-error arms.
        let g = p("stateDiagram-v2\nclassA --> stateB");
        assert_eq!(g.nodes.len(), 2);
    }
```

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish
git add crates/mermaid/src/state/parse.rs
git commit -m "feat(mermaid): state errors name classDef/class/accTitle/accDescr

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Bare-id statements and colon descriptions

**Files:**
- Modify: `crates/mermaid/src/state/parse.rs` (`parse_statement` fallthrough before `parse_transition`)
- Test: new tests in `parse.rs` `mod tests`

**Interfaces:**
- Consumes: `ensure_node`, `composite_ids`, the id-scan idiom.
- Produces: a `try_parse_decl(&mut self, stmt: &str) -> Result<bool, ParseError>` helper; `parse_statement` calls it after the keyword dispatch and before `parse_transition` (`if self.try_parse_decl(stmt)? { return Ok(()); }`).

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run to verify RED**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish && cargo test -p ogrenotes-mermaid colon_description`
Expected: FAIL — `expected a transition (e.g. \`-->\`), found ": This is a state description"` etc. (`bare_id_declares_a_state` fails with `found ""`.)

- [ ] **Step 3: Implement**

Add to `impl Parser` (above `parse_transition`):

```rust
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
            Some(rest.trim().to_string())
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
```

In `parse_statement`, replace the final line:

```rust
        if self.try_parse_decl(stmt)? {
            return Ok(());
        }
        self.parse_transition(stmt)
```

- [ ] **Step 4: Full suite** — `cargo test -p ogrenotes-mermaid` → PASS. (Trace against immutable tests: `a --> b: x` — after the id, `--> …` is neither empty nor `:`-prefixed → Ok(false) → transition ✓. `[*] --> x` — id_len 0 → Ok(false) ✓. `}`/keywords — dispatched before the fallthrough ✓.)

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish
git add crates/mermaid/src/state/parse.rs
git commit -m "feat(mermaid): state bare-id declarations and colon descriptions

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Trailing `%%` comments

**Files:**
- Modify: `crates/mermaid/src/state/parse.rs` (line loop in `parse()` + helper)
- Test: new tests in `parse.rs` `mod tests`

**Interfaces:**
- Produces: module-level `fn strip_trailing_comment(line: &str) -> &str`; applied in `parse()` after the full-line comment check, before the `;` split (comments run to end of LINE, like mermaid).

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run to verify RED**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish && cargo test -p ogrenotes-mermaid trailing_comment`
Expected: the first two FAIL (`unexpected text after transition target: "%% another comment"`); the third passes already (pins current behavior).

- [ ] **Step 3: Implement**

Module-level helper (near the top of `parse.rs`):

```rust
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
```

In `parse()`'s line loop, after the `line.is_empty() || line.starts_with("%%")` check:

```rust
        let line = strip_trailing_comment(line).trim_end();
        if line.is_empty() {
            continue;
        }
```

(placed before the header check and the `;` split so the comment covers the whole rest of the line; a line REDUCED to emptiness is skipped like a blank line).

- [ ] **Step 4: Full suite** — `cargo test -p ogrenotes-mermaid` → PASS (`comments`-style full-line handling unchanged; labels with `%%` glued to text keep it, spec-documented divergence only for whitespace-preceded `%%`).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish
git add crates/mermaid/src/state/parse.rs
git commit -m "feat(mermaid): state diagrams accept trailing %% comments

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Notes on synthetic ids error loudly

**Files:**
- Modify: `crates/mermaid/src/state/parse.rs` (`parse_note`)
- Test: new test in `parse.rs` `mod tests`

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run to verify RED**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish && cargo test -p ogrenotes-mermaid note_on_synthetic`
Expected: FAIL (parses; a phantom node named `__start_0` appears).

- [ ] **Step 3: Implement** — in `parse_note`, after the id-charset validation and before `ensure_node`:

```rust
        if id_part.starts_with("__") {
            return Err(self.err("cannot attach a note to a synthetic state id"));
        }
```

- [ ] **Step 4: Full suite** — `cargo test -p ogrenotes-mermaid` → PASS (`notes_both_sides` uses plain ids).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish
git add crates/mermaid/src/state/parse.rs
git commit -m "fix(mermaid): state notes on synthetic __ ids error instead of minting a phantom node

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: Notes join the canvas (viewBox union + translate wrapper)

**Files:**
- Modify: `crates/mermaid/src/state/svg.rs`
- Test: new tests in `svg.rs` `mod tests`

**Interfaces:**
- Produces: `fn note_rect(l: &Layout, sizes: &[(f64, f64)], note: &StateNote) -> (f64, f64, f64, f64)` (x, y, w, h) — used by BOTH the pre-pass and the emission loop; add `StateNote` to the `use crate::state::…` import.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn left_note_is_inside_the_canvas() {
        // The docs' own example used to put the note rect at x≈-216 in a
        // 113-wide viewBox — invisible while render() succeeded.
        let svg = render_state(
            "stateDiagram-v2\nState1 --> State2\nnote left of State2 : This is the note to the left.",
        )
        .unwrap();
        let (w, _h) = view_size(&svg);
        for (x, bw) in note_rects(&svg) {
            assert!(x >= 0.0, "note off-canvas left: x={x} in {svg}");
            assert!(x + bw <= w + 0.5, "note off-canvas right: {} > {w} in {svg}", x + bw);
        }
        // The translate wrapper exists exactly because the note extended
        // past the left edge.
        assert!(svg.contains("<g transform=\"translate("), "{svg}");
    }

    #[test]
    fn right_note_extends_the_canvas_without_a_wrapper() {
        let svg = render_state("stateDiagram-v2\nA --> B\nnote right of A: a fairly long note text").unwrap();
        let (w, _h) = view_size(&svg);
        for (x, bw) in note_rects(&svg) {
            assert!(x >= 0.0 && x + bw <= w + 0.5, "{svg}");
        }
        assert!(!svg.contains("<g transform=\"translate("), "no left/top overflow: {svg}");
    }

    #[test]
    fn no_notes_means_no_wrapper_and_unchanged_size() {
        let svg = render_state("stateDiagram-v2\nA --> B").unwrap();
        assert!(!svg.contains("<g transform"), "{svg}");
    }

    // Test helpers (module-level in `mod tests`):
    fn view_size(svg: &str) -> (f64, f64) {
        let vb: Vec<f64> = svg.split("viewBox=\"").nth(1).unwrap()
            .split('"').next().unwrap()
            .split(' ').map(|v| v.parse().unwrap()).collect();
        (vb[2], vb[3])
    }

    /// (x, width) of every note rect (identified by the note fill var).
    fn note_rects(svg: &str) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        let mut rest = svg;
        while let Some(fi) = rest.find("--mermaid-note-fill") {
            let rect_start = rest[..fi].rfind("<rect").unwrap();
            let rect = &rest[rect_start..fi];
            let attr = |name: &str| -> f64 {
                let j = rect.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
                rect[j..].split('"').next().unwrap().parse().unwrap()
            };
            out.push((attr("x"), attr("width")));
            rest = &rest[fi + 1..];
        }
        out
    }
```

**Important nuance for the x/width assertions:** the emitted note `x` is in PRE-translate coordinates when the wrapper exists; the on-canvas position is `x + dx`. The helper reads raw attribute values, so for the wrapped case assert using the translate offset:

Replace `left_note_is_inside_the_canvas`'s body with this complete version (use THIS, not the sketch above):

```rust
    #[test]
    fn left_note_is_inside_the_canvas() {
        let svg = render_state(
            "stateDiagram-v2\nState1 --> State2\nnote left of State2 : This is the note to the left.",
        )
        .unwrap();
        let (w, _h) = view_size(&svg);
        let dx = translate_dx(&svg);
        assert!(dx > 0.0, "expected a translate wrapper: {svg}");
        for (x, bw) in note_rects(&svg) {
            let on_canvas_x = x + dx;
            assert!(on_canvas_x >= -0.5, "note off-canvas left: {on_canvas_x} in {svg}");
            assert!(on_canvas_x + bw <= w + 0.5, "note off-canvas right in {svg}");
        }
    }

    /// dx of the single translate wrapper, or 0.0 when absent.
    fn translate_dx(svg: &str) -> f64 {
        svg.split("<g transform=\"translate(").nth(1)
            .and_then(|s| s.split(',').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    }
```

(And in `right_note_extends_the_canvas_without_a_wrapper`, raw x is on-canvas since there is no wrapper.)

- [ ] **Step 2: Run to verify RED**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish && cargo test -p ogrenotes-mermaid note_is_inside`
Expected: `left_note_is_inside_the_canvas` FAILS (no wrapper; note x negative). `right_note…` FAILS (note extends past `w`).

- [ ] **Step 3: Implement**

Import: `use crate::state::{StateGraph, StateKind, StateNote};`

Module-level helper (above `emit`):

```rust
/// Geometry of one note's box — shared by the canvas-extent pre-pass
/// and the emission loop so the two cannot drift.
fn note_rect(l: &Layout, sizes: &[(f64, f64)], note: &StateNote) -> (f64, f64, f64, f64) {
    let (cx, cy) = l.node_centers[note.state];
    let (nw, _nh) = sizes[note.state];
    let (tw, th) = measure::text_size(&note.text);
    let (bw, bh) = (tw + 16.0, th + 12.0);
    let x = if note.right {
        cx + nw / 2.0 + 12.0
    } else {
        cx - nw / 2.0 - 12.0 - bw
    };
    (x, cy - bh / 2.0, bw, bh)
}
```

At the top of `emit`, replace `let (w, h) = l.size;` and the `<svg …>` format with:

```rust
    let (lw, lh) = l.size;
    // Notes are a post-layout overlay; union their rects into the
    // canvas so none can land off-canvas. They may still OVERLAP other
    // content (accepted v1 behavior) — but never be invisible.
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (0.0f64, 0.0f64, lw, lh);
    for note in &g.notes {
        let (x, y, bw, bh) = note_rect(l, sizes, note);
        min_x = min_x.min(x - 4.0);
        min_y = min_y.min(y - 4.0);
        max_x = max_x.max(x + bw + 4.0);
        max_y = max_y.max(y + bh + 4.0);
    }
    let (dx, dy) = (-min_x, -min_y); // ≥ 0 by construction (mins start at 0)
    let (w, h) = (max_x - min_x, max_y - min_y);
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
```

After the `<defs>…</defs>` push, add:

```rust
    // One wrapper re-homes everything when a note overflowed left/top;
    // per-element coordinates stay untouched.
    let wrapped = dx > 0.0 || dy > 0.0;
    if wrapped {
        out.push_str(&format!(r#"<g transform="translate({dx:.1},{dy:.1})">"#));
    }
```

Before the final `out.push_str("</svg>");`:

```rust
    if wrapped {
        out.push_str("</g>");
    }
```

In the note-emission loop, replace the inline geometry (the `(cx, cy)`, `(nw, _nh)`, `(tw, th)`, `(bw, bh)`, `x`, `y` computations) with:

```rust
        let (x, y, bw, bh) = note_rect(l, sizes, note);
```

keeping everything from the `<rect …>` push onward identical (`ncx`/`ncy` derive from `x`/`y`/`bw`/`bh` as today). Delete the now-stale "can overlap … accepted for v1" comment sentence about notes being invisible if any; keep the overlap sentence. Also update `text_size` usage: the emission loop no longer needs its own `measure::text_size` call.

- [ ] **Step 4: Full suite** — `cargo test -p ogrenotes-mermaid` → PASS (`note_renders` still finds the fill token; `basic_machine_renders` has no notes → size unchanged, no wrapper).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish
git add crates/mermaid/src/state/svg.rs
git commit -m "fix(mermaid): state notes are unioned into the canvas — no more off-canvas invisible notes

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: Props-soup extension

**Files:**
- Modify: `crates/mermaid/src/state/props.rs` (`arb_source()` only; the two properties untouched)

- [ ] **Step 1: Extend the strategy** — add these `Just(...)` variants alongside the existing ones, and extend the noise regex's character class with `:` and `%` (keep `-` last):

```rust
        Just("bareId".to_string()),
        Just("s2 : a description".to_string()),
        Just("[*] --> Still:::notMoving".to_string()),
        Just("a --> b %% trailing comment".to_string()),
        Just("note right of __start_0: boo".to_string()),
```

Current noise arm is `"[a-zA-Z0-9_\\-: ]{0,24}"` — replace with:

```rust
        "[a-zA-Z0-9_:%<>\\- ]{0,24}",
```

- [ ] **Step 2: Run** — `cargo test -p ogrenotes-mermaid state::props` → PASS (512 runs). A failure is a REAL bug in Tasks 1-6: report the shrunk input, never weaken the strategy.

- [ ] **Step 3: Full suite + commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish
cargo test -p ogrenotes-mermaid
git add crates/mermaid/src/state/props.rs
git commit -m "test(mermaid): state soup covers bare ids, colon descriptions, :::, trailing %%, synthetic-note rejection

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: Rebuild + promote the state gallery, full verification

**Files:**
- Create: `crates/mermaid/examples/state_gallery.rs`

**Context:** the analysis worktree holding the original untracked gallery was deleted before promotion, so this task REBUILDS it. The doc page is client-rendered — the static HTML has no sources and WebFetch summaries hallucinate examples. Extract deterministically:

1. Fetch https://mermaid.ai/open-source/syntax/stateDiagram.html (WebFetch, ask for the raw `<script>` asset URLs); locate the VitePress payload script matching `assets/syntax_stateDiagram.md.*.js`; fetch THAT URL and extract the mermaid code blocks (JSON-escaped strings in the payload; unescape `\n` etc.). Expect **exactly 20** mermaid blocks — if you get a different count, stop and report BLOCKED with what you found.
2. Build the gallery file with the same structure as the committed `crates/mermaid/examples/doc_gallery.rs` / `seq_gallery.rs` (a `(name, source, note)` table, output to `target/state-gallery/`, RENDERED/ERRORED prints, INVARIANT VIOLATION arm, final tally line). Header doc comment: committed example, run command, reference to the spec.
3. Doc-example expected outcomes (name them `doc01_…`–`doc20_…` in page order):
   - intro (front-matter title, Still/Moving/Crash) → `match: front-matter title not rendered (quiet cosmetic)`
   - v1 `stateDiagram` header variant → `match`
   - bare `stateId` → `match (bare-id declarations added in state-polish)`
   - `state "…" as s2` → `match`
   - `s2 : description` → `match (colon descriptions added in state-polish)`
   - `s1 --> s2` → `match`
   - `s1 --> s2: A transition` → `match`
   - `[*]` start/end → `match`
   - the three composite examples → `error expected (composite used as a transition endpoint — routing deferred, issue #47)`
   - choice → `diverge: we inscribe the id in the diamond; mermaid draws it small and empty`
   - fork/join → `match`
   - notes example (contains a multi-line `note right of … end note` block) → `error expected (multi-line notes unsupported; single-line colon notes now render on-canvas)`
   - concurrency → `error expected`
   - `direction` → `error expected`
   - comments (incl. trailing `%%`) → `match (trailing %% comments added in state-polish)`
   - classDef/class example → `error expected, naming the first unsupported statement on the earliest line (state which keyword that is for the extracted source)`
   - `:::` operator example → `error expected (whichever of direction/::: comes first in the source — note which)`
   - spaces-in-state-names example (colon description + classDef + `:::`) → `error expected, naming classDef`
4. Probe cases (recreate; exact sources):
   - `stateDiagram-v2\n[*] --> Still:::notMoving` → `error expected, naming ::: (fixed silent misparse)`
   - `stateDiagram-v2\nStill:::notMoving --> [*]` → `error expected`
   - `stateDiagram-v2\nState1 --> State2\nnote left of State2 : This is the note to the left.` → `match: note on-canvas (fixed silent divergence)`
   - `stateDiagram-v2\nA --> B\nnote right of A: on the right` → `match: note on-canvas`
   - `stateDiagram-v2\nstate X {\na --> b\n}` → `match: composite never used as endpoint`
   - `stateDiagram-v2\nc --> X\nstate X {\na --> b\n}` → `error expected (endpoint before composite decl → already-a-state guard)`
   - `stateDiagram-v2\n[*]--> A` → `match: no-space [*] arrow`
   - `stateDiagram-v2\na --> b: go;` → `diverge: label truncated at ; (filed on #32)`
   - `stateDiagram-v2\na --> b; c --> d` → `match: ; splits statements`
   - `stateDiagram-v2; s1 --> s2` → `error expected (header cannot chain — filed on #32)`
   - `stateDiagram-v2\n[*] --> A\nnote right of __start_0: boo` → `error expected, naming synthetic (fixed phantom)`
   - `stateDiagram-v2\na --> b\n--` → `error expected, naming --`
   - `stateDiagram-v2\ns : first\ns : second` → `match: repeated descriptions stack`
   - `stateDiagram-v2\na --> b: 50% done` → `match: single % survives`
   - four named-error probes (`classDef x fill:red`, `class a b`, `accTitle: t`, `accDescr: d` each after `a --> b`) → `error expected, naming the keyword`
5. Run `cargo run -p ogrenotes-mermaid --example state_gallery`. **The acceptance criterion is note-consistency, not a pre-baked tally**: every `match`/`diverge` case RENDERED, every `error expected` case ERRORED (and `.err.txt` names the construct where the note says so). Report the actual tally. If any case lands on the wrong side, a previous task has a bug — STOP and report BLOCKED naming the case, source, and actual output; never adjust a note to fit.
6. Verification battery (all green, with the known escape hatch):

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish
cargo test -p ogrenotes-mermaid
cargo test -p ogrenotes-collab
cargo fmt -p ogrenotes-mermaid --check
cargo clippy -p ogrenotes-mermaid --all-targets -- -D warnings
cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown
```

fmt/clippy have KNOWN pre-existing repo-wide drift (verified on main): fmt diffs in untouched files; clippy at layout/position.rs + layout/rank.rs (`items_after_test_module`), flowchart/parse.rs `map_or`, layout/order.rs needless-range-loop. Only-those → DONE_WITH_CONCERNS listing them; anything in THIS slice's code → fix (behavior-preserving) and note it.

- [ ] **Step 1: Extract the 20 doc sources** (method above).
- [ ] **Step 2: Write the gallery** (structure + expectations above).
- [ ] **Step 3: Run it; verify note-consistency; record the tally.**
- [ ] **Step 4: Run the battery.**
- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-state-polish
git add crates/mermaid/examples/state_gallery.rs
git commit -m "docs(mermaid): commit the state doc-example gallery with post-polish expectations

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Plan Self-Review (performed at write time)

- **Spec coverage:** `:::` target error (Task 1), named keyword errors (Task 2), bare-id + colon descriptions incl. append semantics and composite collision (Task 3), trailing `%%` (Task 4), synthetic-id note rejection (Task 5), note canvas union + wrapper (Task 6), props (Task 7), gallery rebuild + battery (Task 8). Out-of-scope list untouched. ✓
- **Immutable-test audit:** `transitions_and_labels` (labels with `:` inside — Task 1 guard only fires on `:::` prefix; Task 3 declines on `-->`-bearing statements); `out_of_scope_statements_error_named` (`--`, `[H]`, `direction` arms untouched); `note_renders` (right note, fill token unchanged, extents grow rightward only); `basic_machine_renders` (no notes → identical output); `labels_escaped`; `multiline_note_block_errors` (Task 5's `__` check sits after placement/colon handling — the block form still errors on the missing colon first). ✓
- **Type consistency:** `try_parse_decl(&mut self, &str) -> Result<bool, ParseError>`; `strip_trailing_comment(&str) -> &str`; `note_rect(&Layout, &[(f64,f64)], &StateNote) -> (f64, f64, f64, f64)`; test helpers `view_size`/`note_rects`/`translate_dx` defined in the same `mod tests` that uses them. ✓
- **Known deviation from the no-placeholders rule:** Task 8 cannot embed the 20 doc sources verbatim (the original gallery was deleted with the analysis worktree; sources live on the web). The task instead pins the extraction method, the exact expected count (20), per-case expected outcomes, and the note-consistency acceptance criterion — with a BLOCKED escape hatch if extraction disagrees. ✓
