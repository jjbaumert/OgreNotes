# Mermaid Styling Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `classDef` / `class` / `cssClass` / `:::` / `style` / `linkStyle` render (instead of erroring) in the class, state, and ER diagrams, by generalizing flowchart's existing styling engine into a shared module, with luminance-based auto-contrast text for dark-mode legibility.

**Architecture:** Extract flowchart's `sanitize_style` / `STYLE_PROPS` / `ClassDef` and its class-resolution logic into a new shared `crates/mermaid/src/style.rs`. Refactor flowchart onto it (adding auto-contrast). Give each of class/state/ER a `class_defs` list on the graph, `classes` + `style` on nodes, and `style` on edges; route the styling statements to shared helpers; apply the resolved style in each SVG emitter via a `<g style="…">` wrapper plus a `style="…"` attribute on the node rect and edge path.

**Tech Stack:** Rust (workspace crate `ogrenotes-mermaid`), pure-Rust SVG emission, `proptest` for property tests. No new dependencies.

## Global Constraints

- **No new dependencies.** `crates/mermaid/Cargo.toml` has zero runtime deps; keep it that way (std only).
- **CSS-injection boundary is `style::sanitize_style` only.** Allowlist = `fill, stroke, stroke-width, stroke-dasharray, color, font-weight, font-style, opacity`; value chars = ASCII alphanumeric plus `` #.,%- `` and space. Unknown props/values are dropped silently, never errored.
- **Unstyled diagrams must render byte-for-byte unchanged.** No `<g>` wrapper or `style=` attribute when a node/edge has no resolved style.
- **`render()` never panics; XOR svg/error; deterministic.** (Existing crate contract — property tests enforce it.)
- **Tests are behavioral contracts.** The only existing tests this plan changes are the flowchart style-string assertions (auto-contrast) and the class/state "styling not supported" rejection tests — both disclosed in the design and replaced with success-path assertions. Do not modify any other test.
- **Commit after every task** with the shown message.

---

## File Structure

- **Create** `crates/mermaid/src/style.rs` — shared styling vocabulary: allowlist, `sanitize_style`, `ClassDef`, `resolve`, auto-contrast. One responsibility: turn class/inline style inputs into a safe, resolved `style="…"` string.
- **Modify** `crates/mermaid/src/lib.rs` — register `mod style;`.
- **Modify** `crates/mermaid/src/flowchart/{mod,parse,svg}.rs` — use `style::*`; delete private copies; add auto-contrast.
- **Modify** `crates/mermaid/src/class/{mod,parse,svg}.rs` — styling fields, parser routing, SVG application.
- **Modify** `crates/mermaid/src/state/{mod,parse,svg}.rs` — same for state.
- **Modify** `crates/mermaid/src/er/{mod,parse,svg}.rs` — same for ER.

---

## Task 1: Shared `style` module + flowchart refactor

**Files:**
- Create: `crates/mermaid/src/style.rs`
- Modify: `crates/mermaid/src/lib.rs` (add `pub(crate) mod style;`)
- Modify: `crates/mermaid/src/flowchart/mod.rs` (delete `ClassDef`, re-export from `style`)
- Modify: `crates/mermaid/src/flowchart/parse.rs` (delete `STYLE_PROPS`/`sanitize_style`, call `style::sanitize_style`; `ClassDef` → `style::ClassDef`)
- Modify: `crates/mermaid/src/flowchart/svg.rs` (replace `node_style` + `combined` with `style::resolve`)
- Test: `crates/mermaid/src/style.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub(crate) const STYLE_PROPS: &[&str]`
  - `pub(crate) fn sanitize_style(styles: &str) -> String`
  - `pub(crate) struct ClassDef { pub name: String, pub style: String }`
  - `pub(crate) fn resolve(classes: &[String], inline: Option<&str>, defs: &[ClassDef]) -> Option<String>`

- [ ] **Step 1: Write the failing test** — create `crates/mermaid/src/style.rs` with only the tests + `use` at top:

```rust
//! Shared styling vocabulary for node/edge diagrams (flowchart, class,
//! state, ER): the CSS-injection allowlist, named `classDef` styles, and
//! resolution of a node's effective `style="…"` (class + inline +
//! luminance-based auto-contrast text color).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_unknown_and_unsafe() {
        // allowlisted survives; unknown prop and unsafe value dropped.
        assert_eq!(sanitize_style("fill:#f00,stroke:#333"), "fill:#f00;stroke:#333");
        assert_eq!(sanitize_style("fill:red,evil:url(x),onclick:alert"), "fill:red");
        assert_eq!(sanitize_style("fill:\"</style>"), "");
    }

    #[test]
    fn resolve_class_then_inline_then_autocontrast() {
        let defs = vec![ClassDef { name: "hot".into(), style: "fill:#f00".into() }];
        // class only -> fill + auto-contrast (dark red -> white text)
        assert_eq!(
            resolve(&["hot".into()], None, &defs).as_deref(),
            Some("fill:#f00;color:#fff")
        );
        // inline overrides via CSS last-wins ordering; explicit color wins,
        // so no auto-contrast is added.
        assert_eq!(
            resolve(&["hot".into()], Some("fill:#ffffcc;color:#111"), &defs).as_deref(),
            Some("fill:#f00;fill:#ffffcc;color:#111")
        );
        // light fill, no color -> black text
        assert_eq!(
            resolve(&[], Some("fill:#ffffcc"), &defs).as_deref(),
            Some("fill:#ffffcc;color:#000")
        );
        // nothing -> None (unstyled path)
        assert_eq!(resolve(&[], None, &defs), None);
        // non-hex fill -> no auto-contrast
        assert_eq!(resolve(&[], Some("fill:red"), &defs).as_deref(), Some("fill:red"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ogrenotes-mermaid style::`
Expected: FAIL to compile ("cannot find function `sanitize_style`").

- [ ] **Step 3: Write minimal implementation** — prepend above the test module in `crates/mermaid/src/style.rs`:

```rust
/// The style allowlist is the CSS-injection boundary: only these props,
/// and only benign value characters, survive into an emitted `style`
/// attribute. Everything else is dropped silently — styling is cosmetic
/// and mermaid's vocabulary is huge, so erroring would be hostile.
pub(crate) const STYLE_PROPS: &[&str] = &[
    "fill", "stroke", "stroke-width", "stroke-dasharray",
    "color", "font-weight", "font-style", "opacity",
];

/// Sanitize a comma-separated `prop:value` list against the allowlist,
/// returning `prop:value;`-joined survivors (possibly empty).
pub(crate) fn sanitize_style(styles: &str) -> String {
    let mut kept = Vec::new();
    for pair in styles.split(',') {
        let Some((prop, value)) = pair.split_once(':') else { continue };
        let (prop, value) = (prop.trim(), value.trim());
        let value_ok = value.chars().all(|c| c.is_ascii_alphanumeric() || " #.,%-".contains(c));
        if STYLE_PROPS.contains(&prop) && value_ok && !value.is_empty() {
            kept.push(format!("{prop}:{value}"));
        }
    }
    kept.join(";")
}

#[derive(Debug, Clone)]
pub(crate) struct ClassDef {
    pub name: String,
    pub style: String, // already sanitized at parse time
}

/// A node's effective style: the first assigned class with a non-empty
/// style (declared order), then the node's inline `style` layered on top
/// (CSS last-declaration-wins gives override semantics), then a
/// luminance-derived text `color` when a `fill` is set without one.
/// Returns `None` when nothing applies (the unstyled render path).
pub(crate) fn resolve(classes: &[String], inline: Option<&str>, defs: &[ClassDef]) -> Option<String> {
    let class_style = classes
        .iter()
        .find_map(|c| defs.iter().find(|d| &d.name == c && !d.style.is_empty()))
        .map(|d| d.style.as_str());
    let mut combined = match (class_style, inline) {
        (Some(c), Some(i)) => format!("{c};{i}"),
        (Some(c), None) => c.to_string(),
        (None, Some(i)) => i.to_string(),
        (None, None) => return None,
    };
    if let Some(color) = auto_contrast_text(&combined) {
        combined.push_str(";color:");
        combined.push_str(color);
    }
    Some(combined)
}

/// Black or white text for the style's `fill`, chosen by luminance — only
/// when a hex `fill` is present and no explicit `color` is set.
fn auto_contrast_text(style: &str) -> Option<&'static str> {
    if prop_value(style, "color").is_some() {
        return None;
    }
    let (r, g, b) = parse_hex(prop_value(style, "fill")?)?;
    let lum = 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
    Some(if lum > 140.0 { "#000" } else { "#fff" })
}

/// Value of `prop` in a `prop:value;prop:value` string (last wins).
fn prop_value<'a>(style: &'a str, prop: &str) -> Option<&'a str> {
    style
        .split(';')
        .filter_map(|p| p.split_once(':'))
        .filter(|(k, _)| k.trim() == prop)
        .last()
        .map(|(_, v)| v.trim())
}

/// Parse `#rgb` / `#rrggbb` into (r,g,b). `None` for any other form.
fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    let (r, g, b) = match h.len() {
        3 => (dup(h.get(0..1)?), dup(h.get(1..2)?), dup(h.get(2..3)?)),
        6 => (h.get(0..2)?, h.get(2..4)?, h.get(4..6)?),
        _ => return None,
    };
    Some((
        u8::from_str_radix(&r, 16).ok()?,
        u8::from_str_radix(&g, 16).ok()?,
        u8::from_str_radix(&b, 16).ok()?,
    ))
}

fn dup(nibble: &str) -> String {
    format!("{nibble}{nibble}")
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ogrenotes-mermaid style::`
Expected: PASS (2 tests).

- [ ] **Step 5: Register the module** — in `crates/mermaid/src/lib.rs`, next to the other `pub(crate) mod …;` declarations, add:

```rust
pub(crate) mod style;
```

- [ ] **Step 6: Refactor flowchart onto the shared module.**
  - In `crates/mermaid/src/flowchart/mod.rs`: delete the local `ClassDef` struct; wherever it referenced `ClassDef`, use `crate::style::ClassDef`. Update the `class_defs: Vec<...>` field type to `Vec<crate::style::ClassDef>`.
  - In `crates/mermaid/src/flowchart/parse.rs`: delete the local `STYLE_PROPS` const and `sanitize_style` fn; replace calls `Self::sanitize_style(x)` with `crate::style::sanitize_style(x)`; replace `crate::flowchart::ClassDef { .. }` with `crate::style::ClassDef { .. }`.
  - In `crates/mermaid/src/flowchart/svg.rs`: delete the `node_style` fn and the `combined` match block; replace with:

```rust
        let combined = crate::style::resolve(&n.classes, n.style.as_deref(), &g.class_defs);
        match &combined {
            Some(style) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(style))),
            None => out.push_str("<g>"),
        }
        out.push_str(&shapes::emit(n.shape, cx, cy, nw, nh, combined.as_deref()));
```

- [ ] **Step 7: Update flowchart style tests for auto-contrast** — in `crates/mermaid/src/flowchart/svg.rs` tests, update the exact style strings to include the derived color. Change the assertions:

```rust
        // was: assert!(svg.contains("fill:#f00;stroke:#900"), ...)
        assert!(svg.contains("fill:#f00;stroke:#900;color:#fff"), "node style: {svg}");
```
```rust
        // was: assert!(svg.contains("style=\"fill:#f00\""));
        assert!(svg.contains("style=\"fill:#f00;color:#fff\""));
```
```rust
        // was: assert_eq!(svg.matches("style=\"fill:#f9f\"").count(), 4, ...)
        assert_eq!(svg.matches("style=\"fill:#f9f;color:#000\"").count(), 4, "{svg}");
```
```rust
        // was: assert!(svg.contains("stroke-width=\"1\" style=\"fill:#f9f\""), ...)
        assert!(svg.contains("stroke-width=\"1\" style=\"fill:#f9f;color:#000\""), "style on shape: {svg}");
```

- [ ] **Step 8: Run the full crate suite**

Run: `cargo test -p ogrenotes-mermaid`
Expected: PASS (all tests, including the updated flowchart style tests).

- [ ] **Step 9: Commit**

```bash
git add crates/mermaid/src/style.rs crates/mermaid/src/lib.rs crates/mermaid/src/flowchart/
git commit -m "style: extract shared styling module; add auto-contrast; refactor flowchart onto it

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Class-diagram styling

**Files:**
- Modify: `crates/mermaid/src/class/mod.rs` (add `class_defs` to `ClassGraph`; `classes`/`style` to `ClassBox`)
- Modify: `crates/mermaid/src/class/parse.rs` (route `classDef`/`class`/`cssClass`/`style`; `:::` suffix; remove `:::`-rejection)
- Modify: `crates/mermaid/src/class/svg.rs` (wrap styled boxes)
- Test: same files' `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::style::{sanitize_style, ClassDef, resolve}` (Task 1).
- Produces: `ClassBox.classes: Vec<String>`, `ClassBox.style: Option<String>`, `ClassGraph.class_defs: Vec<crate::style::ClassDef>`.

- [ ] **Step 1: Write the failing test** — in `crates/mermaid/src/class/parse.rs` tests:

```rust
    #[test]
    fn class_styling_parses_and_resolves() {
        let g = p("classDiagram\nclassDef hot fill:#f00\nclass A\nA:::hot\nclass B\nstyle B fill:#0f0");
        let a = g.classes.iter().find(|c| c.id == "A").unwrap();
        assert_eq!(a.classes, vec!["hot".to_string()]);
        let b = g.classes.iter().find(|c| c.id == "B").unwrap();
        assert_eq!(b.style.as_deref(), Some("fill:#0f0"));
        assert_eq!(g.class_defs.iter().find(|d| d.name == "hot").unwrap().style, "fill:#f00");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ogrenotes-mermaid class::parse::tests::class_styling_parses_and_resolves`
Expected: FAIL (fields `classes`/`style`/`class_defs` don't exist; `:::` errors).

- [ ] **Step 3: Add model fields** — in `crates/mermaid/src/class/mod.rs`:
  - Add to `struct ClassBox`: `pub classes: Vec<String>,` and `pub style: Option<String>,`.
  - Add to `struct ClassGraph`: `pub class_defs: Vec<crate::style::ClassDef>,`.
  - Update the `ClassGraph { classes: vec![], relations: vec![] }` initializer in `class/parse.rs::parse` to `ClassGraph { classes: vec![], relations: vec![], class_defs: vec![] }`.
  - Update the `ClassBox { … }` literal in `ensure_class` to include `classes: vec![], style: None,`.

- [ ] **Step 4: Route styling statements** — in `crates/mermaid/src/class/parse.rs::parse_statement`:
  - Delete the `if stmt.contains(":::") { return Err(...) }` block.
  - In the keyword `match first`, remove `"cssClass"` / `"classDef"` / `"style"` from the "not supported" arm and add:

```rust
            "classDef" => return self.parse_class_def(stmt),
            "style" => return self.parse_style(stmt),
            "class" | "cssClass" => {
                if let Some(res) = self.try_class_assign(stmt) {
                    return res;
                }
                // fall through: `class Foo { … }` declaration handled below
            }
```
  Keep `"class" => return self.parse_class_stmt(stmt)` reachable only for the declaration form — implement `try_class_assign` to return `Some(Ok(()))` when the statement is the assignment form (`class A,B name` / `cssClass "A,B" name`) and `None` when it's a `class Foo`/`class Foo {` declaration (so the existing `parse_class_stmt` runs). Add these methods:

```rust
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

    /// `style <id> prop:val,...`
    fn parse_style(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("style").unwrap().trim();
        let Some((id, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("style needs a class id and styles"));
        };
        let idx = self.ensure_class(id.trim())?;
        let s = crate::style::sanitize_style(styles);
        if !s.is_empty() {
            self.g.classes[idx].style = Some(s);
        }
        Ok(())
    }

    /// `class A,B styleName` / `cssClass "A,B" styleName`. Returns `None`
    /// when `stmt` is instead a `class Foo`/`class Foo {` declaration.
    fn try_class_assign(&mut self, stmt: &str) -> Option<Result<(), ParseError>> {
        let kw = if stmt.starts_with("cssClass") { "cssClass" } else { "class" };
        let rest = stmt.strip_prefix(kw).unwrap().trim();
        // Declaration form (single id, optional `{` / `["label"]`): not an
        // assignment. Assignment needs a trailing style name after a space.
        let (ids, name) = rest.rsplit_once(char::is_whitespace)?;
        let name = name.trim();
        if name.is_empty() || name == "{" || name.starts_with('[') {
            return None;
        }
        // ids may be quoted (`cssClass "A,B"`); strip quotes.
        let ids = ids.trim().trim_matches('"');
        for id in ids.split(',') {
            let id = id.trim();
            if let Err(e) = self.validate_id(id) {
                return Some(Err(e));
            }
            let idx = match self.ensure_class(id) {
                Ok(i) => i,
                Err(e) => return Some(Err(e)),
            };
            self.g.classes[idx].classes.push(name.to_string());
        }
        Some(Ok(()))
    }
```
  Note: the existing `"class" => return self.parse_class_stmt(stmt)` arm must now be reached only when `try_class_assign` returns `None`. Restructure the match arm as shown (call `try_class_assign` first; on `None`, call `self.parse_class_stmt(stmt)`).

- [ ] **Step 5: Handle the `:::` suffix on a class id** — in `parse_class_stmt`, after computing `id` and before the generic/`[`/`{` handling, peel an optional `:::className`:

```rust
        // `class A:::styleName` — attach a style class to the node.
        if let Some(colon) = after.strip_prefix(":::") {
            let n = colon.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
            if n == 0 {
                return Err(self.err("expected a class name after `:::`"));
            }
            self.g.classes[idx].classes.push(colon[..n].to_string());
            after = colon[n..].trim_start();
        }
```
  Also allow `:::` on a bare relationship/member id: in `parse_member_or_relationship`, before the operator scan, if `before` (trimmed, single token) contains `:::`, split it and record the class. (Minimal: handle the `class X:::name` and standalone `A:::name` forms; the standalone `A:::name` statement reaches `parse_member_or_relationship` with `before = "A:::name"`, no `:` label and no operator.) Add at the top of `parse_member_or_relationship`:

```rust
        if let Some((id, cls)) = stmt.split_once(":::") {
            let id = id.trim();
            let cls = cls.trim();
            if !id.is_empty() && !cls.is_empty() && !cls.contains(char::is_whitespace) {
                self.validate_id(id)?;
                let idx = self.ensure_class(id)?;
                self.g.classes[idx].classes.push(cls.to_string());
                return Ok(());
            }
        }
```

- [ ] **Step 6: Run the parse test to verify it passes**

Run: `cargo test -p ogrenotes-mermaid class::parse::tests::class_styling_parses_and_resolves`
Expected: PASS.

- [ ] **Step 7: Write the failing SVG test** — in `crates/mermaid/src/class/svg.rs` tests:

```rust
    #[test]
    fn class_style_applied_to_box() {
        let svg = crate::render("classDiagram\nclassDef hot fill:#f00\nclass A\nA:::hot").svg.unwrap();
        // resolved style on the box rect + group wrapper for text color.
        assert!(svg.contains("fill:#f00;color:#fff"), "styled box: {svg}");
        // unstyled diagram unchanged: no stray style= on the rect.
        let plain = crate::render("classDiagram\nclass B").svg.unwrap();
        assert!(!plain.contains("<g style="), "unstyled must not wrap: {plain}");
    }
```

- [ ] **Step 8: Run to verify it fails**

Run: `cargo test -p ogrenotes-mermaid class::svg::tests::class_style_applied_to_box`
Expected: FAIL (no style applied yet).

- [ ] **Step 9: Apply style in the SVG emitter** — in `crates/mermaid/src/class/svg.rs`, inside the `for (i, c) in g.classes.iter().enumerate()` loop, before emitting the box `<rect>`, compute and open the wrapper; add the style to the rect; close after the compartments:

```rust
        let resolved = crate::style::resolve(&c.classes, c.style.as_deref(), &g.class_defs);
        match &resolved {
            Some(s) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(s))),
            None => out.push_str("<g>"),
        }
        let box_style = match &resolved {
            Some(s) => format!(r#" style="{}""#, escape_xml(s)),
            None => String::new(),
        };
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor" rx="4"{box_style}/>"#,
            box_left, box_top, bw, bh
        ));
```
  Then, at the end of that loop iteration (after the last compartment text is emitted for this class), add:

```rust
        out.push_str("</g>");
```

- [ ] **Step 10: Run the SVG test + full suite**

Run: `cargo test -p ogrenotes-mermaid class::`
Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add crates/mermaid/src/class/
git commit -m "class: support classDef/class/cssClass/:::/style

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: State-diagram styling

**Files:**
- Modify: `crates/mermaid/src/state/mod.rs` (add `class_defs` to `StateGraph`; `classes`/`style` to `StateNode`)
- Modify: `crates/mermaid/src/state/parse.rs` (route `classDef`/`class`/`style`; `:::` target suffix)
- Modify: `crates/mermaid/src/state/svg.rs` (wrap styled state boxes)
- Test: same files' test modules

**Interfaces:**
- Consumes: `crate::style::{sanitize_style, ClassDef, resolve}`.
- Produces: `StateNode.classes: Vec<String>`, `StateNode.style: Option<String>`, `StateGraph.class_defs: Vec<crate::style::ClassDef>`.

- [ ] **Step 1: Write the failing test** — in `crates/mermaid/src/state/parse.rs` tests:

```rust
    #[test]
    fn state_styling_parses() {
        let g = p("stateDiagram-v2\nclassDef mov fill:#0f0\n[*] --> Still\nStill:::mov\nstyle Still fill:#00f");
        let still = g.nodes.iter().find(|n| n.id == "Still").unwrap();
        assert_eq!(still.classes, vec!["mov".to_string()]);
        assert_eq!(still.style.as_deref(), Some("fill:#00f"));
        assert_eq!(g.class_defs.iter().find(|d| d.name == "mov").unwrap().style, "fill:#0f0");
    }
```
  (Use the crate's existing state test `p(...)` helper.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-mermaid state::parse::tests::state_styling_parses`
Expected: FAIL (fields missing; `classDef`/`class` error).

- [ ] **Step 3: Add model fields** — in `crates/mermaid/src/state/mod.rs`:
  - `struct StateNode`: add `pub classes: Vec<String>,` and `pub style: Option<String>,`.
  - `struct StateGraph`: add `pub class_defs: Vec<crate::style::ClassDef>,`.
  - Update the `StateGraph { … }` initializer and every `StateNode { … }` literal (in `state/parse.rs`, e.g. `ensure_node`) to set `classes: vec![], style: None,` and `class_defs: vec![]`.

- [ ] **Step 4: Route styling statements** — in `crates/mermaid/src/state/parse.rs::parse_statement`, replace the `"classDef" | "class" => Err("not supported")` arm with:

```rust
            "classDef" => return self.parse_class_def(stmt),
            "class" => return self.parse_class_assign(stmt),
            "style" => return self.parse_style(stmt),
```
  and add the three methods (mirroring Task 2's `parse_class_def`/`parse_style`, but resolving ids via the state parser's node lookup — use `self.ensure_node(id)` which returns the node index):

```rust
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

    fn parse_class_assign(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("class").unwrap().trim();
        let Some((ids, name)) = rest.rsplit_once(char::is_whitespace) else {
            return Err(self.err("class needs a state list and a class name"));
        };
        let name = name.trim();
        for id in ids.trim().trim_matches('"').split(',') {
            let idx = self.ensure_node(id.trim());
            self.g.nodes[idx].classes.push(name.to_string());
        }
        Ok(())
    }

    fn parse_style(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("style").unwrap().trim();
        let Some((id, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("style needs a state id and styles"));
        };
        let idx = self.ensure_node(id.trim());
        let s = crate::style::sanitize_style(styles);
        if !s.is_empty() {
            self.g.nodes[idx].style = Some(s);
        }
        Ok(())
    }
```
  (If `ensure_node` has a different name/signature in `state/parse.rs`, use whatever the existing node-creation helper is; it is the function `parse_transition` already calls to create/find a node by id.)

- [ ] **Step 5: Handle `:::` on a state id** — the state parser already routes `:::` targets to a loud error (see `parse_statement` comment). Find where a transition endpoint or a bare state id with `:::` is parsed and, instead of erroring, split `id:::className`, call `ensure_node(id)`, and push the class. Add near the top of `parse_statement` (after the `--` guard):

```rust
        // `State:::className` (standalone) attaches a style class.
        if let Some((id, cls)) = stmt.split_once(":::") {
            let id = id.trim();
            let cls = cls.trim();
            if !id.is_empty() && !cls.is_empty() && !cls.contains(char::is_whitespace)
                && !id.contains(char::is_whitespace)
            {
                let idx = self.ensure_node(id);
                self.g.nodes[idx].classes.push(cls.to_string());
                return Ok(());
            }
        }
```
  For a transition target `A --> B:::cls`, in the transition parser split the target on `:::` before `ensure_node`, and push the class onto the resulting node (replace any existing loud-error path for `:::` targets).

- [ ] **Step 6: Run parse test**

Run: `cargo test -p ogrenotes-mermaid state::parse::tests::state_styling_parses`
Expected: PASS.

- [ ] **Step 7: Write the failing SVG test** — in `crates/mermaid/src/state/svg.rs` tests:

```rust
    #[test]
    fn state_style_applied() {
        let svg = crate::render("stateDiagram-v2\nclassDef mov fill:#0f0\n[*] --> S\nS:::mov").svg.unwrap();
        assert!(svg.contains("fill:#0f0;color:#000"), "styled state: {svg}");
        let plain = crate::render("stateDiagram-v2\n[*] --> S").svg.unwrap();
        assert!(!plain.contains("<g style="), "unstyled must not wrap: {plain}");
    }
```

- [ ] **Step 8: Run to verify it fails**

Run: `cargo test -p ogrenotes-mermaid state::svg::tests::state_style_applied`
Expected: FAIL.

- [ ] **Step 9: Apply style in the state SVG emitter** — in `crates/mermaid/src/state/svg.rs`, where a normal state node's `<rect>` is emitted (the rounded box for a simple state), wrap and add the style, mirroring Task 2 Step 9:

```rust
        let resolved = crate::style::resolve(&n.classes, n.style.as_deref(), &g.class_defs);
        let open = match &resolved {
            Some(s) => format!(r#"<g style="{}">"#, escape_xml(s)),
            None => "<g>".to_string(),
        };
        out.push_str(&open);
        let box_style = match &resolved {
            Some(s) => format!(r#" style="{}""#, escape_xml(s)),
            None => String::new(),
        };
        // ...append box_style inside the state <rect ...{box_style}/> ...
        // ...emit the state's label text as before...
        out.push_str("</g>");
```
  Insert `{box_style}` into the existing state-box `<rect …/>` format string (just before the closing `/>`), and wrap that node's rect+label between the `<g …>` and `</g>`.

- [ ] **Step 10: Run state suite**

Run: `cargo test -p ogrenotes-mermaid state::`
Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add crates/mermaid/src/state/
git commit -m "state: support classDef/class/:::/style

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: ER-diagram styling

**Files:**
- Modify: `crates/mermaid/src/er/mod.rs` (add `class_defs` to `ErGraph`; `classes`/`style` to `Entity`)
- Modify: `crates/mermaid/src/er/parse.rs` (route `classDef`/`class`/`style`; `:::` on an entity)
- Modify: `crates/mermaid/src/er/svg.rs` (wrap styled entity boxes)
- Test: same files' test modules

**Interfaces:**
- Consumes: `crate::style::{sanitize_style, ClassDef, resolve}`.
- Produces: `Entity.classes: Vec<String>`, `Entity.style: Option<String>`, `ErGraph.class_defs: Vec<crate::style::ClassDef>`.

- [ ] **Step 1: Write the failing test** — in `crates/mermaid/src/er/parse.rs` tests:

```rust
    #[test]
    fn er_styling_parses() {
        let g = parse("erDiagram\nclassDef warm fill:#f80\nCAR\nCAR:::warm\nstyle CAR fill:#333").unwrap();
        let car = g.entities.iter().find(|e| e.id == "CAR").unwrap();
        assert_eq!(car.classes, vec!["warm".to_string()]);
        assert_eq!(car.style.as_deref(), Some("fill:#333"));
        assert_eq!(g.class_defs.iter().find(|d| d.name == "warm").unwrap().style, "fill:#f80");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-mermaid er::parse::tests::er_styling_parses`
Expected: FAIL (fields missing; `classDef` unrecognized).

- [ ] **Step 3: Add model fields** — in `crates/mermaid/src/er/mod.rs`:
  - `struct Entity`: add `pub classes: Vec<String>,` and `pub style: Option<String>,`.
  - `struct ErGraph`: add `pub class_defs: Vec<crate::style::ClassDef>,`.
  - Update the `ErGraph { … }` initializer and every `Entity { … }` literal in `er/parse.rs` (the entity-creation helper) to include `classes: vec![], style: None,` and `class_defs: vec![]`.

- [ ] **Step 4: Route styling statements** — in `crates/mermaid/src/er/parse.rs`, find the top-level statement dispatch (where a line is classified as entity / relationship / attribute-block). Before the existing classification, add keyword handling:

```rust
        let first = line.split_whitespace().next().unwrap_or("");
        match first {
            "classDef" => return self.parse_class_def(line),
            "class" => return self.parse_class_assign(line),
            "style" => return self.parse_style(line),
            _ => {}
        }
```
  and add the three methods (id lookup via the ER parser's entity-ensure helper — the function the relationship parser uses to create/find an entity by id; call it `ensure_entity` below, adjust to the real name):

```rust
    fn parse_class_def(&mut self, line: &str) -> Result<(), ParseError> {
        let rest = line.strip_prefix("classDef").unwrap().trim();
        let Some((name, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("classDef needs a name and styles"));
        };
        self.g.class_defs.push(crate::style::ClassDef {
            name: name.trim().to_string(),
            style: crate::style::sanitize_style(styles),
        });
        Ok(())
    }

    fn parse_class_assign(&mut self, line: &str) -> Result<(), ParseError> {
        let rest = line.strip_prefix("class").unwrap().trim();
        let Some((ids, name)) = rest.rsplit_once(char::is_whitespace) else {
            return Err(self.err("class needs an entity list and a class name"));
        };
        let name = name.trim();
        for id in ids.trim().trim_matches('"').split(',') {
            let idx = self.ensure_entity(id.trim());
            self.g.entities[idx].classes.push(name.to_string());
        }
        Ok(())
    }

    fn parse_style(&mut self, line: &str) -> Result<(), ParseError> {
        let rest = line.strip_prefix("style").unwrap().trim();
        let Some((id, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("style needs an entity id and styles"));
        };
        let idx = self.ensure_entity(id.trim());
        let s = crate::style::sanitize_style(styles);
        if !s.is_empty() {
            self.g.entities[idx].style = Some(s);
        }
        Ok(())
    }
```
  Also handle a standalone `ENTITY:::className` line: add before the keyword match:

```rust
        if let Some((id, cls)) = line.split_once(":::") {
            let id = id.trim();
            let cls = cls.trim();
            if !id.is_empty() && !cls.is_empty() && !cls.contains(char::is_whitespace) {
                let idx = self.ensure_entity(id);
                self.g.entities[idx].classes.push(cls.to_string());
                return Ok(());
            }
        }
```

- [ ] **Step 5: Run parse test**

Run: `cargo test -p ogrenotes-mermaid er::parse::tests::er_styling_parses`
Expected: PASS.

- [ ] **Step 6: Write the failing SVG test** — in `crates/mermaid/src/er/svg.rs` tests:

```rust
    #[test]
    fn er_style_applied() {
        let svg = render_er("erDiagram\nclassDef warm fill:#f80\nCAR\nCAR:::warm").unwrap();
        assert!(svg.contains("fill:#f80;color:#000"), "styled entity: {svg}");
        let plain = render_er("erDiagram\nCAR").unwrap();
        assert!(!plain.contains("<g style="), "unstyled must not wrap: {plain}");
    }
```

- [ ] **Step 7: Run to verify it fails**

Run: `cargo test -p ogrenotes-mermaid er::svg::tests::er_style_applied`
Expected: FAIL.

- [ ] **Step 8: Apply style in the ER SVG emitter** — in `crates/mermaid/src/er/svg.rs`, in the per-entity loop, before emitting the entity box `<rect …fill="var(--mermaid-node-fill…">`, wrap and add the style (mirroring Task 2 Step 9), and close `</g>` after the entity's header/attribute rows are emitted:

```rust
        let resolved = crate::style::resolve(&e.classes, e.style.as_deref(), &g.class_defs);
        match &resolved {
            Some(s) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(s))),
            None => out.push_str("<g>"),
        }
        let box_style = match &resolved {
            Some(s) => format!(r#" style="{}""#, escape_xml(s)),
            None => String::new(),
        };
        // insert {box_style} into the entity box <rect ...{box_style}/>
        // ...emit header + attribute rows as before...
        out.push_str("</g>");
```

- [ ] **Step 9: Run ER suite**

Run: `cargo test -p ogrenotes-mermaid er::`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/mermaid/src/er/
git commit -m "er: support classDef/class/:::/style

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: linkStyle (edges) + end-to-end verification

**Files:**
- Modify: `crates/mermaid/src/{class,state,er}/mod.rs` (add `style: Option<String>` to each edge type: `Relation`, `Transition`, `ErRelation`)
- Modify: `crates/mermaid/src/{class,state,er}/parse.rs` (route `linkStyle`)
- Modify: `crates/mermaid/src/{class,state,er}/svg.rs` (apply edge style to the `<path>`)
- Test: each type's test module + a manual `mermaid_cli` render

**Interfaces:**
- Consumes: `crate::style::sanitize_style`.
- Produces: `style: Option<String>` on `Relation` / `Transition` / `ErRelation`.

- [ ] **Step 1: Write the failing test** (class shown; repeat the analogous test for state and ER) — in `crates/mermaid/src/class/svg.rs` tests:

```rust
    #[test]
    fn linkstyle_colours_edge() {
        let svg = crate::render("classDiagram\nA --> B\nlinkStyle 0 stroke:#f00").svg.unwrap();
        assert!(svg.contains("stroke:#f00"), "edge style: {svg}");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-mermaid class::svg::tests::linkstyle_colours_edge`
Expected: FAIL (`linkStyle` unrecognized).

- [ ] **Step 3: Add edge `style` field + parse `linkStyle`** — for each of class/state/er:
  - Add `pub style: Option<String>,` to the edge struct (`Relation` / `Transition` / `ErRelation`); set `style: None` in every constructor.
  - In each parser dispatch, add:

```rust
            "linkStyle" => return self.parse_link_style(stmt),
```
  and the method (edges addressed by declaration index; `default` = all):

```rust
    fn parse_link_style(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("linkStyle").unwrap().trim();
        let Some((sel, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("linkStyle needs an index and styles"));
        };
        let s = crate::style::sanitize_style(styles);
        if s.is_empty() {
            return Ok(());
        }
        let edges = /* &mut self.g.relations | .transitions */;
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
```
  (Replace the `edges` binding with the concrete field: `&mut self.g.relations` for class/ER, `&mut self.g.transitions` for state.)

- [ ] **Step 4: Apply edge style in each SVG emitter** — where the edge `<path …/>` is emitted, append the style. Class example (in `class/svg.rs`, the relation loop that builds `attrs`):

```rust
        if let Some(style) = &r.style {
            attrs.push_str(&format!(r#" style="{}""#, escape_xml(style)));
        }
```
  Add the analogous `style` push to the state transition `<path>`/`<line>` and the ER relation `<path>` attribute strings.

- [ ] **Step 5: Run the edge tests (all three types)**

Run: `cargo test -p ogrenotes-mermaid linkstyle`
Expected: PASS (class + state + er edge tests).

- [ ] **Step 6: Full suite + property tests**

Run: `cargo test -p ogrenotes-mermaid`
Expected: PASS (all tests, no property-test regressions).

- [ ] **Step 7: End-to-end visual check** — render a styled example of each type in light and dark and confirm colors + auto-contrast:

```bash
cargo build -p ogrenotes-mermaid --bin mermaid_cli
printf 'classDiagram\nclassDef hot fill:#f9f\nclass A\nA:::hot\nA --> B : uses\nlinkStyle 0 stroke:#f00\n' > /tmp/s_class.mmd
./target/debug/mermaid_cli /tmp/s_class.mmd -t dark -o /tmp/s_class_dark.png
./target/debug/mermaid_cli /tmp/s_class.mmd -t light -o /tmp/s_class_light.png
```
Expected: node A filled `#f9f` with black (auto-contrast) text in both themes; the A→B edge stroked red. Repeat for a `stateDiagram-v2` and an `erDiagram` styled source.

- [ ] **Step 8: Commit**

```bash
git add crates/mermaid/src/class/ crates/mermaid/src/state/ crates/mermaid/src/er/
git commit -m "class/state/er: support linkStyle edge styling

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- Shared `style.rs` (allowlist, `sanitize_style`, `ClassDef`, `resolve`, auto-contrast) → Task 1. ✅
- Flowchart refactor + auto-contrast + test updates → Task 1 Steps 6–7. ✅
- Class/State/ER node styling (`classDef`/`class`/`cssClass`/`:::`/`style`) → Tasks 2/3/4. ✅
- `linkStyle` edge styling for all three → Task 5. ✅
- Verbatim colors + auto-contrast text; applied to flowchart too → `style::resolve` (Task 1), used everywhere. ✅
- Injection boundary unchanged, single point → `style::sanitize_style` (Task 1). ✅
- Unstyled diagrams unchanged → `<g>` (no style) path + explicit "must not wrap" tests in Tasks 2/3/4. ✅
- Deliberate test changes disclosed (flowchart style strings; class/state rejection tests) → Task 1 Step 7; Tasks 2/3 remove the rejection arms. ✅

**Placeholder scan:** The only intentionally-parameterized bits are the concrete field/helper names that differ per type (`ensure_node`/`ensure_entity`, `relations`/`transitions`), called out explicitly with "adjust to the real name" — resolve them by grepping each parser for its existing node/entity-creation helper before implementing. No `TODO`/`TBD`.

**Type consistency:** `ClassDef { name, style }`, `resolve(&[String], Option<&str>, &[ClassDef]) -> Option<String>`, and the per-graph `class_defs: Vec<crate::style::ClassDef>` / per-node `classes: Vec<String>` + `style: Option<String>` names are used identically across Tasks 1–5.
