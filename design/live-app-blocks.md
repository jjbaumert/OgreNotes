# Live-App Blocks — internal plugin interface for native structured blocks

## Overview

**Live-App blocks** are native CRDT-persisted structured blocks a user
drops into a document from the `/` slash menu or block-menu — Calendar,
Kanban, Mermaid, and future widgets in that family. Each block
owns its own schema, renderer, and edit affordances but is compiled
into the OgreNotes binary rather than loaded from an external package.

This doc describes the **internal plugin interface** that lets us add
a new live-app block by writing one module on the backend and one on
the frontend, plus a single registration line in each. It is delivered
alongside #136 (Calendar) as the pilot.

### Boundary — what this is NOT

- **Not iframe embeds.** Those are `NodeType::Embed` (Phase 5 M-P6),
  which host external third-party content behind a per-provider URL
  allowlist.
- **Not an external Live Apps SDK.** External `.ele` packaging,
  developer console, iframe↔host bridge, marketplace, Auth / Proxy
  APIs — all explicitly deferred to v2 per Phase 5 M-P6 scope-down.
  This interface is
  Rust-compile-time only; every block ships in the main binary.
- **Not a runtime-loadable plugin system.** No dynamic loading, no
  WASM sandbox for third-party code, no per-workspace enable/disable
  toggle in v1. Once we have real demand for third-party
  authorship, the traits described below become the seam a runtime
  loader plugs into — but that work is out of scope.

## Architecture

The existing editor schema is **exhaustively matched** over an 18-
variant `NodeType` enum in
[`crates/collab/src/schema.rs`](../crates/collab/src/schema.rs) and
mirrored in `frontend/src/editor/{model.rs, schema.rs}`. A CI
duality test (`cross_schema_*` functions in that same file) enforces
the two schemas match variant-for-variant. Adding a NodeType is a
compile-error cascade of ~10 files.

The plugin interface **preserves that cascade** — schema drift stays
loud — but absorbs the SEMANTIC logic (rendering, HTML/markdown
export, attribute validation) behind traits so each new block is one
new module implementing the trait + one line in a registry const.

### Traits and registries

**Backend — `crates/collab/src/blocks/`:**

```rust
pub trait LiveAppBlock: Sync + 'static {
    /// Every NodeType this plugin owns. Calendar owns both
    /// `Calendar` and `CalendarEvent`.
    fn node_types(&self) -> &'static [NodeType];

    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError>;
}

pub const BLOCKS: &[&(dyn LiveAppBlock + 'static)] =
    &[&calendar::CALENDAR, &kanban::KANBAN, &mermaid::MERMAID];
```

Live-app export (HTML/markdown) is handled inline in
`export.rs`/`markdown.rs` per NodeType — the trait deliberately omits
export methods (heavy render logic stays inline; see the `blocks/mod.rs`
module doc).

**Frontend — `frontend/src/editor/blocks/`:**

```rust
pub trait LiveAppBlockView: Sync + 'static {
    fn node_types(&self) -> &'static [NodeType];
    fn render(&self, node: &Node, ctx: &RenderCtx)
        -> Result<web_sys::Element, JsValue>;
}

pub trait LiveAppBlockInsert {
    fn id(&self) -> &'static str;               // e.g. "calendar"
    fn label_key(&self) -> &'static str;        // fluent key
    fn description_key(&self) -> &'static str;
    fn icon(&self) -> &'static str;             // 20x20 SVG or emoji
    fn insert(&self, editor: &mut Editor, ctx: &InsertCtx)
        -> Result<(), JsValue>;
}

pub const BLOCK_VIEWS: &[&(dyn LiveAppBlockView + 'static)] =
    &[&calendar::CalendarView, &kanban::KanbanView, &mermaid::MermaidView];
pub const BLOCK_INSERTS: &[&(dyn LiveAppBlockInsert + 'static)] =
    &[&calendar::CalendarInsert, &kanban::KanbanInsert, &mermaid::MermaidInsert];
```

`view.rs::render_node` gets a fallback arm: after the existing hand-
written Embed / Image / etc. arms, look up `BLOCK_VIEWS` and delegate
to the trait's `render`.

### Storage model

Live-app blocks reuse the existing CRDT primitives — nothing new in
yrs. Two patterns:

1. **Container + children.** Complex blocks that hold a variable-
   length collection (Calendar's events, Kanban's cards) use one
   NodeType as the container and a second NodeType as the child.
   Children live in the yrs XmlFragment normally; add/remove/move
   converges via yrs XmlFragment semantics unchanged. Per-child
   props ride as yrs XmlElement attributes on the child node.
2. **Attribute bag on the container.** Block-level state (Calendar's
   view mode, cursor, timezone; Kanban's column list) rides as
   yrs XmlElement attributes on the container node. Each attribute
   name LWW-converges independently.

Concurrent edits to different attribute names on the same node don't
conflict — a user in Berlin dragging an event to a new day and a
user in Tokyo changing its color both land. Concurrent edits to the
SAME attribute name resolve LWW; the last write wins. This matches
Embed's behavior today.

All attribute values are strings. Callers parse (dates, integers,
enums) at render time. Attribute validation happens on
import/paste via `LiveAppBlock::validate_attrs` — the interactive
write path trusts the client because the client is under our control.

### Insert surfaces

Three surfaces read from the same `BLOCK_INSERTS` registry so a new
widget appears in all three at once:

- **`/` slash-command menu.** New for this work. Typing `/` at the
  start of a paragraph shows a filterable menu; entries are
  paragraph/heading/list/quote (the existing block-menu entries)
  plus every registered `LiveAppBlockInsert`.
- **Block-menu (hover).** Extended to include live-app entries
  alongside existing paragraph-level entries.
- **Command palette (⌘K).**
  `commands/mod.rs::palette_commands()` iterates `BLOCK_INSERTS` and
  auto-registers each as an editor-scope command using the
  existing `editor_cmd()` helper.

Toolbar buttons are intentionally NOT extended for live-app blocks —
they would clutter the toolbar and the three surfaces above cover the
same intent.

## How to add a new live-app block

1. Add two NodeType variants (container + child) to
   `crates/collab/src/schema.rs` — the enum, `tag_name`,
   `from_tag`, `default_schema`, `is_leaf`, valid-children rules,
   `ALL_NODE_TYPES`.
2. Mirror them in `frontend/src/editor/model.rs`,
   `frontend/src/editor/schema.rs`, and both `node_type_to_tag`
   tables in `frontend/src/editor/{view.rs, yrs_bridge.rs}`.
3. Extend the cross-schema tests in `crates/collab/src/schema.rs`
   (`ALL_NODE_TYPES` const + count assertion).
4. Create `crates/collab/src/blocks/<name>.rs` implementing
   `LiveAppBlock`; register it in `BLOCKS` in
   `crates/collab/src/blocks/mod.rs`.
5. Create `frontend/src/editor/blocks/<name>/` with a `mod.rs`
   exporting `<Name>View` and `<Name>Insert`; register both in
   `BLOCK_VIEWS` / `BLOCK_INSERTS` in
   `frontend/src/editor/blocks/mod.rs`.
6. Add CSS under a `/* ── <Name> block ── */` section in
   `frontend/style/main.css`.
7. Add fluent strings under `insert-<name>-*` and other feature-
   specific prefixes in `frontend/locales/en-US/main.ftl`.
8. Add a playwright doctor scenario in
   `scripts/frontend-doctor/doctor.js` mirroring the shape of
   `scenarioCalendarBlock`.

That's the entire touch-list. Everything else (yrs sync, CRDT
convergence, insert menu wiring, palette registration) is inherited
from the interface.

## Calendar as the pilot (#136)

The Calendar block ships with the interface. Full spec:

- **NodeTypes:** `Calendar` (container), `CalendarEvent` (leaf atom).
- **Views:** month, week, day. Persisted per-block via the
  container's `view` attr.
- **Timezone-aware.** Container carries `timezone` (IANA name);
  events store timestamps in UTC (RFC 3339 `...Z`) and render in
  the block's timezone. All-day events use `startDate` /
  `endDate` (`YYYY-MM-DD`) instead of `startAt` / `endAt`,
  gated by an `allDay="true"` flag.
- **Event model.** Attrs on `CalendarEvent`:
  `color` (six-hue enum), `allDay` (`"true"`/`"false"`),
  `startAt` + `endAt` (RFC 3339 UTC, when `allDay=false`),
  `startDate` + `endDate` (`YYYY-MM-DD`, when `allDay=true`),
  `content` (short display string).
- **Interactions.** Drag-to-move (snap to day in month, 15-min in
  week/day). Drag bottom-right handle to resize. Click event →
  modal (content, color, all-day toggle, start/end). Empty-day
  click → new event at that day, focused for content entry.
- **Real-time co-editing.** Falls out of yrs XmlElement attribute
  LWW + yrs XmlFragment child ordering. No new plumbing.
- **Export.** HTML export renders the month grid as a `<table>`
  with events as `<span class="calendar-event calendar-event--<color>">`.
  Round-trips import via `CalendarBlock::validate_attrs`. Markdown
  export is lossy: a `[Calendar 2026-07 view=month tz=UTC]`
  placeholder followed by a numbered event list.

### Out of scope for Calendar v1

- External calendar sync (Google Calendar / Outlook / iCal) —
  deferred; can slot in later as an `externalId` attr on
  `CalendarEvent` plus a background worker.
- Recurring events.
- Attendees / RSVP — belongs with a future notifications integration.

## Kanban Board (#137)

The second live-app block; extends the plugin interface from
Calendar's two-NodeType shape to a three-NodeType tree
(container → column → card). Follows the same "one module +
one line in the registry" pattern.

### NodeTypes

- **`Kanban`** — container of columns. Atom + isolating. No
  interesting attributes at v1; a future `wipEnabled: bool`
  toggles per-column WIP enforcement.
- **`KanbanColumn`** — container of cards; child of `Kanban`.
  Attrs:
  - `title` (string, max 60 chars) — the column header.
  - `wipLimit` (integer, optional) — max cards before the
    column shows a "full" state. v1 renders but doesn't
    enforce.
- **`KanbanCard`** — leaf; child of `KanbanColumn`. Attrs:
  - `title` (string, max 120 chars) — the card's headline.
  - `content` (string, optional short description).
  - `color` (six-hue enum reusing the palette Calendar
    established).

### Interactions

- **Add card** — button at the tail of each column opens the
  card modal in Add mode.
- **Edit card** — click card body opens Edit mode.
- **Delete card** — Delete button in the Edit modal.
- **Add column** — button at the trailing edge of the columns
  row.
- **Rename column** — click the column title inline.
- **Delete column** — menu on the column header (guarded when
  the column holds cards).
- **Move card between columns** — drag the card body; drop on
  a target column. Reorder within a column via drop position.

### Storage

Same container + child pattern Calendar uses. Add / edit /
delete via targeted `Step::Replace` + `Step::SetAttr` — no
whole-subtree rewrites, so concurrent adds by peers survive.

### Out of scope for Kanban v1

- Assignee / avatar chip on cards — pending user-mention
  integration.
- Due date on cards — pending Calendar↔Kanban cross-linking.
- WIP enforcement — the `wipLimit` attr is stored but not
  enforced in v1; the UI shows a "full" pill.
- Card labels beyond the six-hue color — a proper multi-tag
  system is v2.
- Column reordering (drag columns) — cards-between-columns is
  the v1 win; column moves ship as a follow-up.
- Cross-board card links.

## Related tickets

- [#136](https://github.com/jjbaumert/OgreNotes/issues/136) — Calendar
  (this doc's pilot).
- [#137](https://github.com/jjbaumert/OgreNotes/issues/137) — Kanban
  Board (follow-up; container = `Kanban`, children = `KanbanColumn`
  → `KanbanCard`).
- [#135](https://github.com/jjbaumert/OgreNotes/issues/135) — Project
  Tracker (follow-up; container = `ProjectTracker`, children =
  `ProjectMilestone` / `ProjectTask`).
- [#134](https://github.com/jjbaumert/OgreNotes/issues/134) —
  Feature-parity audit (source of the three tickets
  above).

## Path to v2 (external SDK)

When runtime-loadable third-party plugins are on the roadmap, the
existing traits become the seam:

- `LiveAppBlock` grows a manifest-loaded variant that reads
  render/validate logic from a `.ele`-shaped bundle instead of a
  compiled `impl`.
- `BLOCKS` becomes a `RwLock<Vec<Box<dyn LiveAppBlock>>>` populated
  at boot from an on-disk plugin dir + workspace admin console
  config.
- A workspace-scoped enable/disable registry appears (natural
  extension of the `embed_allowed_domains` idea).
- An iframe↔host bridge exposes a small subset of yrs to sandboxed
  third-party code.

None of that is in scope for #136 / #135 / #137. The v2 seam matters
because it defines the "if we ever ship an SDK, what stays and what
changes" boundary — and this doc's answer is: the traits stay, the
registry gains a runtime variant, and workspace-scoped enable/disable
gets built.
