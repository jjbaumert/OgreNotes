# Deck frames render the full editor block set — design

**Status:** approved 2026-08-01. Follow-up to presentations P1
(design/presentations.md); precedes P2.

## Problem

`Frame.valid_children` already admits the full editor block set
(images, mermaid, calendar, kanban, tables, code, quotes, task lists,
dividers, embeds), and the embedded frame editor can insert them — but
the canvas's read-only frame renderer (`render_node_readonly`,
`deck_view.rs`) only draws paragraphs/headings/flat lists and shows a
debug placeholder box (`deck-frame-placeholder` with the enum label)
for everything else. A mermaid diagram or image placed in a frame is
invisible outside the editor.

## Goal

Every block the editor can put in a frame renders with real fidelity
on the slide canvas and thumbnails. No debug placeholder boxes remain.

## Design

### Rendering mechanisms (deck_view.rs)

1. **Delegated DOM blocks** — `Mermaid`, `Calendar`, `Kanban` render
   via the existing live-app block views
   (`crate::editor::blocks::view_for(nt).render(document, nt, attrs,
   content)`), producing the same DOM the document editor shows
   (mermaid SVG included). A small `RawDom` Leptos helper mounts the
   imperatively built `web_sys::Node` into the declarative tree
   (`node_ref` + `Effect`, re-mounting when the source node changes).
2. **Images** — extract the editor's Image arm (`editor/view.rs`
   ~:1402: blob-ref parse → `image_bridge::resolve` async presigned
   URL with `is_safe_url` checks; legacy/external URLs verbatim) into
   a shared `pub(crate) fn build_image_element(attrs) ->
   Option<web_sys::Element>` used by both the editor view and the deck
   renderer (via `RawDom`). One resolution cache serves both paths.
3. **Native markup** — `Table`/`TableRow`/`TableCell`/`TableHeader`
   (plain table markup, cell text), `CodeBlock` (`pre > code`, text
   only — no syntax highlighting on canvas v1), `Blockquote`,
   `TaskList`/`TaskItem` (☐/☑ glyph from the item's `checked` attr +
   text), `HorizontalRule` (`hr`), `HardBreak` (`br`). Bullet/ordered
   lists upgrade from flattened `text_content()` to recursive
   `render_node_readonly` on children so nesting renders. `Embed`
   renders a link-card chip (title if present, else URL) — never an
   iframe on the canvas.

### Interaction rule

Read-only frame content is wrapped with `pointer-events: none` (CSS on
a content wrapper class) so delegated blocks' buttons/click targets
can't intercept frame selection/drag. Blocks are interacted with by
entering the frame editor — same model as text. The frame comment
button stays outside the wrapper and remains clickable.

### Insertion path (verify, fix if broken)

The frame editor already receives toolbar commands and hosts the slash
menu, so Insert → Image/Mermaid/Calendar/Kanban should work today.
Verify live. Known risk: the Mermaid edit modal opens inside a frame
whose `overflow: hidden` could clip it — if the modal is
`position: fixed` it escapes clipping; otherwise `.deck-frame--editing`
gets `overflow: visible` while the editor is open.

### Out of scope

- Syntax highlighting inside canvas code blocks (text-only v1).
- Live iframes for embeds on the canvas.
- Any schema, validator, or export change (the persisted model and
  degraded exports already handle all these node types).

## Verification

- Unit tests for pure helpers (recursive list rendering shape,
  task-item glyph selection, embed chip text choice) where natively
  testable; DOM-producing paths are covered by the scenario.
- New `deck-blocks` frontend-doctor scenario: create deck → edit a
  frame → insert a mermaid via the Insert toolbar → Escape → assert a
  live `svg` inside the frame on the canvas → reload → still present.
  Plus an API-seeded frame containing an `Image` node with an external
  URL, asserting an `<img>` renders in the frame. Wired into
  `playwright.yml` next to `deck-basics`.
- Full frontend suite + wasm check green.

## Process

Single branch, inline TDD implementation (no multi-agent
orchestration), same PR → checks → merge → test-stack deploy flow as
the P1 launch fixes.
