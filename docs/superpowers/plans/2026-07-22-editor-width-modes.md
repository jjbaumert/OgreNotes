# Editor Width Modes (S/M/L) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a three-way editor-width toggle (S/M/L → 800/1080/1600px) in the document header, left of the "Saved" indicator, persisted as a per-user global preference.

**Architecture:** A `WidthMode` enum drives an inline `--content-max-width` CSS variable on the `.editor-with-panels` wrapper (same technique as the existing `--editor-zoom` var); `.editor-content` inherits it. The choice persists via the existing `UiPrefs` JSON-blob prefs pipeline (`PUT /users/me/prefs`) plus a localStorage cache, mirroring the `docTheme` preference. A new `EditorWidthToggle` component renders the segmented control.

**Tech Stack:** Rust — backend Axum (`crates/api`, `crates/storage`), frontend Leptos 0.7 CSR/WASM (`frontend/`, outside the workspace), Fluent i18n (`.ftl` catalogs).

## Global Constraints

- **Frontend is outside the cargo workspace** — always `cd frontend/` before building/testing it.
- **Width values are fixed:** Narrow=800px, Medium=1080px (current baseline), Wide=1600px.
- **Wire values are lowercase:** `"narrow" | "medium" | "wide"`, field name `editorWidth` (camelCase).
- **Default is Medium** whenever the pref is absent or unrecognized.
- **i18n:** every new user-facing string must be added to ALL SIX catalogs: `frontend/locales/{en-US,ar,es,it,fr,de}/main.ftl`. Non-English catalogs get real translations, not English placeholders.
- **Visible glyphs stay literal `S`/`M`/`L`** — only `title`/`aria-label` are internationalized.
- **Do not `git add -A`** in this repo (untracked `verification/` area). Stage exact paths only.
- **`UiPrefs` is stored as a JSON blob** — adding a struct field auto-threads through the storage repo and all response DTOs; no repo/DTO-construction edits are needed on the backend.

---

## File Structure

**Backend (workspace):**
- Modify `crates/storage/src/models/user.rs` — add `EditorWidth` enum + `editor_width` field on `UiPrefs`.
- Modify `crates/api/src/routes/users.rs` — add merge line in `put_ui_prefs`.
- Modify `crates/api/tests/test_users.rs` — round-trip test.

**Frontend (`cd frontend/`):**
- Create `frontend/src/editor_width.rs` — `WidthMode` enum + mapping + localStorage cache + async persist. Owns the pure logic + its unit tests.
- Modify `frontend/src/main.rs` — register `mod editor_width;`, cache `editorWidth` in `apply_boot_prefs`.
- Modify `frontend/src/api/client.rs` — add `editor_width` to `UiPrefsDto`.
- Create `frontend/src/components/editor_width_toggle.rs` — the `EditorWidthToggle` component.
- Modify `frontend/src/components/mod.rs` — register the component module.
- Modify `frontend/style/main.css` — `.editor-width-modes` styles.
- Modify `frontend/src/pages/document.rs` — signal, inline CSS var, place component in header.
- Modify `frontend/locales/{en-US,ar,es,it,fr,de}/main.ftl` — four new strings each.

---

## Task 1: Backend — `EditorWidth` pref field + round-trip test

**Files:**
- Modify: `crates/storage/src/models/user.rs` (`UiPrefs` struct ~line 124-155; `ThemePref` enum ~line 71)
- Modify: `crates/api/src/routes/users.rs` (`put_ui_prefs` merge block ~line 383-389)
- Test: `crates/api/tests/test_users.rs` (after `ui_prefs_round_trip_and_merge` ~line 442)

**Interfaces:**
- Produces: `UiPrefs.editor_width: Option<EditorWidth>` serializing as wire field `editorWidth` with values `"narrow" | "medium" | "wide"`. Consumed by the frontend `UiPrefsDto` in Task 3.

- [ ] **Step 1: Write the failing round-trip test**

Add to `crates/api/tests/test_users.rs` (model it on `ui_prefs_round_trip_and_merge`):

```rust
#[tokio::test]
async fn ui_prefs_editor_width_round_trip() {
    common::require_infra!();
    let app = common::TestApp::spawn().await;
    let token = app.dev_login("width@example.com", "Width User").await;

    // PUT editorWidth = wide
    let resp = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/prefs",
            Some(&token),
            Some(&json!({ "editorWidth": "wide" })),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // GET /users/me and assert it round-tripped
    let me = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    let body: serde_json::Value = common::body_json(me).await;
    assert_eq!(body["uiPrefs"]["editorWidth"], "wide");
}
```

Note: confirm the exact helper names (`dev_login`, `json_request`, `body_json`, `require_infra!`) match the surrounding tests in the file; copy the setup lines from `ui_prefs_round_trip_and_merge` verbatim if any differ.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ogrenotes-api ui_prefs_editor_width_round_trip -- --nocapture`
Expected: FAIL — either a compile error (`editorWidth` unknown / field missing) or the assertion fails because the field is dropped on the wire.

- [ ] **Step 3: Add the `EditorWidth` enum**

In `crates/storage/src/models/user.rs`, next to `ThemePref` (~line 71), add:

```rust
/// Editor content max-width preference. Lowercase on the wire.
/// Absent ⇒ Medium (the 1080px baseline).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum EditorWidth {
    Narrow,
    #[default]
    Medium,
    Wide,
}
```

- [ ] **Step 4: Add the field to `UiPrefs`**

In the same file, inside `struct UiPrefs` (after the `locale` field, ~line 152), add:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_width: Option<EditorWidth>,
```

- [ ] **Step 5: Add the merge line in `put_ui_prefs`**

In `crates/api/src/routes/users.rs`, in the merge block (after `if patch.reduce_motion.is_some() { ... }`), add:

```rust
    if patch.editor_width.is_some() { merged.editor_width = patch.editor_width; }
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p ogrenotes-api ui_prefs_editor_width_round_trip -- --nocapture`
Expected: PASS.

- [ ] **Step 7: Verify nothing else broke**

Run: `cargo build -p ogrenotes-storage -p ogrenotes-api`
Expected: builds clean (the JSON-blob storage and the `TokenResponse`/`UserResponse` clone the whole struct, so no other edits are required).

- [ ] **Step 8: Commit**

```bash
git add crates/storage/src/models/user.rs crates/api/src/routes/users.rs crates/api/tests/test_users.rs
git commit -m "feat(prefs): add editorWidth user preference (backend)"
```

---

## Task 2: Frontend — i18n strings (S/M/L accessible names)

**Files:**
- Modify: `frontend/locales/en-US/main.ftl` (near the `sync-*` block ~line 873)
- Modify: `frontend/locales/{ar,es,it,fr,de}/main.ftl`

**Interfaces:**
- Produces: Fluent keys `editor-width-group`, `editor-width-narrow`, `editor-width-medium`, `editor-width-wide`, consumed by the component in Task 4 via `crate::t!(...)`.

- [ ] **Step 1: Add the keys to `en-US`**

Append to `frontend/locales/en-US/main.ftl` (group them with a comment, near the `sync-*` strings):

```
# Editor width toggle (S/M/L)
editor-width-group = Editor width
editor-width-narrow = Narrow width
editor-width-medium = Medium width
editor-width-wide = Wide width
```

- [ ] **Step 2: Add translated keys to the other five catalogs**

`frontend/locales/es/main.ftl`:
```
# Editor width toggle (S/M/L)
editor-width-group = Ancho del editor
editor-width-narrow = Ancho estrecho
editor-width-medium = Ancho medio
editor-width-wide = Ancho amplio
```

`frontend/locales/it/main.ftl`:
```
# Editor width toggle (S/M/L)
editor-width-group = Larghezza editor
editor-width-narrow = Larghezza stretta
editor-width-medium = Larghezza media
editor-width-wide = Larghezza ampia
```

`frontend/locales/fr/main.ftl`:
```
# Editor width toggle (S/M/L)
editor-width-group = Largeur de l'éditeur
editor-width-narrow = Largeur étroite
editor-width-medium = Largeur moyenne
editor-width-wide = Grande largeur
```

`frontend/locales/de/main.ftl`:
```
# Editor width toggle (S/M/L)
editor-width-group = Editorbreite
editor-width-narrow = Schmale Breite
editor-width-medium = Mittlere Breite
editor-width-wide = Große Breite
```

`frontend/locales/ar/main.ftl`:
```
# Editor width toggle (S/M/L)
editor-width-group = عرض المحرر
editor-width-narrow = عرض ضيق
editor-width-medium = عرض متوسط
editor-width-wide = عرض واسع
```

- [ ] **Step 3: Verify the catalogs still parse**

Run: `cd frontend && cargo check 2>&1 | grep -i "fluent\|ftl\|parse" || echo "no ftl parse errors"`
Expected: `no ftl parse errors` (the `.ftl` files are `include_str!`-compiled; a syntax error would surface at build time).

- [ ] **Step 4: Commit**

```bash
git add frontend/locales/en-US/main.ftl frontend/locales/ar/main.ftl frontend/locales/es/main.ftl frontend/locales/it/main.ftl frontend/locales/fr/main.ftl frontend/locales/de/main.ftl
git commit -m "i18n(editor): add S/M/L width toggle accessible-name strings"
```

---

## Task 3: Frontend — `editor_width` module + DTO field + boot caching

**Files:**
- Create: `frontend/src/editor_width.rs`
- Modify: `frontend/src/main.rs` (add `mod editor_width;` near `mod theme;` ~line 21; extend `apply_boot_prefs` ~line 133)
- Modify: `frontend/src/api/client.rs` (`UiPrefsDto` struct ~line 198)

**Interfaces:**
- Consumes: `crate::api::client::api_put(path, body) -> Result<(), ApiClientError>`; `UiPrefsDto`.
- Produces:
  - `pub enum WidthMode { Narrow, Medium, Wide }` (Default = Medium), with `pub fn as_wire(self) -> &'static str`, `pub fn from_wire(s: &str) -> WidthMode`, `pub fn max_width_px(self) -> u32`.
  - `pub fn cache_editor_width(mode: WidthMode)`, `pub fn read_cached_editor_width() -> Option<WidthMode>`.
  - `pub async fn persist_editor_width(mode: WidthMode) -> Result<(), crate::api::client::ApiClientError>`.
  - `UiPrefsDto.editor_width: Option<String>` (wire `editorWidth`).

- [ ] **Step 1: Write the failing unit tests**

Create `frontend/src/editor_width.rs` with ONLY the enum + tests first:

```rust
//! Editor content-width preference (S/M/L → 800/1080/1600px).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WidthMode {
    Narrow,
    #[default]
    Medium,
    Wide,
}

impl WidthMode {
    /// Lowercase wire token stored in the `editorWidth` pref.
    pub fn as_wire(self) -> &'static str {
        match self {
            WidthMode::Narrow => "narrow",
            WidthMode::Medium => "medium",
            WidthMode::Wide => "wide",
        }
    }

    /// Parse a wire token; anything unrecognized ⇒ Medium.
    pub fn from_wire(s: &str) -> WidthMode {
        match s {
            "narrow" => WidthMode::Narrow,
            "wide" => WidthMode::Wide,
            _ => WidthMode::Medium,
        }
    }

    /// The `--content-max-width` pixel value this mode applies.
    pub fn max_width_px(self) -> u32 {
        match self {
            WidthMode::Narrow => 800,
            WidthMode::Medium => 1080,
            WidthMode::Wide => 1600,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_round_trips() {
        for m in [WidthMode::Narrow, WidthMode::Medium, WidthMode::Wide] {
            assert_eq!(WidthMode::from_wire(m.as_wire()), m);
        }
    }

    #[test]
    fn unknown_wire_defaults_to_medium() {
        assert_eq!(WidthMode::from_wire(""), WidthMode::Medium);
        assert_eq!(WidthMode::from_wire("garbage"), WidthMode::Medium);
        assert_eq!(WidthMode::default(), WidthMode::Medium);
    }

    #[test]
    fn pixel_values_are_fixed() {
        assert_eq!(WidthMode::Narrow.max_width_px(), 800);
        assert_eq!(WidthMode::Medium.max_width_px(), 1080);
        assert_eq!(WidthMode::Wide.max_width_px(), 1600);
    }
}
```

Register the module: in `frontend/src/main.rs`, next to `mod theme;` (~line 21), add:

```rust
mod editor_width;
```

- [ ] **Step 2: Run the tests to verify they pass (logic is already correct)**

Run: `cd frontend && cargo test editor_width`
Expected: 3 tests pass. (These assert the mapping; they pass immediately — the failing-first gate here is the compile/registration, so if `mod editor_width;` is missing the build fails.)

- [ ] **Step 3: Add the localStorage cache + persist functions**

Append to `frontend/src/editor_width.rs` (below the `impl`, above `#[cfg(test)]`):

```rust
const EDITOR_WIDTH_KEY: &str = "ogrenotes.editor_width";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

/// Cache the mode for the next pre-mount read on the document page.
pub fn cache_editor_width(mode: WidthMode) {
    if let Some(ls) = local_storage() {
        let _ = ls.set_item(EDITOR_WIDTH_KEY, mode.as_wire());
    }
}

/// Read the cached mode, if any.
pub fn read_cached_editor_width() -> Option<WidthMode> {
    let ls = local_storage()?;
    let v = ls.get_item(EDITOR_WIDTH_KEY).ok()??;
    Some(WidthMode::from_wire(&v))
}

/// Cache locally and persist to the server prefs blob.
pub async fn persist_editor_width(
    mode: WidthMode,
) -> Result<(), crate::api::client::ApiClientError> {
    cache_editor_width(mode);
    crate::api::client::api_put(
        "/users/me/prefs",
        &serde_json::json!({ "editorWidth": mode.as_wire() }),
    )
    .await
}
```

- [ ] **Step 4: Add the DTO field**

In `frontend/src/api/client.rs`, inside `struct UiPrefsDto` (after `doc_theme`, ~line 210), add:

```rust
    #[serde(default)]
    pub editor_width: Option<String>,
```

- [ ] **Step 5: Cache the served pref at boot**

In `frontend/src/main.rs`, inside `apply_boot_prefs` (after the `doc_theme` block, ~line 147), add:

```rust
    // Editor width (S/M/L): cache so the document page reads it pre-mount.
    // No DOM apply here — the editor isn't mounted at boot.
    if let Some(w) = prefs.editor_width.as_deref() {
        crate::editor_width::cache_editor_width(crate::editor_width::WidthMode::from_wire(w));
    }
```

- [ ] **Step 6: Verify the crate builds and tests pass**

Run: `cd frontend && cargo test editor_width && cargo check`
Expected: tests pass; `cargo check` clean.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/editor_width.rs frontend/src/main.rs frontend/src/api/client.rs
git commit -m "feat(editor): WidthMode module, editorWidth DTO field + boot caching"
```

---

## Task 4: Frontend — `EditorWidthToggle` component + styles

**Files:**
- Create: `frontend/src/components/editor_width_toggle.rs`
- Modify: `frontend/src/components/mod.rs` (add `pub mod editor_width_toggle;`, alphabetical ~line 24)
- Modify: `frontend/style/main.css` (near `.doc-header-actions` styles)

**Interfaces:**
- Consumes: `crate::editor_width::WidthMode` (Task 3); Fluent keys (Task 2); Leptos 0.7 `Callback<T>` (`.run(v)`).
- Produces: `#[component] pub fn EditorWidthToggle(mode: Signal<WidthMode>, on_select: Callback<WidthMode>) -> impl IntoView`. Consumed by `document.rs` in Task 5.

- [ ] **Step 1: Write the component**

Create `frontend/src/components/editor_width_toggle.rs`:

```rust
use leptos::prelude::*;

use crate::editor_width::WidthMode;

/// Segmented S/M/L control for the editor content width.
/// Visible glyphs stay literal; the accessible names are internationalized.
#[component]
pub fn EditorWidthToggle(
    #[prop(into)] mode: Signal<WidthMode>,
    on_select: Callback<WidthMode>,
) -> impl IntoView {
    view! {
        <div
            class="editor-width-modes"
            role="group"
            aria-label=crate::t!("editor-width-group")
        >
            <button
                type="button"
                class="editor-width-mode-btn"
                class:selected=move || mode.get() == WidthMode::Narrow
                title=crate::t!("editor-width-narrow")
                aria-label=crate::t!("editor-width-narrow")
                aria-pressed=move || (mode.get() == WidthMode::Narrow).to_string()
                on:click=move |_| on_select.run(WidthMode::Narrow)
            >
                "S"
            </button>
            <button
                type="button"
                class="editor-width-mode-btn"
                class:selected=move || mode.get() == WidthMode::Medium
                title=crate::t!("editor-width-medium")
                aria-label=crate::t!("editor-width-medium")
                aria-pressed=move || (mode.get() == WidthMode::Medium).to_string()
                on:click=move |_| on_select.run(WidthMode::Medium)
            >
                "M"
            </button>
            <button
                type="button"
                class="editor-width-mode-btn"
                class:selected=move || mode.get() == WidthMode::Wide
                title=crate::t!("editor-width-wide")
                aria-label=crate::t!("editor-width-wide")
                aria-pressed=move || (mode.get() == WidthMode::Wide).to_string()
                on:click=move |_| on_select.run(WidthMode::Wide)
            >
                "L"
            </button>
        </div>
    }
}
```

- [ ] **Step 2: Register the module**

In `frontend/src/components/mod.rs`, add in alphabetical position (after `pub mod editor_gutter;`):

```rust
pub mod editor_width_toggle;
```

- [ ] **Step 3: Add the styles**

Append to `frontend/style/main.css` (near the `.doc-header-actions` rules):

```css
.editor-width-modes {
  display: inline-flex;
  align-items: center;
  gap: 1px;
  border: 1px solid var(--border-color, #d0d0d0);
  border-radius: 6px;
  overflow: hidden;
}

.editor-width-mode-btn {
  min-width: 26px;
  padding: 3px 7px;
  font-size: 12px;
  font-weight: 600;
  line-height: 1;
  border: none;
  background: transparent;
  color: var(--text-secondary, #666);
  cursor: pointer;
}

.editor-width-mode-btn:hover {
  background: var(--hover-bg, rgba(0, 0, 0, 0.05));
}

.editor-width-mode-btn.selected {
  background: var(--accent-color, #3b82f6);
  color: #fff;
}
```

Note: match the exact CSS-variable names used elsewhere in `main.css` (e.g. grep for `--accent-color` / `--border-color` / `--hover-bg`); substitute the project's real token names if these differ. Keep the fallbacks.

- [ ] **Step 4: Verify it builds**

Run: `cd frontend && cargo check`
Expected: clean (the component is not yet used — a dead-code warning on `EditorWidthToggle` is acceptable until Task 5).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/editor_width_toggle.rs frontend/src/components/mod.rs frontend/style/main.css
git commit -m "feat(editor): EditorWidthToggle segmented control + styles"
```

---

## Task 5: Frontend — wire the toggle into the document header

**Files:**
- Modify: `frontend/src/pages/document.rs` — signal declaration (near the `editor_zoom` signal ~line 361); inline CSS var on `.editor-with-panels` (~line 3018); component placement in `.doc-header-actions` (~line 2804).

**Interfaces:**
- Consumes: `EditorWidthToggle` (Task 4), `crate::editor_width::{WidthMode, read_cached_editor_width, persist_editor_width}` (Task 3).

- [ ] **Step 1: Declare the width-mode signal**

In `frontend/src/pages/document.rs`, near the `editor_zoom` signal (`let (editor_zoom, set_editor_zoom) = signal(1.0f64);`, ~line 361), add:

```rust
    let (width_mode, set_width_mode) = signal(
        crate::editor_width::read_cached_editor_width().unwrap_or_default(),
    );
```

- [ ] **Step 2: Apply the width via an inline CSS variable**

Find the `.editor-with-panels` wrapper that already carries `style:--editor-zoom=...` (~line 3018). Add a second `style:` binding to the same element:

```rust
                style:--content-max-width=move || format!("{}px", width_mode.get().max_width_px())
```

(Place it right after the existing `style:--editor-zoom=...` line, inside the same opening `<div class="editor-with-panels" ...>` tag.)

- [ ] **Step 3: Place the toggle left of the Saved indicator**

Find `.doc-header-actions` (~line 2804). Insert `EditorWidthToggle` as the FIRST child, immediately before `<SyncIndicator state=sync_state.into() />`:

```rust
                <crate::components::editor_width_toggle::EditorWidthToggle
                    mode=width_mode
                    on_select=Callback::new(move |m: crate::editor_width::WidthMode| {
                        set_width_mode.set(m);
                        leptos::task::spawn_local(async move {
                            let _ = crate::editor_width::persist_editor_width(m).await;
                        });
                    })
                />
                <SyncIndicator state=sync_state.into() />
```

Notes:
- `mode=width_mode` passes the `ReadSignal`; the component's `#[prop(into)]` converts it to `Signal<WidthMode>`.
- If `spawn_local` is already imported in `document.rs`, use the bare `spawn_local(...)`; otherwise the fully-qualified `leptos::task::spawn_local` above works without an added import. (Grep the file first: `grep -n spawn_local frontend/src/pages/document.rs`.)
- If `Callback` isn't already in scope, it comes from `leptos::prelude::*`, which `document.rs` already imports.

- [ ] **Step 4: Build the frontend natively**

Run: `cd frontend && cargo check`
Expected: clean, no dead-code warning on `EditorWidthToggle` anymore.

- [ ] **Step 5: Build for the real WASM target**

Because this touches editor rendering and runtime code, verify the actual WASM build (native `cargo check` skips wasm-only paths):

Run: `cd frontend && cargo build --target wasm32-unknown-unknown`
Expected: builds clean.

- [ ] **Step 6: Manual verification (visual + persistence)**

Follow the project `verify` skill (local compose stack + trunk dist). Confirm:
1. The S/M/L control renders left of "Saved" in the document header.
2. Clicking S narrows the editor to 800px, M to 1080px, L to 1600px (measure `.editor-content` computed `max-width`).
3. Exactly one button shows the `.selected` style at a time.
4. Reload the page → the last-selected width is restored (localStorage).
5. Open the doc in a fresh browser profile logged in as the same user → the width follows the account (served pref → boot cache).
6. Hovering a button shows the localized tooltip; switching locale changes the tooltip/aria text while glyphs stay S/M/L.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/pages/document.rs
git commit -m "feat(editor): wire S/M/L width toggle into the document header"
```

---

## Self-Review Notes

- **Spec coverage:** widths 800/1080/1600 (Task 1 enum + Task 3 `max_width_px`); toggle left of Saved (Task 5 Step 3); i18n tooltips/aria across 6 catalogs (Task 2); per-user synced global persistence (Task 1 backend + Task 3 persist/boot); component modeled on `share-link-modes` (Task 4); apply via inline `--content-max-width` (Task 5 Step 2); tests (Task 1 round-trip, Task 3 enum units, Task 5 manual). All covered.
- **Backend threading:** JSON-blob storage means the storage repo, `TokenResponse`, and `UserResponse` need no edits — verified in exploration; Task 1 Step 7 confirms the build.
- **Type consistency:** `WidthMode`/`as_wire`/`from_wire`/`max_width_px`/`persist_editor_width`/`cache_editor_width`/`read_cached_editor_width`/`EditorWidthToggle(mode, on_select)` are used identically wherever referenced across Tasks 3–5.
- **Open verification during impl:** confirm test-helper names in `test_users.rs` (Task 1 Step 1) and CSS token names in `main.css` (Task 4 Step 3) against the real files; both are flagged inline.
