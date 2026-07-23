# Editor width modes (S / M / L) — design

**Date:** 2026-07-22
**Area:** frontend (`frontend/`, Leptos/WASM — outside the cargo workspace) + backend prefs endpoint
**Status:** approved design, ready for implementation plan

## Goal

Give the user control over the editor's maximum horizontal content width via a
three-way toggle (Narrow / Medium / Wide), surfaced as `S` / `M` / `L` buttons in
the document header, immediately to the left of the "Saved" sync indicator. The
choice is a per-user, global (all-documents) preference that syncs across devices.

## Widths

Driven off the current baseline CSS variable `--content-max-width: 1080px`
(`frontend/style/variables.css:36`, consumed by `.editor-content` at
`frontend/style/main.css:878`).

| Mode | Button | `--content-max-width` |
|------|--------|-----------------------|
| Narrow | `S` | **800px** |
| Medium | `M` | **1080px** (current baseline) |
| Wide   | `L` | **1600px** |

No viewport cap is applied — small screens are already constrained by the
existing `.editor-content` responsive overrides in `frontend/style/responsive.css`,
which must continue to win on tablet/mobile so Wide mode does not fight them.

## Component

New component `EditorWidthToggle` at
`frontend/src/components/editor_width_toggle.rs`, modeled on the existing
segmented-control pattern in `share_dialog.rs:284` (`share-link-modes`):

- Wrapper `<div class="editor-width-modes" role="group" aria-label=…>`.
- Three `<button class="editor-width-mode-btn" class:selected=…>` children with
  visible glyphs `S`, `M`, `L`.
- Mutually exclusive: exactly one button carries `.selected` at a time, keyed off
  the `width_mode` signal.
- Each button has an i18n'd `title` and `aria-label`; the glyph text itself stays
  the literal letter (letters are near-universal and compact).
- Clicking a button sets the mode (see State & persistence).

New CSS block `.editor-width-modes` / `.editor-width-mode-btn` / `.selected`
(the `share-link-*` classes have no shared stylesheet block to reuse, so this
control gets its own rules alongside the existing header styles).

### Placement

First child of `.doc-header-actions` in `frontend/src/pages/document.rs:2804`,
immediately before `<SyncIndicator state=sync_state.into() />`.

## State & apply

- New enum `WidthMode { Narrow, Medium, Wide }` with helpers:
  - `as_wire(&self) -> &'static str` → `"narrow" | "medium" | "wide"`
  - `from_wire(&str) -> WidthMode` (unknown/empty → `Medium`)
  - `max_width_px(&self) -> u32` → `800 | 1080 | 1600`
- New `width_mode` signal in `document.rs`, initialized from the served/cached
  preference (default `Medium`).
- The signal drives `--content-max-width` inline on the `.editor-with-panels`
  wrapper using `style:--content-max-width=move || format!("{}px", …)` — the same
  inline-CSS-variable technique the pinch-zoom `--editor-zoom` var uses
  (`document.rs:3018`). `.editor-content` is a descendant and inherits the value,
  overriding the `variables.css` default.

Width-apply is deliberately kept local to the document page (not a pre-mount
`<html>` attribute like theme), because `.editor-content` only exists on the doc
page.

## Persistence (per-user, synced, global)

Mirror the `change_doc_theme` three-layer pattern in `frontend/src/theme.rs`:

1. **DTO** — add `editor_width: Option<String>` to `UiPrefsDto`
   (`frontend/src/api/client.rs:198`), `#[serde(default)]`, camelCase `editorWidth`
   on the wire.
2. **On change** — update the `width_mode` signal → write localStorage key
   `ogrenotes.editor_width` → `client::api_put("/users/me/prefs", { "editorWidth": wire })`.
3. **On boot** — `apply_boot_prefs` (`frontend/src/main.rs:133`) reads `editorWidth`
   off the auth-response `UiPrefsDto` (no extra fetch) and seeds the initial mode;
   the localStorage cache is the pre-serve fallback. Unset → `Medium`.

### Backend

The `PUT /users/me/prefs` handler and the user-prefs persistence/storage must
accept and round-trip the new `editorWidth` field, and the field must be included
in the `UiPrefsDto` returned on the auth response. Exact handler/storage locations
to be confirmed during planning. This is an additive, backward-compatible field
(optional, defaulted), not a breaking wire change.

## i18n

Add four keys to **all six** locale catalogs
(`frontend/locales/{en-US,ar,es,it,fr,de}/main.ftl`), following the `sync-*`
example at `en-US/main.ftl:873`:

```
editor-width-group = Editor width
editor-width-narrow = Narrow width
editor-width-medium = Medium width
editor-width-wide = Wide width
```

`editor-width-group` is the group `aria-label`; the three `-narrow/-medium/-wide`
strings are the per-button `title` + `aria-label`. Non-en-US catalogs get
translated values (placeholders equal to English are not acceptable — follow the
existing translated-catalog convention).

## Testing

- Unit test the `WidthMode` mapping: `as_wire` / `from_wire` round-trip and
  `max_width_px` returns `800 / 1080 / 1600`.
- Component test for `EditorWidthToggle`: the correct single button is `.selected`
  for each mode, and clicking a button invokes the mode-change callback with the
  expected mode.
- Verify the preference round-trips: setting a mode persists via `PUT /users/me/prefs`
  and a fresh boot restores it (localStorage fallback + served DTO).

## Out of scope

- Per-document width (explicitly global per user).
- Session-only / device-only persistence (explicitly synced).
- Continuous/arbitrary width slider — three discrete modes only.
- Translating the visible `S`/`M`/`L` glyphs (letters stay literal; only the
  accessible names/tooltips are internationalized).
