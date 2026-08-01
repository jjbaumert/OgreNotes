# Presentations (Slide Decks)

**Status:** Design approved 2026-08-01; not yet implemented. Supersedes
the "Presentations (Deferred)" note in `high-level-design.md`.

OgreNotes gains a first-class presentation document kind: slide decks
with freeform-positioned frames of ordinary OgreNotes blocks, real-time
co-editing, a present mode with speaker notes and live
follow-the-presenter, built-in themes, and PDF export.

## Background — the Quip Slides guide

Quip Slides (2018–2021, retired by Salesforce) is the reference model.
Its differentiators were not visual polish but: live charts bound to
spreadsheets, Live Apps embedded in slides, co-editing with built-in
chat and comments placed on slides, audience feedback prompts, and
engagement analytics. Its retirement lesson: the durable value was
slides-as-collaborative-live-documents, not a PowerPoint clone.

OgreNotes already has the underlying machinery Quip built Slides on —
CRDT co-editing, presence, comments, charts, live-app blocks,
templates. This design adds a canvas layout layer, a dedicated deck
view, and a present mode on top of that machinery, following the
spreadsheet's architectural precedent.

## Goals (v1)

- A dedicated deck editor: slides as fixed-aspect canvases of
  positioned, resizable frames; each frame hosts existing OgreNotes
  blocks (rich text, images, tables, code, mermaid, calendar, kanban,
  embeds).
- Real-time co-editing with presence, undo, and offline/reconnect
  semantics identical to documents — inherited, not rebuilt.
- Present mode: full-screen slide rendering, presenter view with
  speaker notes and next-slide preview, and live follow-the-presenter
  for other viewers.
- Frame-anchored comments via the existing comment system.
- Built-in themes and slide layout presets.
- PDF export (one slide per page); degraded HTML/Markdown export.
- A feedback-prompt live-app block (questions with collected
  responses), usable in decks and regular documents.

## Non-goals (deferred, in intended order of revisit)

- Poll block with live vote tallies.
- Engagement analytics (per-viewer, per-slide view tracking).
- `.pptx` import and export.
- Slide transitions and per-element build animations.
- Spatial (x/y-pinned) comments; v1 comments anchor to frames.
- Custom/corporate themes via the admin console; v1 ships built-ins
  only.
- Mobile *editing* of decks (mobile gets view + present).
- A public presenter-session API; live follow rides awareness state
  only.

## Architecture

A deck follows the spreadsheet precedent exactly: the **persisted and
synced form is a normal yrs document** in the shared editor schema, and
the **client keeps a dedicated working model and view**. There is no
new persistence path, no new sync protocol, and no new comment
anchoring scheme.

| Layer | Decks | Precedent |
|---|---|---|
| Storage row | `DocType::Presentation` | `DocType::Spreadsheet` |
| Persisted form | yrs doc, `Doc → Slide → Frame → blocks` | `Doc → Table → TableRow → TableCell` |
| Client model | `frontend/src/presentation/` (`Deck`/`Slide`/`Frame`) | `frontend/src/spreadsheet/` engine |
| View | `DeckView` component | `SpreadsheetView` |
| Bridge | existing `yrs_bridge` diffed sync | same |
| Dispatch | `pages/document.rs` on `doc_type == "presentation"` | `== "spreadsheet"` |

Because a deck is a document row, folders, sharing, link-sharing,
templates (`is_template`), trash, search indexing of frame text,
mentions, notifications, and audit logging (`SecurityAudit` on delete,
share-revoke) apply unchanged.

The Quip importer's thread-type fall-through (`worker_mode.rs`,
unmatched types → `DocType::Document`) is intentionally unchanged:
post-2021 Quip exports cannot contain editable slide decks.

## Data model

### Schema additions

Two new `NodeType` variants in `crates/collab/src/schema.rs`, mirrored
in `frontend/src/editor/schema.rs`, covered by the `cross_schema_*`
duality CI test. This follows the `Calendar`/`CalendarEvent` container
pilot (a known ~10-file compile-error cascade per variant).

**`Slide`** — container.

| Attribute | Meaning |
|---|---|
| `layout` | Preset id the slide was created from (`title`, `title-content`, `two-column`, `blank`); informational + "reapply layout" |
| `background` | Optional theme-relative background override |

**`Frame`** — container.

| Attribute | Meaning |
|---|---|
| `x`, `y`, `w`, `h` | Geometry normalized to 0..1 of the slide; readers clamp out-of-range or unparseable values into 0..1 (never panic) |
| `z` | Stacking order (integer) |
| `role` | `content` (on-canvas) or `notes` (speaker notes; rendered in presenter view, never on the canvas) |

**Deck-level attributes on `Doc`** (present only when
`doc_type == "presentation"`):

| Attribute | Meaning |
|---|---|
| `theme` | Built-in theme id |
| `slideSize` | `16:9` in v1; attr exists so other ratios are not a schema change |

### Validity rules

- In a presentation, `Doc.valid_children = [Slide]`.
- `Slide.valid_children = [Frame]`.
- `Frame.valid_children` = the existing block set (paragraph, heading,
  lists, task lists, blockquote, code block, horizontal rule, image,
  table, mermaid, calendar, kanban, embed). This is what makes
  frames-of-blocks free: every block behavior, input rule, and
  live-app block works inside a frame.
- `Slide` and `Frame` are not valid children anywhere in a regular
  document.

### Identity, comments, convergence

- Slides and frames carry ordinary random blockIds like any editor
  block. The spreadsheet's synthetic positional id scheme
  (`ss:{sheet}:c:{r}:{c}`) is *not* replicated — that exists because
  cells are coordinate-addressed; frames are not.
- **Frame-anchored comments are the existing block-anchored comment
  feature with zero anchoring changes.** The comments pane in
  `DeckView` filters threads to the current slide.
- Frame move/resize are attribute writes (per-attribute LWW under
  yrs): concurrent move-by-A + resize-by-B merges sanely. Slide and
  frame reorder, and all text edits, are ordinary yrs tree operations.
- Deck trees are small (tens of slides × a few frames each); the
  dense-tree per-commit costs that motivated the spreadsheet sparse
  representation (#121/#122) do not apply.

## Client model and canvas editor

`frontend/src/presentation/mod.rs` defines the working model:

```
Deck  { theme, slide_size, slides: Vec<Slide> }
Slide { block_id, layout, background, frames: Vec<Frame> }
Frame { block_id, rect: Rect, z, role, content: Node }
```

Built from the shared editor doc on load and on remote change; local
edits write back through the existing diffed `yrs_bridge` sync
(`sync_model_to_ydoc_diffed`), as the spreadsheet does.

`DeckView` (`frontend/src/components/deck_view.rs`):

- **Slide strip** (left): live-render thumbnails via CSS transform
  scale of the same slide component (no rasterization); drag to
  reorder; add / duplicate / delete slide; add-slide opens the layout
  preset picker.
- **Canvas** (center): current slide at fixed aspect. Frames are
  absolutely-positioned elements. Single-click selects a frame
  (drag/resize handles; snap guides against slide edges, centers, and
  other frames' edges). Double-click enters text editing: the frame's
  content becomes an editable region running the existing editor bound
  to that `Frame` subtree — slash menu, input rules, and block
  behaviors included.
- **Comments pane** (right): the existing pane, filtered to the
  current slide's frames.

### Canvas keymap matrix

Per the block-interaction lesson (new block behaviors need the full
key sweep up front, not one keypress at a time):

| Key | Frame selected (not editing) | Editing inside frame |
|---|---|---|
| Enter | Enter text editing | Normal block behavior |
| Escape | Deselect | Exit to frame-selected |
| Delete / Backspace | Delete frame | Normal text behavior; at-empty-frame does *not* delete the frame (explicit frame-delete only) |
| Tab / Shift-Tab | Cycle frame selection by z-then-position | Normal (lists indent, table cells) |
| Arrow keys | Nudge geometry (Shift = larger step) | Caret movement |
| Cmd/Ctrl-D | Duplicate frame | — |
| Paste | New frame from clipboard content, centered | Normal paste into blocks |

With no frame selected, paste on the canvas also creates a new
centered frame from the clipboard content.

Empty states: an empty deck shows a "first slide" layout picker; an
empty slide shows a hint plus the layout's placeholder frames; a slide
with zero frames is valid.

### Presence

Existing awareness renders collaborators. The awareness payload gains
an optional `frame` field (selected frame blockId); a remote user's
selected frame is outlined in their cursor color — the same channel
the spreadsheet uses for cell cursors.

## Present mode and live follow

- **Route:** `/doc/{id}/present` — a full-screen overlay rendering
  slides read-only at viewport size, keyboard/click navigation.
  Available to any viewer with read access.
- **Presenter view:** same route with `?presenter=1` — current slide,
  next-slide preview, the slide's `role="notes"` frames, and an
  elapsed timer. Runs in a second browser window; it needs no
  cross-window channel beyond the deck's own WebSocket.
- **Live follow:** the presenter's client broadcasts
  `presenting: { slide_block_id }` in its awareness state. No backend
  surface is added; the session is ephemeral by construction
  (awareness state vanishes on disconnect, ending the session).
  Viewers in present mode see a "Follow <name>" affordance whenever
  one or more `presenting` states exist (multiple presenters are
  permitted; viewers choose). Following tracks the presenter's slide;
  navigating manually pauses following and shows a "rejoin" pill.

## Feedback-prompt block

A live-app block per `design/live-app-blocks.md`, registered like
Calendar, usable in decks **and** regular documents (independently
shippable value).

- **`FeedbackPrompt`** — container. Attributes: `question` (text),
  `visibility` = `everyone` | `author-only`.
- **`FeedbackResponse`** — leaf atom child. Attributes: author user
  id, response text, created-at. Complies with the block-atom rules
  from day one: `data-atom-size` on the DOM wrapper and both backspace
  and forward-delete atom handlers.
- Viewers with comment permission submit responses; concurrent
  submissions converge as yrs child appends.
- **Known limitation (stated deliberately):** `author-only` visibility
  is enforced at render time. Responses live in the doc, so anyone
  with read access to the document could read them via the API. This
  is presentation-level privacy, not access control — acceptable for
  v1; server-side private responses would require out-of-doc storage
  and are bundled with the deferred engagement work.

Two more `NodeType` variants → the same duality-CI cascade as
`Slide`/`Frame`.

## Theming and layouts

- **Themes:** six built-in themes (background, heading/body fonts from
  the existing font stack, accent palette). The deck's `theme` attr
  selects one; a theme materializes as CSS variables scoped to the
  canvas/present surface, so switching restyles without touching
  content. Theme definitions live in a small `themes.rs` table in
  `crates/collab` (needed for PDF export) mirrored in the frontend,
  guarded by a duality unit test — same philosophy as the schema.
- **Layouts:** client-side presets only (`title`, `title-content`,
  `two-column`, `blank`) that instantiate frames with preset geometry
  and placeholder content on slide creation. Nothing is persisted
  beyond the resulting frames and the `layout` attr.

## Export

- **PDF** (the supported interchange): one slide per page at 16:9 via
  `crates/collab/src/export.rs`, frames placed at their geometry with
  theme styling. Live-app blocks (calendar, kanban, feedback prompt)
  render as static snapshots of current state; embeds render as their
  link-card fallback, matching existing export behavior.
- **HTML / Markdown:** degraded linear export — slides as `<section>`s
  / `## Slide N` headings, frames emitted in z-then-position order —
  so no export path errors on a deck. Fidelity is explicitly not a
  goal for these formats.
- No `.pptx` in either direction in v1.

## Testing

- **Schema duality:** `cross_schema_*` extends for `Slide`, `Frame`,
  `FeedbackPrompt`, `FeedbackResponse`.
- **Round-trip:** deck model → yrs doc → deck model identity;
  geometry clamping (malformed / out-of-range attrs never panic);
  concurrent move+resize and slide-reorder merge tests at the yrs
  level.
- **Boundary sweep** per the canvas keymap matrix above, plus
  empty-deck / empty-slide / zero-frame-slide states and paste at
  frame and canvas scope.
- **Frontend-doctor scenarios** (written before any deploy relies on
  them): create deck → add slide and frames → edit text in a frame;
  two-tab co-edit convergence; present + follow-the-presenter across
  two tabs; frame comment round-trip.
- **Export:** golden PDF and HTML outputs for a fixture deck.

## Security and permissions

- No new authentication, sharing, or destructive-write paths: deck
  lifecycle uses document routes, so existing `SecurityAudit` emission
  (doc-delete, share-revoke) covers decks with no new call sites.
- Present mode and live follow expose nothing beyond existing read
  access; awareness is already scoped to users with doc access.
- The feedback block's `author-only` render-time limitation is
  documented above and revisited with engagement analytics.

## Phasing

- **P1 — Deck foundation:** `DocType::Presentation`; `Slide`/`Frame`
  schema; client model + `DeckView` canvas editor; layout presets;
  themes; frame comments; degraded HTML/MD export.
- **P2 — Presenting:** present mode; presenter view + speaker notes;
  live follow; PDF export; mobile view/present.
- **P3 — Feedback prompt:** the block, in docs and decks. Independent
  of P2; may land in parallel.
- Deferred items are listed under Non-goals with their intended
  revisit order.
