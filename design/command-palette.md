# Command Palette

Phase 5 M-P4 design doc. The palette is a fuzzy-find action
dispatcher reachable via Ctrl-K (or Cmd-K on macOS) from
anywhere in the app, plus Ctrl-Shift-P to open directly in
**Action mode**. It is the universal keyboard escape hatch
referenced by `design/accessibility.md` — every interactive
control that doesn't have an obvious key binding should be
reachable through the palette.

## Goals

- **One key gesture from anywhere.** Ctrl-K opens the palette
  on home, editor, spreadsheet, admin pages.
- **Dual mode**, controlled by leading `>` prefix:
  - **Search mode** (default) — full-text search across the
    user's documents.
  - **Action mode** — fuzzy-find a command, run it, palette
    closes.
- **Scope-aware.** Editor commands surface only on the editor
  page; spreadsheet commands only inside a spreadsheet; home
  commands only on the file browser. `Global` commands surface
  everywhere.
- **Familiar commands rank first.** A 20-entry MRU list,
  persisted in localStorage, biases ranking so the third time a
  user reaches for "Insert table" it's the first match.
- **i18n-native.** Labels resolve through Fluent at render
  time; a runtime locale switch doesn't require re-registering
  the registry.

Out of scope for v1:

- Per-user shortcut remapping. Shortcuts are baked in.
- Workflow / macro recording. The palette runs single commands.
- Discoverability hints / first-run tour. Ctrl-K is documented
  in the help footer; users find it.

## Architecture

```
┌─────────────────────┐      ┌──────────────────────┐
│   main.rs           │      │ search_dialog.rs     │
│  register_defaults()│      │ (dual-mode component)│
└──────────┬──────────┘      └──────────┬───────────┘
           │                            │
           ▼                            │
┌──────────────────────────────────────────────────┐
│ frontend/src/commands/mod.rs                     │
│  - REGISTRY: thread_local Vec<PaletteCommand>    │
│  - RECENT:   thread_local Vec<String> (MRU)      │
│  - matching(query, scope) → Vec<CommandView>     │
│  - run(id)  → fires the action's closure         │
└──────────┬───────────────────────────────────────┘
           │ ToolbarCommand events
           ▼
┌──────────────────────────────────────────────────┐
│ frontend/src/commands/editor_bridge.rs           │
│  - thread_local Option<Callback<ToolbarCommand>> │
│  - installed by document.rs on mount             │
│  - cleared on unmount                            │
└──────────────────────────────────────────────────┘
```

Three pieces:

1. **Registry** (`frontend/src/commands/mod.rs`) — a flat
   `thread_local!` `Vec<PaletteCommand>` populated once at boot
   by `register_defaults()`. Idempotent on `id`; a re-register
   replaces.
2. **Search dialog** (`frontend/src/components/search_dialog.rs`)
   — dual-mode UI. The leading-`>` prefix toggles to Action
   mode. Enter runs the first match (Action) or navigates to
   the first result (Search). Escape closes.
3. **Editor bridge** (`frontend/src/commands/editor_bridge.rs`)
   — thread-local `Option<Callback<ToolbarCommand>>` that the
   document page installs on mount. Lets a non-editor context
   (the palette) dispatch editor commands without holding a
   reference to the editor.

## Data types

```rust
pub enum CommandScope {
    Global,         // always visible
    Editor,         // /d/:id (rich-text)
    Spreadsheet,    // /d/:id (doc_type=Spreadsheet)
    Home,           // /
}

pub struct PaletteCommand {
    pub id: &'static str,                    // stable; key for MRU + dedup
    pub scope: CommandScope,
    pub label_key: &'static str,             // Fluent key, not literal
    pub shortcut: Option<&'static str>,      // "Ctrl+B", informational only
    pub action: Box<dyn Fn()>,               // closure fired by run()
}

pub struct CommandView {
    pub id: &'static str,
    pub scope: CommandScope,
    pub label: String,                       // resolved at render time
    pub shortcut: Option<&'static str>,
}
```

The `Vec<CommandView>` snapshot the palette consumes is the
rendering surface; the action closures stay in the registry.
This separation matters because closures aren't Clone — passing
them through Leptos signals would require an Rc/RefCell.

## Ranking

`matching(query, scope)` walks the registry and returns up to
20 `CommandView`s. Three ranking inputs combine into the score:

| Signal | Magnitude | Source |
|---|---|---|
| Fuzzy match | SkimMatcherV2 (subsequence) | the [`fuzzy-matcher`](https://crates.io/crates/fuzzy-matcher) crate |
| Scope bonus | +50 when `cmd.scope == scope && cmd.scope != Global` | hosting page passes its scope |
| MRU bonus | +100 for index 0, decaying −5 per slot, floored at 0 | localStorage round-trip |

The empty-query case is special: MRU-ordered, then registration-
order to fill up to 20. Users opening the palette with no query
see "what you usually do."

## MRU persistence

The recent-commands list is kept in two places:

- `RECENT: RefCell<Vec<String>>` — in-memory, used by every
  `matching()` call.
- localStorage `ogrenotes.recent_commands.v1` — written by
  `run(id)`, read by `load_recent_from_storage()` at boot.

Why localStorage and not server-side: cross-device MRU is not a
user-visible feature in v1, and a server round-trip on every
palette open would add latency to a fast-feel surface. v2 can
mirror the list into `User.ui_prefs` if cross-device MRU
becomes a request.

The `target_arch = "wasm32"` gate on the localStorage code is
intentional: the Rust unit tests run on the host architecture,
where `web_sys::window()` would panic.

## Scopes and the editor bridge

The hosting page passes its `CommandScope` to the palette as a
component prop. The palette then calls `matching(query, scope)`
and gets back a filtered list:

- `/` → `CommandScope::Home`
- `/d/:id/...` → `Editor` if `doc_type == Document`, else
  `Spreadsheet`
- everywhere else → `Global`

Editor commands need to reach back into the editor's mutation
surface (`ToolbarCommand` enum). The naïve approach — pass an
`on_command` callback through to the palette — falls apart
because the palette is rendered as a sibling of the document
page, not a child. Instead:

```rust
// editor_bridge.rs
thread_local! {
    static ROUTER: RefCell<Option<Callback<ToolbarCommand>>> =
        const { RefCell::new(None) };
}

pub fn install(cb: Callback<ToolbarCommand>) { ... }
pub fn dispatch(cmd: ToolbarCommand) { ... }
pub fn clear() { ... }
```

`document.rs` calls `install` on mount and `clear` on unmount.
Editor commands like `editor.bold` look like:

```rust
register(PaletteCommand {
    id: "editor.bold",
    scope: CommandScope::Editor,
    label_key: "palette-cmd-bold",
    shortcut: Some("Ctrl+B"),
    action: Box::new(|| editor_bridge::dispatch(ToolbarCommand::ToggleBold)),
});
```

If the user opens the palette outside the editor and somehow
runs `editor.bold` (the scope filter normally prevents this),
`dispatch` finds `None` and the command is a silent no-op. The
scope filter is the load-bearing guard; the bridge is a fallback.

## Keyboard contract

| Key | Action |
|---|---|
| Ctrl-K / Cmd-K | Open palette in Search mode |
| Ctrl-Shift-P | Open palette in Action mode (pre-fills `>`) |
| `>` (typed first) | Switch to Action mode mid-query |
| Backspace at empty | (no special handling) |
| Esc | Close palette (focus returns via M-P8 focus trap) |
| Enter | Run first action (Action mode) / navigate to first result (Search mode) |
| Tab / Shift-Tab | Cycle focus inside the dialog (M-P8 focus trap) |
| `?` (prefix in Action mode) | Show shortcut-help overlay |

The Enter handler in `search_dialog.rs` (added in piece D)
runs `commands::run(first.id)` for Action mode and falls back
to navigation for Search.

## Command set (v1, ~40 entries)

Registered by `commands::register_defaults()`. Labels are
Fluent keys (`palette-cmd-*`); the table shows the en-US value.

### Global (always visible)

| id | Label | Shortcut |
|---|---|---|
| `global.go-home` | Go to home | — |
| `global.new-document` | New document | — |
| `global.new-spreadsheet` | New spreadsheet | — |
| `global.new-folder` | New folder | — |
| `global.search` | Search documents | Ctrl+K |
| `global.toggle-theme` | Toggle dark mode | — |
| `global.shortcut-help` | Show keyboard shortcuts | ? |

### Editor (rich-text document)

| id | Label | Shortcut |
|---|---|---|
| `editor.bold` | Bold | Ctrl+B |
| `editor.italic` | Italic | Ctrl+I |
| `editor.underline` | Underline | Ctrl+U |
| `editor.strike` | Strikethrough | — |
| `editor.heading-1` | Heading 1 | Ctrl+Alt+1 |
| `editor.heading-2` | Heading 2 | Ctrl+Alt+2 |
| `editor.heading-3` | Heading 3 | Ctrl+Alt+3 |
| `editor.bullet-list` | Bulleted list | Ctrl+Shift+L |
| `editor.numbered-list` | Numbered list | — |
| `editor.task-list` | Checklist | — |
| `editor.code-block` | Code block | — |
| `editor.blockquote` | Blockquote | — |
| `editor.insert-link` | Insert link | Ctrl+K (when text selected) |
| `editor.insert-image` | Insert image | — |
| `editor.insert-embed` | Insert embed | — |
| `editor.insert-table` | Insert table | — |
| `editor.undo` | Undo | Ctrl+Z |
| `editor.redo` | Redo | Ctrl+Y / Ctrl+Shift+Z |
| `editor.share` | Share document | — |
| `editor.export-html` | Export as HTML | — |
| `editor.export-markdown` | Export as Markdown | — |
| `editor.outline-toggle` | Toggle outline | Ctrl+Shift+O |
| `editor.history-open` | View history | — |

### Spreadsheet (sheet-typed document)

| id | Label | Shortcut |
|---|---|---|
| `sheet.format-currency` | Format as currency | — |
| `sheet.format-percent` | Format as percent | — |
| `sheet.format-date` | Format as date | — |
| `sheet.format-clear` | Clear formatting | — |
| `sheet.freeze-rows` | Freeze rows | — |
| `sheet.freeze-columns` | Freeze columns | — |
| `sheet.insert-chart` | Insert chart | — |
| `sheet.insert-pivot` | Insert pivot table | — |

### Home (file browser)

| id | Label | Shortcut |
|---|---|---|
| `home.bulk-export` | Bulk export selected | — |
| `home.empty-trash` | Empty trash | — |

Counts: 7 Global + 23 Editor + 8 Spreadsheet + 2 Home = **40 v1
commands**. The shortcut column is informational — the actual
key handler lives in the relevant component (editor's
`KeyboardEvent` listener, etc.); the palette doesn't intercept
keystrokes other than Ctrl-K / Ctrl-Shift-P.

## i18n

Every label is a Fluent key, not a literal. The label resolves
at render time inside `matching()` via `crate::t!(label_key)`,
so:

- A locale switch propagates without re-registering the
  registry.
- New translations land by adding the key in `frontend/locales/
  <locale>/main.ftl` — no Rust changes.
- The i18n-audit script flags any raw English string in a
  `view!` macro; registry labels go through Fluent.

The MRU storage uses command **ids**, not labels, so the ranking
is locale-stable.

## Testing

- **Unit tests** in `commands/mod.rs::tests` — registry
  idempotency, fuzzy ranking, scope filter, MRU promotion.
- **Doctor scenario `command-palette-actions`** (piece D) —
  end-to-end: types in editor, opens palette via
  Ctrl-Shift-P, filters to "bold", Enter dispatches, asserts
  `<strong>` lands in the editor DOM.
- **i18n audit** — passes (every label routes through `t!`).
- **Axe-core scenario `a11y-audit`** (M-P8 piece D) audits the
  palette-open surface; modal ARIA + focus trap + live region
  on the results list all gate on it.

## Extending the palette

To add a new command:

1. Pick an `id` in the `<area>.<verb>` namespace.
2. Add the label key to `frontend/locales/en-US/main.ftl` under
   the `palette-cmd-*` section. Add Arabic too if the label is
   short.
3. In `commands::register_defaults()`, push the
   `PaletteCommand` with the right `CommandScope`.
4. If the action needs to reach into the editor, route through
   `editor_bridge::dispatch(...)`.
5. If it's a navigation, use
   `window().location().set_href("/...")` inside the action
   closure.
6. Run `cargo check -p ogrenotes-frontend`; no extra wiring.

## v2 carry-forwards

- Per-user shortcut remapping. Today's shortcuts are baked into
  the `Option<&'static str>` field; remapping needs a
  `User.ui_prefs.shortcuts` map and a fallback chain.
- Workflow recording — chain multiple commands.
- Cross-device MRU via `User.ui_prefs.recent_commands`.
- AI-powered query understanding (palette becomes a natural-
  language target). Needs the LLM gateway from a later phase.
- Plugin-registered commands (Live Apps SDK, blocked on
  embed-host v2 work).
