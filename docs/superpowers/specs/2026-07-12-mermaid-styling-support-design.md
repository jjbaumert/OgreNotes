# Mermaid styling support — design

- **Date:** 2026-07-12
- **Status:** approved (design), pending implementation plan
- **Crate:** `crates/mermaid` (`ogrenotes-mermaid`)
- **Related:** class-gaps work (branch `mermaid-class-gaps`); dark-mode theme vars (PR #55); intro/syntax parity audit (PR #56).

## Motivation

Mermaid's styling directives — `classDef`, `class` / `cssClass` / `:::`,
`style`, `linkStyle` — are fully supported in our **flowchart** renderer but
are **rejected with a parse error** in the class, state, and ER renderers. A
diagram that uses any of them fails to render entirely (shows an error card),
even though the styling is cosmetic. This closes that gap by generalizing the
flowchart styling engine to the other node/edge diagram types.

## Decisions

1. **Scope:** Class + State + ER (all remaining node/edge diagram types).
   Flowchart already has styling and is refactored onto the shared module.
2. **Directives:** full parity — `classDef`, `class` / `cssClass` / `:::`,
   `style` (inline per-node), and `linkStyle` (edges).
3. **Dark mode:** user colors are emitted **verbatim** in both themes (matching
   mermaid), **plus auto-contrast** — when a node has a `fill` but no explicit
   text `color`, a legible `#000`/`#fff` text color is derived by luminance.
   Applied to flowchart too, for one consistent behavior.

### Out of scope

- New diagram *types* (journey, quadrant, xychart, requirement, C4, timeline,
  ZenUML, sankey, block, packet, kanban, architecture, radar, treemap) — these
  have no renderer at all and are separate work.
- Theme/`init` directives, `themeCSS`, and CSS beyond the property allowlist.
- Styling properties outside the existing 8-property allowlist.

## Architecture

One shared styling module, used by every node/edge diagram type.

### `crates/mermaid/src/style.rs` (new, shared)

Owns the styling vocabulary and the security boundary — lifted from
`flowchart::parse`/`flowchart::mod` so there is exactly one implementation:

- `STYLE_PROPS` — the allowlist (`fill`, `stroke`, `stroke-width`,
  `stroke-dasharray`, `color`, `font-weight`, `font-style`, `opacity`).
- `sanitize_style(&str) -> String` — the **CSS-injection boundary**. Splits a
  comma-separated `prop:value` list, keeps only allowlisted props whose values
  contain only benign chars (`[A-Za-z0-9] #.,%-` and space), joins survivors as
  `prop:value;…`. Unknown props/values are dropped silently (never errored).
- `ClassDef { name: String, style: String }` — a named, pre-sanitized style set.
- `resolve(classes: &[String], inline: Option<&str>, defs: &[ClassDef]) -> String`
  — merge order: each assigned class's style in assignment order, then the
  node's inline style last (inline wins). Returns a sanitized `prop:value;…`
  string (already injection-safe from its inputs).
- `auto_contrast(fill: &str) -> Option<&'static str>` — parse a `#rgb`/`#rrggbb`
  fill, compute relative luminance, return `"#000"` (light fill) or `"#fff"`
  (dark fill). Returns `None` for non-hex fills.
- `text_color(resolved: &str) -> Option<String>` — if `resolved` has a `fill`
  and no `color`, return the `auto_contrast` result; else `None`.

### Flowchart refactor

`flowchart::parse` and `flowchart::mod` drop their private `STYLE_PROPS`,
`sanitize_style`, and `ClassDef`, importing them from `style`. Flowchart's
node-style emission additionally calls `text_color` (this is the auto-contrast
upgrade). Behavior is otherwise unchanged.

### Per-type integration (Class, State, ER)

Each diagram gains, mirroring flowchart:

- Graph: `class_defs: Vec<ClassDef>`.
- Node: `classes: Vec<String>` and `style: Option<String>`.
- Edge: `style: Option<String>` (for `linkStyle`).
- Parser: route `classDef` / `class` / `cssClass` / `style` / `linkStyle`
  statements and the `:::className` node suffix to shared helpers instead of
  returning "not supported". The `default` classDef auto-applies to unclassed
  nodes, as in flowchart.

## SVG application

Each node is a `<rect>` (plus text; class boxes also have compartment lines).
For a node whose `resolve(...)` is non-empty:

- **Fill/stroke** → add `style="fill:…;stroke:…"` to the node's `<rect>`. An
  inline `style` attribute wins over the presentation `fill=`/`stroke=`
  attributes that carry the theme vars, so the user's colors take effect.
- **Text color** → wrap that node's SVG elements in `<g style="color:…">` using
  the resolved/auto-contrast color. Text is drawn `fill="currentColor"`, so it
  inherits the group's `color` — one wrapper recolors every label (title,
  attributes, methods) without touching each text emission.
- **Edges** (`linkStyle`) → merge the sanitized style into the relation /
  transition `<path>`'s attributes (its `stroke`, `stroke-dasharray`, etc.).

Nodes/edges with no resolved style render exactly as today (no wrapper, no
style attr) — zero change to unstyled diagrams.

## Security

- All user style text passes through `sanitize_style`; the allowlist + benign
  value-char check is the single injection boundary and is unchanged.
- The value-char set excludes `"`/`<`/`>`/`;`/`(`/`)`, so nothing can break out
  of the `style="…"` attribute or inject markup/`url(...)`.
- Auto-contrast colors are literals we choose (`#000`/`#fff`) — trusted.
- Node ids referenced by `class`/`style`/`:::` are validated by each parser's
  existing id validation.

## Testing

Per new type (Class, State, ER):

- `classDef name …` + `A:::name` (and `class A name`) emits the resolved
  `style="…"` on the node rect.
- inline `style A fill:…` overrides class style.
- `linkStyle 0 stroke:…` colors the correct edge.
- auto-contrast: a light fill yields `color:#000`, a dark fill `color:#fff`;
  a non-hex fill leaves `currentColor`.
- injection attempt (e.g. `fill:red;stroke:url(x);<script>`) is sanitized to at
  most the allowlisted `fill:red`.
- unstyled diagram is byte-for-byte unchanged (guard against accidental
  wrappers).

Flowchart: update the existing exact-string style assertions to include the new
auto-contrast `color:…` (disclosed deliberate change); its other behavior is
unchanged.

## Deliberate behavior changes (disclosed)

- **Flowchart style output** now includes an auto-contrast `color:…` when a fill
  is set without an explicit text color. Existing flowchart tests that assert
  exact `style="fill:…"` strings are updated to match.
- **Class / State / ER** previously *errored* on `classDef`/`class`/`cssClass`/
  `style`/`linkStyle`/`:::`; they now render (styled). The class-diagram
  "`:::` not supported" / "`classDef` not supported" errors and their tests are
  replaced with success-path tests.

## Risks

- **Per-type node/edge model churn:** adding `classes`/`style` fields touches
  three parsers, three models, three SVG emitters. Mitigated by the shared
  module keeping each integration small and uniform.
- **linkStyle indexing:** each type must number edges in declaration order
  consistently; ER/class/state already store relations in order, so the index
  is their `Vec` position.
- **Auto-contrast only understands hex fills.** Named colors (`red`) keep
  `currentColor`; acceptable (matches "cosmetic, best-effort").

## Implementation outline (detailed plan follows in writing-plans)

1. Create `style.rs`; move `sanitize_style`/`STYLE_PROPS`/`ClassDef` there;
   add `resolve`, `auto_contrast`, `text_color`. Refactor flowchart onto it +
   apply auto-contrast; update flowchart style tests.
2. Class: model + parser routing + SVG application + tests.
3. State: model + parser routing + SVG application + tests.
4. ER: model + parser routing + SVG application + tests.
5. Full-suite run + a styled example per type through `mermaid_cli` (light+dark).
