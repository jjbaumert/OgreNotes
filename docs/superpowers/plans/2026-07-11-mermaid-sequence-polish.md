# Mermaid Sequence Polish (issue #45) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kill the sequence parser's semicolon silent misparse and close the cheap doc-parity gaps (spaced activation shorthand, `option` divider, bidirectional arrows, multi-participant notes, `<br/>` participant displays, keyword-naming errors), per the approved spec `docs/superpowers/specs/2026-07-11-mermaid-sequence-polish-design.md`.

**Architecture:** All parser work lands in `sequence/parse.rs` (statement splitting, arrow table, note ids, error naming); `Event::Message` gains a `from_head: Head` field (existing `head` stays the to-end head — tests assert its name); SVG work is confined to `sequence/svg.rs` (marker-start, per-line tspans in `draw_box`/`draw_actor`). No layout-algorithm changes.

**Tech Stack:** Rust workspace crate `crates/mermaid` (`ogrenotes-mermaid`); zero runtime dependencies; proptest dev-only.

## Global Constraints

- Worktree: `/home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish` (branch `worktree-mermaid-seq-polish`, base `1bb6257`). Shell cwd may reset between commands — `cd` explicitly every time.
- **Existing tests are immutable.** Additions only. Notably: `header_required` (accepts `sequenceDiagram;`), `note_placements` (asserts `Over(0, None)` / `Over(0, Some(1))` shapes), `all_arrow_forms`, `open_head_has_no_marker_reference_on_its_line` (zero `marker-end` for `A->B`), `basic_exchange_renders` (`>Alice<` appears ≥2×), the `msg()` helper matching field name `head`.
- Contract: `render()` never panics; XOR svg/error; 1-based error lines; unsupported constructs error naming their keyword; every user string through `escape_xml`; deterministic; caps unchanged (`MAX_PARTICIPANTS` 50, `MAX_EVENTS` 1000, `MAX_FRAGMENT_DEPTH` 16, `MAX_SOURCE_LEN` 20_000).
- UTF-8 discipline: `;`, `#`, and probed alphanumerics are ASCII; `char_indices` offsets of ASCII chars are boundary-safe — comment this at each new slice site per crate convention.
- NEVER bare `git stash`; never `git add -A` / `git add .` — stage files by name.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`
- Test command: `cargo test -p ogrenotes-mermaid` (full crate suite at the end of every task).

---

### Task 1: Semicolon statement splitting with entity-code guard

**Files:**
- Modify: `crates/mermaid/src/sequence/parse.rs` (module doc, `parse()` loop, new `split_statements`)
- Test: new tests in `parse.rs` `mod tests`

**Interfaces:**
- Produces: module-level `fn split_statements(line: &str) -> Vec<&str>` in `sequence/parse.rs`; `parse()` iterates fragments per line. Later tasks assume statements may arrive semicolon-split (no other task depends on the function directly).

- [ ] **Step 1: Write the failing tests** (append inside `mod tests`)

```rust
    // ── Polish slice (issue #45): semicolon statement separation ────
    // (docs/superpowers/specs/2026-07-11-mermaid-sequence-polish-design.md)

    #[test]
    fn semicolon_splits_statements_like_newlines() {
        // THE silent-misparse regression (issue #45): this used to parse
        // as ONE message with `; B-->>A: yo` inside its text.
        let g = p("sequenceDiagram\nA->>B: hi; B-->>A: yo");
        assert_eq!(g.events.len(), 2);
        assert_eq!(msg(&g.events[0]).4, "hi");
        let (from, to, _, _, text) = msg(&g.events[1]);
        assert_eq!(text, "yo");
        assert_eq!((from, to), (1, 0)); // B -> A
    }

    #[test]
    fn entity_code_semicolons_do_not_split() {
        // `#59;` / `#9829;` / `#infin;` are entity-code tokens: their
        // closing `;` is not a separator (they still render literally —
        // the accepted entity-code divergence is unchanged).
        for text in ["hi#59;there", "I #9829; you", "x #infin; y"] {
            let g = p(&format!("sequenceDiagram\nA->>B: {text}"));
            assert_eq!(g.events.len(), 1, "for {text}");
            assert_eq!(msg(&g.events[0]).4, text, "for {text}");
        }
    }

    #[test]
    fn non_entity_hash_semicolons_still_split() {
        // A `;` only gets the guard when alphanumerics directly connect
        // it back to a `#`.
        for (src, first_text) in [
            ("A->>B: x #5 9; C->>D: y", "x #5 9"),
            ("A->>B: x#; C->>D: y", "x#"),
        ] {
            let g = p(&format!("sequenceDiagram\n{src}"));
            assert_eq!(g.events.len(), 2, "for {src}");
            assert_eq!(msg(&g.events[0]).4, first_text, "for {src}");
        }
    }

    #[test]
    fn semicolon_fragments_report_their_line() {
        let e = parse("sequenceDiagram\nA->>B: ok\nA->>B: x; wibble wobble").unwrap_err();
        assert_eq!(e.line, Some(3));
    }

    #[test]
    fn header_with_same_line_statement_after_semicolon() {
        let g = p("sequenceDiagram; A->>B: hi");
        assert_eq!(g.events.len(), 1);
        assert_eq!(g.participants.len(), 2);
    }

    #[test]
    fn note_text_semicolon_terminates_the_note() {
        // mermaid: `;` ends the note statement too (docs mandate #59;);
        // the tail is then an invalid statement -> loud error.
        let e = parse("sequenceDiagram\nNote over A: watch; this").unwrap_err();
        assert_eq!(e.line, Some(2));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid sequence::parse::tests::semicolon`
Expected: FAIL — `semicolon_splits_statements_like_newlines` sees 1 event, `header_with_same_line...` errors ("must start with"), etc.

- [ ] **Step 3: Implement**

Add above `impl Parser` (module level):

```rust
/// Split a line into `;`-separated statements — mermaid treats `;` as a
/// line terminator everywhere (its docs mandate `#59;` for a literal
/// semicolon) — EXCEPT a `;` that closes an entity-code token: `#`
/// followed directly by one or more ASCII alphanumerics (`#59;`,
/// `#9829;`, `#infin;`), which stays inside its statement so entity
/// codes keep rendering literally. `;`, `#`, and the probed
/// alphanumerics are ASCII, and `char_indices` yields char-start byte
/// offsets, so every slice below lands on a char boundary.
fn split_statements(line: &str) -> Vec<&str> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in line.char_indices() {
        if c != ';' {
            continue;
        }
        // Entity-code guard: walk back over [A-Za-z0-9]+ to a `#`.
        let mut j = i;
        while j > start && b[j - 1].is_ascii_alphanumeric() {
            j -= 1;
        }
        if j > start && j < i && b[j - 1] == b'#' {
            continue; // this `;` closes `#…;` — not a separator
        }
        out.push(&line[start..i]);
        start = i + 1;
    }
    out.push(&line[start..]);
    out
}
```

Replace the body of `parse()`'s line loop (the `for (idx, raw) in source.lines().enumerate()` block) with:

```rust
    for (idx, raw) in source.lines().enumerate() {
        p.line = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        for stmt in split_statements(line) {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            if !seen_header {
                if stmt != "sequenceDiagram" {
                    return Err(p.err("sequence diagram must start with `sequenceDiagram`"));
                }
                seen_header = true;
                continue;
            }
            p.parse_statement(stmt)?;
        }
    }
```

(The old `line.strip_suffix(';')` header special-case is subsumed: `sequenceDiagram;` splits into `["sequenceDiagram", ""]` and the empty fragment is skipped — the immutable `header_required` test keeps passing.)

Update the module doc comment's first line from "Line-oriented;" to "Line-oriented; each line splits on `;` into statements (entity-code tokens like `#59;` excepted);".

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS (all pre-existing sequence tests, including `header_required` and the lib.rs adversarial list, plus the six new tests).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
git add crates/mermaid/src/sequence/parse.rs
git commit -m "fix(mermaid): sequence statements split on \`;\` like mermaid (entity codes excepted)

A->>B: hi; B-->>A: yo used to render ONE message with the tail inside
its text — a silent misparse. \`;\` is a line terminator in mermaid.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Spaced activation shorthand

**Files:**
- Modify: `crates/mermaid/src/sequence/parse.rs` (`try_parse_message`)
- Test: new test in `parse.rs` `mod tests`

**Interfaces:**
- Consumes: `try_parse_message`'s `+`/`-` strip site (the `if let Some(r) = after.strip_prefix('+')` block).

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn spaced_activation_shorthand() {
        // The docs' Background-Highlighting example spells it with
        // spaces: `Alice ->>+ John: ...` / `John -->>- Alice: ...`.
        let g = p("sequenceDiagram\nAlice ->>+ John: hi\nJohn -->>- Alice: yo");
        match &g.events[0] {
            Event::Message { activate_target, .. } => assert!(*activate_target),
            other => panic!("expected message, got {other:?}"),
        }
        match &g.events[1] {
            Event::Message { deactivate_source, .. } => assert!(*deactivate_source),
            other => panic!("expected message, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid spaced_activation`
Expected: FAIL — `expected a target participant after the arrow` (the space after `+` stops the id scan at length 0).

- [ ] **Step 3: Implement** — in `try_parse_message`, change the two strip arms to trim after stripping:

```rust
        if let Some(r) = after.strip_prefix('+') {
            activate_target = true;
            after = r.trim_start();
        } else if let Some(r) = after.strip_prefix('-') {
            deactivate_source = true;
            after = r.trim_start();
        }
```

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
git add crates/mermaid/src/sequence/parse.rs
git commit -m "feat(mermaid): accept the docs' spaced activation shorthand (A ->>+ B)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: `option` divider inside `critical`

**Files:**
- Modify: `crates/mermaid/src/sequence/parse.rs` (`parse_statement` dispatch + `parse_divider`)
- Test: new tests in `parse.rs` `mod tests`

**Interfaces:**
- Consumes: `parse_divider(stmt, keyword)`, `FragmentKind::Critical`, `Event::FragmentDivider`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn critical_uses_option_divider() {
        let g = p("sequenceDiagram\ncritical connect\nA-->B: c\noption Network timeout\nA-->A: log\noption Credentials rejected\nA-->A: log2\nend");
        assert!(matches!(&g.events[2], Event::FragmentDivider { label } if label == "Network timeout"));
        assert!(matches!(&g.events[4], Event::FragmentDivider { label } if label == "Credentials rejected"));
    }

    #[test]
    fn option_outside_critical_errors() {
        let e = parse("sequenceDiagram\nalt c\noption x\nend").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("option"), "got: {}", e.message);
        assert!(e.message.contains("critical"), "got: {}", e.message);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid option`
Expected: FAIL — `unsupported statement: "option"`.

- [ ] **Step 3: Implement** — in `parse_statement`, change the divider arm to:

```rust
            "else" | "and" | "option" => return self.parse_divider(stmt, first),
```

and in `parse_divider`, replace the `want` binding with:

```rust
        let want = match keyword {
            "else" => FragmentKind::Alt,
            "and" => FragmentKind::Par,
            _ => FragmentKind::Critical, // "option" — dispatch guarantees the set
        };
```

(The existing context check and error message — `` `option` outside an `critical` fragment `` — work unchanged; `FragmentKind::keyword()` already returns `"critical"`. The "an critical" grammar matches the pre-existing message shape for `else`/`and`; do not restructure it.)

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS (existing `par_uses_and_divider` / `else_outside_alt_errors` untouched and green).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
git add crates/mermaid/src/sequence/parse.rs
git commit -m "feat(mermaid): \`option\` divider inside \`critical\` fragments

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Bidirectional arrows `<<->>` / `<<-->>`

**Files:**
- Modify: `crates/mermaid/src/sequence/mod.rs` (`Event::Message` gains `from_head`)
- Modify: `crates/mermaid/src/sequence/parse.rs` (ARROWS table → 4-tuple, construct site)
- Modify: `crates/mermaid/src/sequence/svg.rs` (marker-start emission)
- Test: new tests in `parse.rs` and `svg.rs`

**Interfaces:**
- Produces: `Event::Message { …, head: Head, from_head: Head, … }` — `head` KEEPS its name (immutable tests match it) and stays the to-end head; `from_head` is the source-end head, `Head::None` for every non-bidirectional arrow. svg emits `marker-start` from `from_head` via the existing `marker_id`.
- Note: if any `match` on `Event::Message` elsewhere (e.g. `layout.rs`) fails to compile after the field addition, add `from_head: _` or `..` to that pattern — do not otherwise touch layout.

- [ ] **Step 1: Write the failing tests** (parse.rs)

```rust
    #[test]
    fn bidirectional_arrows() {
        for (src, want_line) in [
            ("A<<->>B: t", LineStyle::Solid),
            ("A<<-->>B: t", LineStyle::Dotted),
        ] {
            let g = p(&format!("sequenceDiagram\n{src}"));
            match &g.events[0] {
                Event::Message { line, head, from_head, text, .. } => {
                    assert_eq!(*line, want_line, "for {src}");
                    assert_eq!(*head, Head::Arrow, "for {src}");
                    assert_eq!(*from_head, Head::Arrow, "for {src}");
                    assert_eq!(text, "t", "for {src}");
                }
                other => panic!("expected message, got {other:?}"),
            }
        }
    }

    #[test]
    fn plain_arrows_have_no_source_head() {
        let g = p("sequenceDiagram\nA->>B: t");
        match &g.events[0] {
            Event::Message { from_head, .. } => assert_eq!(*from_head, Head::None),
            other => panic!("expected message, got {other:?}"),
        }
    }
```

and (svg.rs `mod tests`):

```rust
    #[test]
    fn bidirectional_message_has_markers_both_ends() {
        let svg = render_sequence("sequenceDiagram\nA<<->>B: both").unwrap();
        assert!(svg.contains(r##"marker-start="url(#mmd-arrow)""##), "{svg}");
        assert!(svg.contains(r##"marker-end="url(#mmd-arrow)""##), "{svg}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid bidirectional`
Expected: FAIL — compile error first (`from_head` unknown), then after the model lands, `unsupported statement` for `A<<->>B`.

- [ ] **Step 3: Implement**

`mod.rs` — extend `Event::Message` (field order shown; `head` doc updated):

```rust
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
```

`parse.rs` — replace the ARROWS table and its find (4-tuples, longest-first; the `<<` entries can go first — they share no prefix with the `-` entries):

```rust
        // Arrows longest-first; each maps to (line, from-head, to-head).
        const ARROWS: &[(&str, LineStyle, Head, Head)] = &[
            ("<<-->>", LineStyle::Dotted, Head::Arrow, Head::Arrow),
            ("<<->>", LineStyle::Solid, Head::Arrow, Head::Arrow),
            ("-->>", LineStyle::Dotted, Head::None, Head::Arrow),
            ("-->", LineStyle::Dotted, Head::None, Head::None),
            ("->>", LineStyle::Solid, Head::None, Head::Arrow),
            ("->", LineStyle::Solid, Head::None, Head::None),
            ("--x", LineStyle::Dotted, Head::None, Head::Cross),
            ("-x", LineStyle::Solid, Head::None, Head::Cross),
            ("--)", LineStyle::Dotted, Head::None, Head::Async),
            ("-)", LineStyle::Solid, Head::None, Head::Async),
        ];
        let Some((arrow, line_style, from_head, head)) = ARROWS
            .iter()
            .find(|(a, _, _, _)| rest.starts_with(a))
            .map(|(a, l, fh, h)| (*a, *l, *fh, *h))
        else {
            return Ok(false);
        };
```

and add `from_head,` to the `Event::Message { … }` construction at the end of `try_parse_message`.

`svg.rs` — in the message loop, destructure `from_head` (`Event::Message { from, to, line, head, from_head, text, .. }`) and add next to the existing `marker_attr`:

```rust
        let marker_start_attr = match marker_id(*from_head) {
            Some(id) => format!(r#" marker-start="url(#{id})""#),
            None => String::new(),
        };
```

then append `{marker_start_attr}` right after `{marker_attr}` in BOTH the self-message `<path …/>` format string and the straight `<line …/>` format string.

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS (`open_head_has_no_marker_reference_on_its_line` still counts zero `marker-end`, and `marker-start` never appears for `Head::None`).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
git add crates/mermaid/src/sequence/mod.rs crates/mermaid/src/sequence/parse.rs crates/mermaid/src/sequence/svg.rs
git commit -m "feat(mermaid): bidirectional sequence arrows <<->> and <<-->> via marker-start

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: `Note over` with three or more participants

**Files:**
- Modify: `crates/mermaid/src/sequence/parse.rs` (`parse_note`'s `over` branch)
- Test: new tests in `parse.rs` and `svg.rs`

**Interfaces:**
- Consumes/keeps: `NotePlacement::Over(usize, Option<usize>)` — the variant shape is UNCHANGED (immutable tests match `Over(0, None)` / `Over(0, Some(1))`); it now stores `(min, max)` participant indices across ALL listed ids. Participant index order == column order, so min/max spans the outermost lifelines; layout needs no change.

- [ ] **Step 1: Write the failing tests** (parse.rs)

```rust
    #[test]
    fn note_over_three_or_more_spans_outermost() {
        use crate::sequence::NotePlacement;
        // Every listed participant is interned; the stored pair is
        // (min, max) by index. (Was: third participant silently
        // dropped — filed on #32, fixed here.)
        let g = p("sequenceDiagram\nA->>B: x\nB->>C: y\nNote over C,A,B: all");
        assert_eq!(g.participants.len(), 3);
        assert!(matches!(&g.events[2],
            Event::Note { placement: NotePlacement::Over(0, Some(2)), .. }));
        // Note-first form creates all three lifelines.
        let g = p("sequenceDiagram\nNote over X,Y,Z: trio");
        assert_eq!(g.participants.len(), 3);
    }

    #[test]
    fn note_over_trailing_comma_errors() {
        // Previously a trailing comma was silently ignored; empty ids
        // now error like every other id-position empty.
        let e = parse("sequenceDiagram\nNote over A,: t").unwrap_err();
        assert_eq!(e.line, Some(2));
    }
```

and (svg.rs `mod tests`):

```rust
    #[test]
    fn note_over_three_spans_outer_lifelines() {
        let svg = render_sequence(
            "sequenceDiagram\nA->>B: x\nB->>C: y\nNote over A,C: wide",
        )
        .unwrap();
        // Lifelines are the dasharray-"3 3" <line> elements; the note
        // rect (note-fill) must span at least from the first to the
        // last lifeline x.
        let cols: Vec<f64> = svg
            .match_indices("<line ")
            .filter_map(|(i, _)| {
                let seg = &svg[i..i + svg[i..].find("/>").unwrap()];
                if !seg.contains("stroke-dasharray=\"3 3\"") {
                    return None;
                }
                let j = seg.find("x1=\"").unwrap() + 4;
                seg[j..].split('"').next().unwrap().parse().ok()
            })
            .collect();
        assert_eq!(cols.len(), 3, "{svg}");
        let ri = svg.find("--mermaid-note-fill").unwrap();
        let rect = &svg[svg[..ri].rfind("<rect").unwrap()..ri];
        let attr = |name: &str| -> f64 {
            let j = rect.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
            rect[j..].split('"').next().unwrap().parse().unwrap()
        };
        let (x, w) = (attr("x"), attr("width"));
        let cmin = cols.iter().cloned().fold(f64::MAX, f64::min);
        let cmax = cols.iter().cloned().fold(f64::MIN, f64::max);
        assert!(
            x <= cmin && x + w >= cmax,
            "note {x}..{} vs cols {cmin}..{cmax}: {svg}",
            x + w
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid note_over_three`
Expected: FAIL — `Note over C,A,B` yields `Over(2, Some(0))`-ish two-participant parse (only C and A interned; B never appears), participant count 2.

- [ ] **Step 3: Implement** — replace the `over` branch of `parse_note`'s `placement` computation:

```rust
        let placement = if placement_kw == "over" {
            // Mermaid allows any number of comma-separated participants;
            // the note spans the outermost lifelines. Intern every id
            // (creating lifelines as needed) and store (min, max) by
            // participant index — index order is column order.
            let mut bounds: Option<(usize, usize)> = None;
            for id in ids_part.split(',').map(str::trim) {
                if id.is_empty() {
                    return Err(self.err("note needs at least one participant id"));
                }
                self.validate_id(id)?;
                let idx = self.intern(id, None, false, false)?;
                bounds = Some(match bounds {
                    None => (idx, idx),
                    Some((lo, hi)) => (lo.min(idx), hi.max(idx)),
                });
            }
            // ids_part is non-empty (checked above), so bounds is Some.
            let (lo, hi) = bounds.unwrap_or((0, 0));
            if lo == hi {
                NotePlacement::Over(lo, None)
            } else {
                NotePlacement::Over(lo, Some(hi))
            }
        } else {
```

(the `left of` / `right of` else-branch stays exactly as it is).

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS — the immutable `note_placements` test still sees `Over(0, None)` for `Note over A` and `Over(0, Some(1))` for `Note over A,B`; `note_over_multi_word_id_errors` still errors on `A B`.

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
git add crates/mermaid/src/sequence/parse.rs crates/mermaid/src/sequence/svg.rs
git commit -m "feat(mermaid): Note over any number of participants spans the outermost lifelines

Was: the third and later ids were silently dropped (filed on #32).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: `<br/>` in participant display names

**Files:**
- Modify: `crates/mermaid/src/sequence/svg.rs` (`draw_box`, `draw_actor`)
- Test: new test in `svg.rs` `mod tests`

**Interfaces:**
- Consumes: `measure::lines` / `measure::LINE_H` (already used by the message/note emission in this file). Layout already MEASURES displays multi-line (`head_h`/`box_w`); only rendering lags.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn participant_display_line_breaks_render_as_tspans() {
        let svg = render_sequence(
            "sequenceDiagram\nparticipant A as Alice<br/>Johnson\nactor B as Bob<br/>Builder\nA->>B: x",
        )
        .unwrap();
        assert!(!svg.contains("&lt;br/&gt;"), "literal <br/> leaked: {svg}");
        // Each display line appears in top AND bottom header passes,
        // box form (Alice/Johnson) and actor form (Bob/Builder).
        for frag in [">Alice<", ">Johnson<", ">Bob<", ">Builder<"] {
            assert!(svg.matches(frag).count() >= 2, "{frag} missing: {svg}");
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid participant_display_line_breaks`
Expected: FAIL — the SVG contains `&lt;br/&gt;` (escaped literal).

- [ ] **Step 3: Implement**

Replace `draw_box`'s single `<text>` push with:

```rust
    // Display may contain <br/> line breaks; measure::lines splits the
    // same way the box width/head height were measured. Single-line
    // output is position-identical to the old code (dy = +5 baseline).
    let lines = measure::lines(label);
    let n = lines.len();
    out.push_str(&format!(
        r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">"#,
        top_y + box_h / 2.0
    ));
    for (idx, line) in lines.iter().enumerate() {
        let dy = if idx == 0 {
            -((n as f64 - 1.0) / 2.0) * measure::LINE_H + 5.0
        } else {
            measure::LINE_H
        };
        out.push_str(&format!(
            r#"<tspan x="{cx:.1}" dy="{dy:.1}">{}</tspan>"#,
            escape_xml(line)
        ));
    }
    out.push_str("</text>");
```

Replace `draw_actor`'s label `<text>` push with (first line at the old y, extra lines grow downward):

```rust
    let lines = measure::lines(label);
    out.push_str(&format!(
        r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">"#,
        head_cy + 32.0 + 14.0
    ));
    for (idx, line) in lines.iter().enumerate() {
        let dy = if idx == 0 { 0.0 } else { measure::LINE_H };
        out.push_str(&format!(
            r#"<tspan x="{cx:.1}" dy="{dy:.1}">{}</tspan>"#,
            escape_xml(line)
        ));
    }
    out.push_str("</text>");
```

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS — `basic_exchange_renders`'s `>Alice<` count still ≥2 (tspan wrapping preserves the `>text<` shape), `user_strings_escaped` unaffected.

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
git add crates/mermaid/src/sequence/svg.rs
git commit -m "feat(mermaid): render <br/> line breaks in participant display names

Boxes and actor labels were measured multi-line but rendered the
literal text on one line (filed on #32).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: Keyword-naming errors for half-arrows and central connections

**Files:**
- Modify: `crates/mermaid/src/sequence/parse.rs` (`try_parse_message` target-error path + `parse_statement` fallthrough)
- Test: new tests in `parse.rs` `mod tests`

**Interfaces:**
- Consumes: the `to_len == 0` error site inside `try_parse_message`; the final `Err(unsupported statement)` in `parse_statement`. Detection is best-effort in ERROR PATHS ONLY — it runs strictly after a statement has already failed to parse as anything supported, so it can never affect a successful parse.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn half_arrows_error_naming_the_construct() {
        for src in ["A-|\\B: t", "A-|/B: t", "A/|-B: t", "A-\\\\B: t", "A--//B: t"] {
            let e = parse(&format!("sequenceDiagram\n{src}")).unwrap_err();
            assert_eq!(e.line, Some(2), "for {src}");
            assert!(e.message.contains("half arrow"), "for {src}, got: {}", e.message);
        }
    }

    #[test]
    fn central_connections_error_naming_the_construct() {
        for src in ["Alice->>()John: x", "Alice()->>John: x", "John()->>()Alice: x"] {
            let e = parse(&format!("sequenceDiagram\n{src}")).unwrap_err();
            assert_eq!(e.line, Some(2), "for {src}");
            assert!(
                e.message.contains("central connection"),
                "for {src}, got: {}",
                e.message
            );
        }
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid naming_the_construct`
Expected: FAIL — messages are `unsupported statement: …` / `expected a target participant after the arrow`.

- [ ] **Step 3: Implement**

In `try_parse_message`, replace the `to_len == 0` error with:

```rust
        if to_len == 0 {
            // `A->>()B` — the arrow parsed, the target starts with `(`:
            // that's mermaid's central-connection syntax (v11.12.3).
            if after.starts_with('(') {
                return Err(self.err("central connections (`()`) are not supported"));
            }
            return Err(self.err("expected a target participant after the arrow"));
        }
```

In `parse_statement`, replace the final fallthrough:

```rust
        if self.try_parse_message(stmt)? {
            return Ok(());
        }
        // Best-effort recognition of known-unsupported arrow families so
        // the error names the construct. Runs only after every supported
        // parse declined, so it can never shadow a valid statement; a
        // false positive still yields a loud error, just better-labeled.
        let compact: String = stmt.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("()") && (compact.contains("->") || compact.contains("<<")) {
            // `Alice()->>John` — source-side central connection (the
            // target-side form errors inside try_parse_message).
            return Err(self.err("central connections (`()`) are not supported"));
        }
        if ["-|", "|-", "-\\", "\\-", "-/", "/-"].iter().any(|t| compact.contains(t)) {
            return Err(self.err("half arrows are not supported"));
        }
        Err(self.err(format!("unsupported statement: {first:?}")))
```

- [ ] **Step 4: Run the full crate suite**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS (`unknown_statement_errors_with_line`'s `wibble wobble` contains none of the probed tokens and keeps its generic message).

- [ ] **Step 5: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
git add crates/mermaid/src/sequence/parse.rs
git commit -m "feat(mermaid): sequence errors name half-arrows and central connections

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: Props-soup extension

**Files:**
- Modify: `crates/mermaid/src/sequence/props.rs` (strategy only — the two properties are untouched)

**Interfaces:**
- Consumes: `arb_source()`'s `prop_oneof!`.

- [ ] **Step 1: Extend the strategy** — in `arb_source()`, add these variants after the existing `Just(...)` lines and replace the noise regex (last arm):

```rust
        Just("A->>B: hi; B-->>A: yo".to_string()),
        Just("A->>B: x#59;y".to_string()),
        Just("A<<->>B: both".to_string()),
        Just("A<<-->>B: both dotted".to_string()),
        Just("Note over A,B,C: n3".to_string()),
        Just("critical c".to_string()),
        Just("option o".to_string()),
        Just("participant M as Multi<br/>Line".to_string()),
        "[a-zA-Z<>:\\-x)+;#/|() ]{0,24}",
```

(The noise alphabet gains `;`, `#`, `/`, `|`, `(`, `)` — covering the splitter, entity guard, half-arrow, and central-connection paths.)

- [ ] **Step 2: Run the property tests**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid sequence::props`
Expected: PASS (2 properties × 256 cases). If a case fails, that is a REAL bug in Tasks 1-7 — report it with proptest's shrunk input; never weaken the property or strategy.

- [ ] **Step 3: Run the full crate suite, then commit**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo test -p ogrenotes-mermaid`
Expected: PASS.

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
git add crates/mermaid/src/sequence/props.rs
git commit -m "test(mermaid): sequence soup covers ;-splitting, entity guard, bidirectional, option, central/half tokens

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 9: Promote the sequence gallery + full verification

**Files:**
- Create: `crates/mermaid/examples/seq_gallery.rs`

The analysis original lives UNTRACKED at `/home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-gap/crates/mermaid/examples/seq_gallery.rs` — do NOT copy it verbatim; the version below has post-fix expectation notes. Write the file with exactly this content:

```rust
//! Renders the example snippets from the official mermaid sequence-diagram
//! docs (https://mermaid.ai/open-source/syntax/sequenceDiagram.html, source
//! packages/mermaid/src/docs/syntax/sequenceDiagram.md) through our
//! renderer, writing one .svg (success) or .err.txt (error) per example
//! into target/seq-gallery/ for side-by-side comparison with the docs.
//!
//! Run: cargo run -p ogrenotes-mermaid --example seq_gallery
//!
//! Expectation notes reflect the post-sequence-polish behavior (issue #45;
//! docs/superpowers/specs/2026-07-11-mermaid-sequence-polish-design.md).

use std::fs;
use std::path::Path;

fn main() {
    // (name, source, expectation-note)
    let cases: &[(&str, &str, &str)] = &[
        // ── Participants / actors ────────────────────────────────
        ("intro_basic",
         "sequenceDiagram\n    Alice->>John: Hello John, how are you?\n    John-->>Alice: Great!\n    Alice-)John: See you later!",
         "match"),
        ("participants_explicit",
         "sequenceDiagram\n    participant Alice\n    participant Bob\n    Bob->>Alice: Hi Alice\n    Alice->>Bob: Hi Bob",
         "match: declaration order wins"),
        ("actors",
         "sequenceDiagram\n    actor Alice\n    actor Bob\n    Alice->>Bob: Hi Bob\n    Bob->>Alice: Hi Alice",
         "match: stick figures"),
        ("stereo_boundary",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"boundary\" }\n    participant Bob\n    Alice->>Bob: Request from boundary\n    Bob->>Alice: Response to boundary",
         "error expected (stereotype @-syntax out of scope)"),
        ("stereo_control",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"control\" }\n    participant Bob\n    Alice->>Bob: Control request\n    Bob->>Alice: Control response",
         "error expected"),
        ("stereo_entity",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"entity\" }\n    participant Bob\n    Alice->>Bob: Entity request\n    Bob->>Alice: Entity response",
         "error expected"),
        ("stereo_database",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"database\" }\n    participant Bob\n    Alice->>Bob: DB query\n    Bob->>Alice: DB result",
         "error expected"),
        ("stereo_collections",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"collections\" }\n    participant Bob\n    Alice->>Bob: Collections request\n    Bob->>Alice: Collections response",
         "error expected"),
        ("stereo_queue",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"queue\" }\n    participant Bob\n    Alice->>Bob: Queue message\n    Bob->>Alice: Queue response",
         "error expected"),
        ("alias_external",
         "sequenceDiagram\n    participant A as Alice\n    participant J as John\n    A->>J: Hello John, how are you?\n    J->>A: Great!",
         "match"),
        ("alias_external_stereo",
         "sequenceDiagram\n    participant API@{ \"type\": \"boundary\" } as Public API\n    actor DB@{ \"type\": \"database\" } as User Database\n    participant Svc@{ \"type\": \"control\" } as Auth Service\n    API->>Svc: Authenticate\n    Svc->>DB: Query user\n    DB-->>Svc: User data\n    Svc-->>API: Token",
         "error expected (stereotype)"),
        ("alias_inline_stereo",
         "sequenceDiagram\n    participant API@{ \"type\": \"boundary\", \"alias\": \"Public API\" }\n    participant Auth@{ \"type\": \"control\", \"alias\": \"Auth Service\" }\n    participant DB@{ \"type\": \"database\", \"alias\": \"User Database\" }\n    API->>Auth: Login request\n    Auth->>DB: Query user\n    DB-->>Auth: User data\n    Auth-->>API: Access token",
         "error expected"),
        ("alias_precedence",
         "sequenceDiagram\n    participant API@{ \"type\": \"boundary\", \"alias\": \"Internal Name\" } as External Name\n    participant DB@{ \"type\": \"database\", \"alias\": \"Internal DB\" } as External DB\n    API->>DB: Query\n    DB-->>API: Result",
         "error expected"),
        ("create_destroy",
         "sequenceDiagram\n    Alice->>Bob: Hello Bob, how are you ?\n    Bob->>Alice: Fine, thank you. And you?\n    create participant Carl\n    Alice->>Carl: Hi Carl!\n    create actor D as Donald\n    Carl->>D: Hi!\n    destroy Carl\n    Alice-xCarl: We are too many\n    destroy Bob\n    Bob->>Alice: I agree",
         "error expected (`create` out of scope)"),
        ("create_snippet",
         "sequenceDiagram\n    create participant B\n    A --> B: Hello",
         "error expected (`create`)"),
        ("box_purple",
         "sequenceDiagram\n    box Purple Alice & John\n    participant A\n    participant J\n    end\n    box Another Group\n    participant B\n    participant C\n    end\n    A->>J: Hello John, how are you?\n    J->>A: Great!\n    A->>B: Hello Bob, how is Charley?\n    B->>C: Hello Charley, how are you?",
         "error expected (`box` out of scope)"),
        ("box_rgb",
         "sequenceDiagram\n    box rgb(33,66,99)\n    participant A\n    end\n    A->>A: hi",
         "error expected (`box`)"),
        ("box_transparent",
         "sequenceDiagram\n    box transparent Aqua\n    participant A\n    end\n    A->>A: hi",
         "error expected (`box`)"),
        ("central_conn_target",
         "sequenceDiagram\n    participant Alice\n    participant John\n    Alice->>()John: Hello John",
         "error expected, naming central connections (added in seq-polish)"),
        ("central_conn_source",
         "sequenceDiagram\n    participant Alice\n    participant John\n    Alice()->>John: How are you?",
         "error expected, naming central connections"),
        ("central_conn_both",
         "sequenceDiagram\n    participant Alice\n    participant John\n    John()->>()Alice: Great!",
         "error expected, naming central connections"),
        // ── Messages / arrows (doc table) ────────────────────────
        ("arrow_solid_noarrow",   "sequenceDiagram\n    A->B: solid open",    "match: plain line, no marker"),
        ("arrow_dotted_noarrow",  "sequenceDiagram\n    A-->B: dotted open",  "match"),
        ("arrow_solid_head",      "sequenceDiagram\n    A->>B: solid head",   "match"),
        ("arrow_dotted_head",     "sequenceDiagram\n    A-->>B: dotted head", "match"),
        ("arrow_bidi_solid",      "sequenceDiagram\n    A<<->>B: bidirectional", "match: markers both ends (added in seq-polish)"),
        ("arrow_bidi_dotted",     "sequenceDiagram\n    A<<-->>B: bidirectional dotted", "match (added in seq-polish)"),
        ("arrow_cross_solid",     "sequenceDiagram\n    A-xB: cross",         "match: X marker"),
        ("arrow_cross_dotted",    "sequenceDiagram\n    A--xB: cross dotted", "match"),
        ("arrow_async_solid",     "sequenceDiagram\n    A-)B: async open",    "match: open-V marker"),
        ("arrow_async_dotted",    "sequenceDiagram\n    A--)B: async dotted", "match"),
        // half-arrows (v11.12.3 doc table; representative spellings)
        ("half_arrow_top",        "sequenceDiagram\n    A-|\\B: top half",    "error expected, naming half arrows (out of scope)"),
        ("half_arrow_bottom",     "sequenceDiagram\n    A-|/B: bottom half",  "error expected, naming half arrows"),
        ("half_arrow_reverse",    "sequenceDiagram\n    A/|-B: reverse top",  "error expected, naming half arrows"),
        ("half_arrow_stick",      "sequenceDiagram\n    A-\\\\B: top stick",  "error expected, naming half arrows"),
        ("half_arrow_stick_dotted","sequenceDiagram\n    A--//B: bottom stick dotted", "error expected, naming half arrows"),
        // semicolon as statement separator (doc: use #59; for a literal ;)
        ("msg_semicolon_separator",
         "sequenceDiagram\n    A->>B: hi; B-->>A: yo",
         "match: TWO messages — `;` is a statement separator (fixed in seq-polish; was one message with the tail inside its text)"),
        // ── Activations ──────────────────────────────────────────
        ("act_explicit",
         "sequenceDiagram\n    Alice->>John: Hello John, how are you?\n    activate John\n    John-->>Alice: Great!\n    deactivate John",
         "match"),
        ("act_shorthand",
         "sequenceDiagram\n    Alice->>+John: Hello John, how are you?\n    John-->>-Alice: Great!",
         "match"),
        ("act_stacked",
         "sequenceDiagram\n    Alice->>+John: Hello John, how are you?\n    Alice->>+John: John, can you hear me?\n    John-->>-Alice: Hi Alice, I can hear you!\n    John-->>-Alice: I feel great!",
         "match: stacked activation bars"),
        ("act_spaced_shorthand",
         "sequenceDiagram\n    Alice ->>+ John: Did you want to go to the game tonight?\n    John -->>- Alice: Yeah! See you there.",
         "match: spaced spelling accepted (added in seq-polish)"),
        // ── Notes / line breaks ──────────────────────────────────
        ("note_right",
         "sequenceDiagram\n    participant John\n    Note right of John: Text in note",
         "match"),
        ("note_over_two",
         "sequenceDiagram\n    Alice->John: Hello John, how are you?\n    Note over Alice,John: A typical interaction",
         "match: spans both lifelines"),
        ("note_over_three",
         "sequenceDiagram\n    Note over A,B,C: spans three?",
         "match: all three interned, note spans outermost lifelines (fixed in seq-polish; was silent drop, #32)"),
        ("linebreak_msg_note",
         "sequenceDiagram\n    Alice->John: Hello John,<br/>how are you?\n    Note over Alice,John: A typical interaction<br/>But now in two lines",
         "match: <br/> in message and note text -> tspans"),
        ("linebreak_actor_alias",
         "sequenceDiagram\n    participant Alice as Alice<br/>Johnson\n    Alice->John: Hello John,<br/>how are you?\n    Note over Alice,John: A typical interaction<br/>But now in two lines",
         "match: participant display renders per-line tspans (fixed in seq-polish; was literal, #32)"),
        // ── Fragments ────────────────────────────────────────────
        ("loop_basic",
         "sequenceDiagram\n    Alice->John: Hello John, how are you?\n    loop Every minute\n        John-->Alice: Great!\n    end",
         "match"),
        ("alt_opt",
         "sequenceDiagram\n    Alice->>Bob: Hello Bob, how are you?\n    alt is sick\n        Bob->>Alice: Not so good :(\n    else is well\n        Bob->>Alice: Feeling fresh like a daisy\n    end\n    opt Extra response\n        Bob->>Alice: Thanks for asking\n    end",
         "match"),
        ("par_basic",
         "sequenceDiagram\n    par Alice to Bob\n        Alice->>Bob: Hello guys!\n    and Alice to John\n        Alice->>John: Hello guys!\n    end\n    Bob-->>Alice: Hi Alice!\n    John-->>Alice: Hi Alice!",
         "match"),
        ("par_nested",
         "sequenceDiagram\n    par Alice to Bob\n        Alice->>Bob: Go help John\n    and Alice to John\n        Alice->>John: I want this done today\n        par John to Charlie\n            John->>Charlie: Can we do this today?\n        and John to Diana\n            John->>Diana: Can you help us today?\n        end\n    end",
         "match: nested par"),
        ("critical_options",
         "sequenceDiagram\n    critical Establish a connection to the DB\n        Service-->DB: connect\n    option Network timeout\n        Service-->Service: Log error\n    option Credentials rejected\n        Service-->Service: Log different error\n    end",
         "match: option dividers (added in seq-polish)"),
        ("critical_bare",
         "sequenceDiagram\n    critical Establish a connection to the DB\n        Service-->DB: connect\n    end",
         "match"),
        ("break_basic",
         "sequenceDiagram\n    Consumer-->API: Book something\n    API-->BookingService: Start booking process\n    break when the booking process fails\n        API-->Consumer: show failure\n    end\n    API-->BillingService: Start billing process",
         "match"),
        ("rect_highlight",
         "sequenceDiagram\n    participant Alice\n    participant John\n\n    rect rgb(191, 223, 255)\n    note right of Alice: Alice calls John.\n    Alice->>+John: Hello John, how are you?\n    rect rgb(200, 150, 255)\n    Alice->>+John: John, can you hear me?\n    John-->>-Alice: Hi Alice, I can hear you!\n    end\n    John-->>-Alice: I feel great!\n    end\n    Alice ->>+ John: Did you want to go to the game tonight?\n    John -->>- Alice: Yeah! See you there.",
         "error expected (`rect` out of scope; the spaced shorthand inside is supported since seq-polish but `rect` errors first)"),
        ("rect_rgba",
         "sequenceDiagram\n    rect rgba(0, 0, 255, .1)\n    A->>B: x\n    end",
         "error expected (`rect`)"),
        ("comments",
         "sequenceDiagram\n    Alice->>John: Hello John, how are you?\n    %% this is a comment\n    John-->>Alice: Great!",
         "match: %% lines skipped"),
        // ── Escaping / autonumber / menus ────────────────────────
        ("entity_codes",
         "sequenceDiagram\n    A->>B: I #9829; you!\n    B->>A: I #9829; you #infin; times more!",
         "diverge: entity codes render literally, not decoded (their `;` does NOT split — entity guard)"),
        ("entity_semicolon",
         "sequenceDiagram\n    A->>B: hi#59;there",
         "diverge: #59; renders literally instead of `;` — but stays ONE message (entity guard on the splitter)"),
        ("autonumber_basic",
         "sequenceDiagram\n    autonumber\n    Alice->>John: Hello John, how are you?\n    loop HealthCheck\n        John->>John: Fight against hypochondria\n    end\n    Note right of John: Rational thoughts!\n    John-->>Alice: Great!\n    John->>Bob: How about you?\n    Bob-->>John: Jolly good!",
         "match: numbers on arrows incl. self-message; rendered as inline label prefix (mermaid draws a boxed number)"),
        ("autonumber_args",
         "sequenceDiagram\n    autonumber 10 10\n    Alice->>John: Hello\n    John-->>Alice: Hi",
         "error expected (start/increment args deliberately refused, v11.15 syntax)"),
        ("autonumber_off",
         "sequenceDiagram\n    autonumber\n    A->>B: one\n    autonumber off\n    B-->>A: two",
         "error expected (autonumber args refused)"),
        ("link_menu",
         "sequenceDiagram\n    participant Alice\n    participant John\n    link Alice: Dashboard @ https://dashboard.contoso.com/alice\n    link Alice: Wiki @ https://wiki.contoso.com/alice\n    link John: Dashboard @ https://dashboard.contoso.com/john\n    link John: Wiki @ https://wiki.contoso.com/john\n    Alice->>John: Hello John, how are you?\n    John-->>Alice: Great!\n    Alice-)John: See you later!",
         "error expected (`link` out of scope)"),
        ("links_json",
         "sequenceDiagram\n    participant Alice\n    participant John\n    links Alice: {\"Dashboard\": \"https://dashboard.contoso.com/alice\", \"Wiki\": \"https://wiki.contoso.com/alice\"}\n    links John: {\"Dashboard\": \"https://dashboard.contoso.com/john\", \"Wiki\": \"https://wiki.contoso.com/john\"}\n    Alice->>John: Hello John, how are you?\n    John-->>Alice: Great!\n    Alice-)John: See you later!",
         "error expected (`links` out of scope)"),
    ];

    let out_dir = Path::new("target/seq-gallery");
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
    println!("\n{ok} rendered, {err} errored → target/seq-gallery/");
}
```

- [ ] **Step 1: Write the file** exactly as above.

- [ ] **Step 2: Run the gallery**

Run: `cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish && cargo run -p ogrenotes-mermaid --example seq_gallery`
Expected: **38 rendered, 25 errored** — every case whose note begins `match`/`diverge` prints RENDERED; every `error expected` case prints ERRORED (and its `.err.txt` message names the construct where the note says so). If any case lands on the wrong side, a previous task has a bug — STOP and report BLOCKED naming the case, source, and actual output; never adjust a note to match wrong behavior.

- [ ] **Step 3: Full verification battery**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
cargo test -p ogrenotes-mermaid
cargo test -p ogrenotes-collab
cargo fmt -p ogrenotes-mermaid --check
cargo clippy -p ogrenotes-mermaid --all-targets -- -D warnings
cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown
```
Expected: tests + wasm green. fmt/clippy: the repo has KNOWN pre-existing toolchain drift (fmt diffs in files this branch never touched; clippy lints at layout/position.rs, layout/rank.rs `items_after_test_module`, parse.rs `try_parse_bracket` `map_or`, layout/order.rs needless-range-loop — all verified present on main). If fmt/clippy flag ONLY those pre-existing items, report DONE_WITH_CONCERNS listing them; if they flag code THIS slice added, fix it (behavior-preserving) and note the fix.

- [ ] **Step 4: Commit**

```bash
cd /home/kender/projects/rust/ogre/.claude/worktrees/mermaid-seq-polish
git add crates/mermaid/examples/seq_gallery.rs
git commit -m "docs(mermaid): commit the sequence doc-example gallery with post-polish expectations

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Plan Self-Review (performed at write time)

- **Spec coverage:** semicolon splitting + entity guard (Task 1), spaced shorthand (Task 2), `option` (Task 3), bidirectional (Task 4), Note-over 3+ (Task 5), `<br/>` displays (Task 6), keyword-naming errors incl. the target-side-inside-`try_parse_message` placement discovered during plan tracing (Task 7), props extension (Task 8), gallery promotion + battery (Task 9). Out-of-scope list untouched. ✓
- **Immutable-test audit:** `header_required` (`sequenceDiagram;`) — subsumed by fragment splitting; `note_placements` — `Over(min, max)` reduces to the old shapes for 1-2 participants; `msg()` helper — field `head` keeps its name, `from_head` added; `open_head_has_no_marker_reference_on_its_line` — `Head::None` emits neither marker attr; `basic_exchange_renders` `>Alice<` ≥2 — tspans preserve the `>text<` shape; `note_over_multi_word_id_errors` — `validate_id` still runs per comma-split id; `unknown_statement_errors_with_line` — "wibble wobble" hits no probe token. ✓
- **Type consistency:** `split_statements(&str) -> Vec<&str>`, `EdgeOp`-free (sequence uses the ARROWS 4-tuple), `from_head: Head` named identically in Tasks 4 parse/svg, `bounds: Option<(usize, usize)>` local to Task 5. ✓
- **Behavior changes beyond additions, all deliberate and spec-covered:** trailing comma in `Note over A,:` now errors (was silently ignored — loud beats silent); statements after `;` parse independently (the point of Task 1). ✓
