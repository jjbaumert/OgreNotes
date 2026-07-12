# OpenDyslexic Font + UI-Prefs-on-Auth-Response Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bundle the OpenDyslexic font so the dyslexia toggle renders a real dyslexic face, and deliver the user's `ui_prefs` on the auth response so the frontend applies stored locale/theme/accessibility from the first paint — dropping the `/users/me` boot fetch and the locale reconcile-and-reload.

**Architecture:** The refresh call the frontend already awaits before mount (`main.rs`) is extended to return `ui_prefs`; locale init moves into that post-refresh, pre-mount window so first paint is correct with no reload. The dyslexic font is self-hosted (WOFF2 + `@font-face`) — the toggle CSS already names `"OpenDyslexic"` first, so no Rust/toggle change is needed there.

**Tech Stack:** Rust (axum backend, Leptos/WASM frontend), Trunk (frontend build), fluent-rs (i18n), serde, DynamoDB. Frontend is **outside** the Cargo workspace.

## Global Constraints

- **Frontend is outside the workspace** — `cd frontend/` before any `cargo`/`trunk` command for it.
- **WASM verification** — changes to `main.rs`, `client.rs`, `i18n.rs`, `locale_selector.rs`, `index.html`, or CSS must be verified with `cd frontend && trunk build` (a native `cargo check` silently skips wasm-only glue). Native `cargo test` is fine for pure-logic unit tests.
- **Tests are immutable** — only ADD tests. If a change would require editing an existing test, stop and surface it as a separate finding.
- **Wire-shape change is deliberate** — adding `ui_prefs` to `TokenResponse` is an intentional, additive, optional, backward-compatible wire change. Keep it `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- **Never `git add -A` / `git add .`** in this repo — stage explicit paths only (an untracked verification area at the root must not be swept in).
- **Do not push** — commits only; the user runs pushes via `! git push`.
- **Font license** — OpenDyslexic is SIL Open Font License 1.1; ship `OFL.txt` alongside the binaries.
- **Do not edit `design/` as a side effect** — the `design/i18n.md` update in Task 7 is a deliberate, called-out doc change in this same PR.
- All commit messages end with:
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`

---

## File-touch map

- `crates/api/src/routes/auth.rs` — add `ui_prefs` to `TokenResponse` + populate at 3 construction sites (Task 1).
- `crates/api/tests/test_auth.rs` — new integration test (Task 1).
- `frontend/src/api/client.rs` — `UiPrefsDto`, mirror field, refresh refactor, `try_hydrate_from_cookie` returns prefs (Task 2).
- `frontend/src/i18n.rs` — `pick_locale` (pure), `resolve_locale_with_hint`, `resolve_locale` delegates; unit + wasm tests (Task 3, Task 7).
- `frontend/src/main.rs` — boot reorder, `apply_boot_prefs`, delete `apply_stored_prefs` + `BootstrapMe`/`BootstrapPrefs` (Task 4).
- `frontend/src/components/locale_selector.rs` — remove reconcile-and-reload + dead structs (Task 5).
- `frontend/static/fonts/*.woff2`, `frontend/static/fonts/OFL.txt`, `frontend/style/fonts.css`, `frontend/index.html`, `frontend/style/main.css` — font bundling (Task 6).
- `design/i18n.md` — doc update (Task 7).

---

## Task 1: Backend — `ui_prefs` on `TokenResponse`

**Files:**
- Modify: `crates/api/src/routes/auth.rs` (struct at 334-351; construction sites at 482, 851, 1082)
- Test: `crates/api/tests/test_auth.rs`

**Interfaces:**
- Produces (wire): `TokenResponse` now serializes an optional `uiPrefs` object with `{ theme?, docTheme?, dyslexicFont?, reduceMotion?, locale? }` (the `UiPrefs` camelCase shape). Task 2 consumes `uiPrefs.locale` etc.

- [ ] **Step 1: Write the failing integration test**

Add to `crates/api/tests/test_auth.rs` (model on the existing `test_refresh_token_rotation`):

```rust
/// The auth response carries the user's stored ui_prefs so the
/// frontend can apply locale/theme/a11y on boot without a separate
/// /users/me fetch. Covers both dev-login and refresh (the boot path).
#[tokio::test]
async fn auth_response_carries_ui_prefs_locale() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("prefs-auth@test.com").await;

    // Store a non-default locale via the prefs endpoint.
    let (status, _) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/prefs",
            Some(&token),
            Some(serde_json::json!({ "locale": "ar" })),
        )
        .await;
    assert_eq!(status, 200);

    // A fresh dev-login for the same user must echo the stored locale.
    let (status, login_json) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/dev-login",
            None,
            Some(serde_json::json!({ "email": "prefs-auth@test.com", "name": "Prefs Auth" })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        login_json["uiPrefs"]["locale"], "ar",
        "dev-login carries the stored locale hint"
    );

    // The refresh response carries it too (boot hydrates via refresh).
    let refresh_token = login_json["refreshToken"].as_str().unwrap();
    let (status, refresh_json) = app
        .json_request(
            Method::POST,
            "/api/v1/auth/refresh",
            None,
            Some(serde_json::json!({ "refreshToken": refresh_token })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        refresh_json["uiPrefs"]["locale"], "ar",
        "refresh carries the stored locale hint"
    );

    app.cleanup().await;
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ogrenotes_api --test test_auth auth_response_carries_ui_prefs_locale -- --nocapture`
Expected: FAIL — `uiPrefs` is `null`/absent, so the `assert_eq!(... "ar")` fails (or the index is Null). (If infra isn't available the test is skipped by `require_infra!`; run it in the environment CI uses for integration tests.)

- [ ] **Step 3: Add the field to `TokenResponse`**

In `crates/api/src/routes/auth.rs`, extend the struct (after `mfa_enrollment_required`, before the closing brace at line 351):

```rust
    /// Phase 5 M-P2: the user's stored UI preferences, delivered on
    /// the auth response so the frontend applies locale/theme/a11y on
    /// boot without a separate /users/me fetch. Additive + optional;
    /// omitted when the user has no stored prefs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_prefs: Option<ogrenotes_storage::models::user::UiPrefs>,
```

- [ ] **Step 4: Populate at all three construction sites**

At `issue_session_response` (the `Json(TokenResponse { ... })` at ~482, where `user` is `&User`), add as the last field:

```rust
            ui_prefs: user.ui_prefs.clone(),
```

At `refresh` (the literal at ~851, where `user` is owned) add the same line as the last field — it compiles because `user.ui_prefs` is a disjoint field from the moved `user_id`/`email`/`name`:

```rust
            ui_prefs: user.ui_prefs.clone(),
```

At `dev_login` (the literal at ~1082, `user` owned) add the same last field:

```rust
            ui_prefs: user.ui_prefs.clone(),
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p ogrenotes_api --test test_auth auth_response_carries_ui_prefs_locale`
Expected: PASS.

- [ ] **Step 6: Confirm no other test regressed and the crate builds**

Run: `cargo test -p ogrenotes_api --test test_auth` and `cargo build -p ogrenotes_api`
Expected: all pass / clean build.

- [ ] **Step 7: Commit**

```bash
git add crates/api/src/routes/auth.rs crates/api/tests/test_auth.rs
git commit -m "feat(auth): carry ui_prefs on the auth response

TokenResponse now serializes the user's stored UiPrefs (locale,
theme, a11y) so the frontend can apply them on boot without a
separate /users/me fetch. Additive, optional wire field.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Frontend client — decode `ui_prefs`, return it from boot hydration

**Files:**
- Modify: `frontend/src/api/client.rs` (mirror `TokenResponse` at 167-187; `try_hydrate_from_cookie` at 107-109; `try_refresh_token` at 119-152)

**Interfaces:**
- Consumes: the wire `uiPrefs` object from Task 1.
- Produces:
  - `pub struct UiPrefsDto { theme: Option<String>, locale: Option<String>, dyslexic_font: Option<bool>, reduce_motion: Option<bool> }` (camelCase decode).
  - `pub async fn try_hydrate_from_cookie() -> Option<UiPrefsDto>` (was `-> bool`).
  - `try_refresh_token() -> bool` unchanged for `ensure_token`.

- [ ] **Step 1: Add the `UiPrefsDto` decode struct**

In `frontend/src/api/client.rs`, near the `TokenResponse` mirror (~167), add:

```rust
/// Slim decode of the server `UiPrefs` blob delivered on the auth
/// response. Mirrors the backend camelCase shape; only the fields the
/// boot path applies. Per the per-consumer-slim-decode pattern.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPrefsDto {
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub dyslexic_font: Option<bool>,
    #[serde(default)]
    pub reduce_motion: Option<bool>,
}
```

- [ ] **Step 2: Add the mirror field to the client `TokenResponse`**

Inside `pub struct TokenResponse` (~167-187), add before the closing brace:

```rust
    /// The user's stored UI prefs (Phase 5 M-P2). `serde(default)` so
    /// older servers that omit the field still decode.
    #[serde(default)]
    pub ui_prefs: Option<UiPrefsDto>,
```

- [ ] **Step 3: Refactor the refresh into a shared inner that returns the body**

Replace `try_refresh_token` (119-152) and `try_hydrate_from_cookie` (107-109) with:

```rust
pub async fn try_hydrate_from_cookie() -> Option<UiPrefsDto> {
    refresh_token_inner().await.and_then(|t| t.ui_prefs)
}

/// POST /auth/refresh via the HttpOnly cookie. Returns the decoded
/// TokenResponse on success (and installs the access token), else None.
/// Shared by boot hydration (which wants the ui_prefs) and mid-session
/// token recovery (which ignores the body).
async fn refresh_token_inner() -> Option<TokenResponse> {
    let resp = Request::post(&format!("{API_BASE}/auth/refresh"))
        .header("Content-Type", "application/json")
        .body("{}")
        .ok()?
        .send()
        .await
        .ok()?;
    if !resp.ok() {
        return None;
    }
    let token_resp: TokenResponse = resp.json().await.ok()?;
    set_auth(AuthState {
        access_token: token_resp.access_token.clone(),
        user_id: token_resp.user_id.clone(),
        email: token_resp.email.clone(),
        name: token_resp.name.clone(),
        expires_at: now_ms() + ACCESS_TOKEN_TTL_MS,
    });
    Some(token_resp)
}

/// Return a valid access token, refreshing proactively if expired.
/// Mid-session path — deliberately ignores ui_prefs (a silent token
/// renewal must not re-apply locale/theme).
async fn try_refresh_token() -> bool {
    refresh_token_inner().await.is_some()
}
```

(Leave `ensure_token` — which calls `try_refresh_token` — unchanged.)

- [ ] **Step 4: Verify the frontend compiles for wasm**

Run: `cd frontend && trunk build`
Expected: builds clean. (`try_hydrate_from_cookie`'s new return type is consumed in Task 4; until then `main.rs` still does `let _ = ...await;`, which stays valid since `Option` is fine to discard.)

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/client.rs
git commit -m "feat(frontend): decode ui_prefs from the auth refresh response

try_hydrate_from_cookie now returns the user's UiPrefsDto; the
mid-session refresh path (ensure_token) still ignores it.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Frontend i18n — locale resolution with a server hint

**Files:**
- Modify: `frontend/src/i18n.rs` (`resolve_locale` at 142-153; private `locale_from_*` helpers already exist)

**Interfaces:**
- Produces:
  - `pub fn resolve_locale_with_hint(server_hint: Option<&str>) -> String` — precedence URL → hint → localStorage → navigator → en-US.
  - `resolve_locale()` unchanged in signature; delegates to the shared pure picker with `None` hint.
  - private `fn pick_locale(url, hint, stored, navigator) -> String` (pure, unit-tested).

- [ ] **Step 1: Write the failing unit test**

Add a test module to `frontend/src/i18n.rs` (native `#[test]`, no web_sys):

```rust
#[cfg(test)]
mod locale_precedence_tests {
    use super::pick_locale;

    fn s(v: &str) -> Option<String> { Some(v.to_string()) }

    #[test]
    fn url_beats_everything() {
        assert_eq!(pick_locale(s("fr"), s("ar"), s("es"), s("de")), "fr");
    }
    #[test]
    fn hint_beats_localstorage_and_navigator() {
        assert_eq!(pick_locale(None, s("ar"), s("es"), s("de")), "ar");
    }
    #[test]
    fn localstorage_beats_navigator() {
        assert_eq!(pick_locale(None, None, s("es"), s("de")), "es");
    }
    #[test]
    fn navigator_is_the_last_real_layer() {
        assert_eq!(pick_locale(None, None, None, s("de")), "de");
    }
    #[test]
    fn empty_strings_are_skipped_and_default_applies() {
        assert_eq!(pick_locale(s(""), None, None, None), "en-US");
        assert_eq!(pick_locale(s(""), s(""), s(""), s("de")), "de");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && cargo test --lib pick_locale`
Expected: FAIL — `pick_locale` is not defined.

- [ ] **Step 3: Implement `pick_locale`, `resolve_locale_with_hint`, and re-point `resolve_locale`**

In `frontend/src/i18n.rs`, replace `resolve_locale` (142-153) with:

```rust
pub fn resolve_locale() -> String {
    pick_locale(
        locale_from_url(),
        None,
        locale_from_localstorage(),
        locale_from_navigator(),
    )
}

/// Like [`resolve_locale`] but folds the server-stored pref (delivered
/// on the auth response) into tier 2 of the precedence chain, matching
/// `design/i18n.md`: URL → stored pref → localStorage → navigator →
/// en-US. Called from `main.rs` once the boot refresh resolves.
pub fn resolve_locale_with_hint(server_hint: Option<&str>) -> String {
    pick_locale(
        locale_from_url(),
        server_hint.map(str::to_string),
        locale_from_localstorage(),
        locale_from_navigator(),
    )
}

/// Pure precedence pick — first non-empty layer wins, else en-US.
/// Split out from the web_sys readers so it is unit-testable natively.
fn pick_locale(
    url: Option<String>,
    hint: Option<String>,
    stored: Option<String>,
    navigator: Option<String>,
) -> String {
    [url, hint, stored, navigator]
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "en-US".to_string())
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && cargo test --lib pick_locale`
Expected: PASS (all five tests).

- [ ] **Step 5: Verify wasm build**

Run: `cd frontend && trunk build`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/i18n.rs
git commit -m "feat(frontend): resolve_locale_with_hint for the server pref

Folds the auth-response locale hint into precedence tier 2
(URL > hint > localStorage > navigator > en-US). Pure pick_locale
extracted and unit-tested.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Frontend boot — apply prefs pre-mount, drop the `/users/me` fetch

**Files:**
- Modify: `frontend/src/main.rs` (remove 41-42; edit the spawn_local body at 84-125; delete `BootstrapMe`/`BootstrapPrefs` at 128-148 and `apply_stored_prefs` at 150-190)

**Interfaces:**
- Consumes: `client::try_hydrate_from_cookie() -> Option<UiPrefsDto>` (Task 2), `i18n::resolve_locale_with_hint` (Task 3), existing `theme::{cache_prefs, apply_explicit_theme, pref_from_str, apply_a11y_prefs}`.

- [ ] **Step 1: Remove the pre-refresh locale init**

Delete lines 41-42 (`let locale = i18n::resolve_locale(); i18n::init(&locale);`) and their preceding comment block (33-40). Locale init moves into the post-refresh window below. (`commands::register_defaults()` at ~52 does not resolve translations at call-time, so it is safe to run with no bundle yet.)

- [ ] **Step 2: Rework the spawn_local to hydrate → init → apply → mount**

Replace the head of the `wasm_bindgen_futures::spawn_local(async { ... })` block — specifically the line `let _ = api::client::try_hydrate_from_cookie().await;` (85) and the later `apply_stored_prefs().await;` call (124) — so the body begins:

```rust
    wasm_bindgen_futures::spawn_local(async {
        // Hydrate auth from the refresh cookie BEFORE mount (route
        // guards need it) and pick up the user's stored ui_prefs in the
        // same round trip — no separate /users/me fetch.
        let prefs = api::client::try_hydrate_from_cookie().await;

        // Locale: the load-bearing order is refresh → init → mount.
        // resolve_locale_with_hint folds the server pref into tier 2, so
        // first paint is already in the right locale — no reload.
        let locale = i18n::resolve_locale_with_hint(
            prefs.as_ref().and_then(|p| p.locale.as_deref()),
        );
        i18n::init(&locale);

        // Theme + accessibility: apply authoritatively and refresh the
        // localStorage cache so the next load's pre-mount stamp is right.
        apply_boot_prefs(prefs.as_ref());

        leptos::mount::mount_to_body(app::App);
        // ... existing boot-skeleton removal, rum::init(),
        //     observability::set_token_getter / init_flush_loop ...
    });
```

Keep everything after `mount_to_body` (boot-skeleton removal, `rum::init()`, observability) exactly as-is, and delete the old `apply_stored_prefs().await;` call and its comment (117-125).

- [ ] **Step 3: Add `apply_boot_prefs`, delete the old bootstrap code**

Delete `struct BootstrapMe` / `struct BootstrapPrefs` (128-148) and the whole `async fn apply_stored_prefs()` (150-190). Add in their place:

```rust
/// Apply the user's stored theme + accessibility prefs to the document
/// at boot (locale is applied via resolve_locale_with_hint + i18n::init
/// before this runs). Also refreshes the localStorage cache so the next
/// load paints correctly pre-mount (#152). No-op when the user has no
/// stored prefs (logged-out boot, or a fresh account).
fn apply_boot_prefs(prefs: Option<&api::client::UiPrefsDto>) {
    let Some(prefs) = prefs else {
        return;
    };
    theme::cache_prefs(prefs.theme.as_deref(), prefs.dyslexic_font, prefs.reduce_motion);
    if let Some(theme_str) = prefs.theme.as_deref() {
        theme::apply_explicit_theme(theme::pref_from_str(theme_str));
    }
    theme::apply_a11y_prefs(prefs.dyslexic_font, prefs.reduce_motion);
}
```

- [ ] **Step 4: Verify wasm build (no dead `api_get`/imports left)**

Run: `cd frontend && trunk build`
Expected: clean build with no warnings about unused `BootstrapMe`/`apply_stored_prefs`/imports. If `api::client::api_get` or `i18n::same_locale` is now unused *and* the compiler warns, leave library items in place (they're used elsewhere) — only remove symbols that were local to the deleted `main.rs` code.

- [ ] **Step 5: Manual smoke — no /users/me on boot**

Run: `cd frontend && trunk serve` (backend running per repo dev docs). In the browser DevTools Network tab, load the app logged-in and confirm there is **no** `GET /api/v1/users/me` fired at boot, and the app renders in the stored locale with no reload.
Expected: no `/users/me` boot request; correct locale on first paint.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/main.rs
git commit -m "refactor(frontend): apply ui_prefs from auth response on boot

Locale init moves after the boot refresh (refresh -> init -> mount),
using the ui_prefs hint; theme/a11y applied pre-mount. Deletes
apply_stored_prefs and its /users/me boot fetch and the locale
reconcile-and-reload.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Frontend — strip the locale reconcile-and-reload from the switcher

**Files:**
- Modify: `frontend/src/components/locale_selector.rs` (remove the on-mount `spawn_local` at 79-93 and the now-dead `UserMeLocale`/`UiPrefsRead` at 34-45)

**Interfaces:**
- Consumes: nothing new. `on_change` still uses `client::api_put`, `i18n::set_locale`, `i18n::same_locale`.

- [ ] **Step 1: Remove the reconcile-and-reload block**

Delete the entire `leptos::task::spawn_local(async move { ... });` block (lines 79-93) — boot now owns first-login locale sync, so this is redundant (and was a second `?locale=` reload-loop vector).

- [ ] **Step 2: Remove the dead decode structs**

Delete `struct UserMeLocale` and `struct UiPrefsRead` (34-45). If `use serde::Deserialize;` is now unused, remove it. Keep `use crate::api::client;` (used by `on_change`'s `api_put`), `use crate::i18n;`, and `use wasm_bindgen::JsCast;`.

- [ ] **Step 3: Verify wasm build**

Run: `cd frontend && trunk build`
Expected: clean, no unused-import/struct warnings from this file.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/locale_selector.rs
git commit -m "refactor(frontend): drop locale reconcile-and-reload from switcher

Boot now applies the stored locale from the auth response, so the
switcher's on-mount /users/me fetch + reload is redundant. Removes
the second ?locale= reload-loop vector.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Bundle the OpenDyslexic font

**Files:**
- Create: `frontend/static/fonts/OpenDyslexic-Regular.woff2`, `-Bold.woff2`, `-Italic.woff2`, `-BoldItalic.woff2`, `frontend/static/fonts/OFL.txt`
- Create: `frontend/style/fonts.css`
- Modify: `frontend/index.html` (add copy-dir + css links), `frontend/style/main.css` (comment at 5663)

**Interfaces:** none (assets + CSS). No Rust change.

- [ ] **Step 1: Obtain and validate the four WOFF2 binaries**

Download the four OpenDyslexic WOFF2 files (SIL-OFL 1.1) plus the license into `frontend/static/fonts/`. Candidate source — the official project / a pinned release; e.g.:

```bash
cd frontend/static
mkdir -p fonts
# Try an official/pinned source; adjust URLs to the current release.
for f in Regular Bold Italic Bold-Italic; do
  curl -fsSL -o "fonts/OpenDyslexic-${f/-/}.woff2" \
    "https://github.com/antijingoist/open-dyslexic/raw/master/compiled/OpenDyslexic-${f}.woff2" || true
done
curl -fsSL -o fonts/OFL.txt \
  "https://raw.githubusercontent.com/antijingoist/open-dyslexic/master/OFL.txt" || true
# Validate each is a real WOFF2 (magic bytes "wOF2"):
for w in fonts/OpenDyslexic-*.woff2; do
  printf '%s: ' "$w"; head -c4 "$w" | grep -q 'wOF2' && echo OK || echo "NOT WOFF2"; done
```

If any file is missing or not `wOF2` (source moved, or the release ships `.otf`/`.woff` only), **stop and ask the user** to provide the four `OpenDyslexic-*.woff2` files + `OFL.txt` (or `.otf` files to convert with `woff2_compress`). A valid font binary cannot be authored by hand. Do not proceed until all four validate.

Expected filenames (used by the CSS below): `OpenDyslexic-Regular.woff2`, `OpenDyslexic-Bold.woff2`, `OpenDyslexic-Italic.woff2`, `OpenDyslexic-BoldItalic.woff2`.

- [ ] **Step 2: Create `frontend/style/fonts.css`**

```css
/* OpenDyslexic — dyslexia-friendly document typeface (SIL-OFL 1.1,
 * self-hosted under /fonts). Only fetched when the accessibility
 * toggle sets [data-dyslexic="true"] and matches document text, so
 * users who never enable it pay no bytes. See static/fonts/OFL.txt. */
@font-face {
  font-family: "OpenDyslexic";
  font-style: normal;
  font-weight: 400;
  font-display: swap;
  src: url("/fonts/OpenDyslexic-Regular.woff2") format("woff2");
}
@font-face {
  font-family: "OpenDyslexic";
  font-style: normal;
  font-weight: 700;
  font-display: swap;
  src: url("/fonts/OpenDyslexic-Bold.woff2") format("woff2");
}
@font-face {
  font-family: "OpenDyslexic";
  font-style: italic;
  font-weight: 400;
  font-display: swap;
  src: url("/fonts/OpenDyslexic-Italic.woff2") format("woff2");
}
@font-face {
  font-family: "OpenDyslexic";
  font-style: italic;
  font-weight: 700;
  font-display: swap;
  src: url("/fonts/OpenDyslexic-BoldItalic.woff2") format("woff2");
}
```

- [ ] **Step 3: Wire Trunk copy + CSS link in `index.html`**

Add the fonts CSS link alongside the other `rel="css"` links (after `style/responsive.css`, line 50):

```html
    <link data-trunk rel="css" href="style/fonts.css" />
```

Add a copy-dir for the fonts alongside the other copy links (after line 67):

```html
    <link data-trunk rel="copy-dir" href="static/fonts" />
```

(`copy-dir` copies `static/fonts/` → `dist/fonts/` verbatim — unhashed — so the `/fonts/OpenDyslexic-*.woff2` URLs in `fonts.css` resolve.)

- [ ] **Step 4: Update the stale comment in `main.css`**

Replace the comment at `frontend/style/main.css:5663-5667` with:

```css
/* Dyslexia-friendly typography. OpenDyslexic is bundled (self-hosted
 * WOFF2, @font-face in fonts.css) and leads the stack; the remaining
 * families are fallbacks for the brief font-display: swap window and
 * for glyphs OpenDyslexic doesn't cover (it is Latin-focused). */
```

Leave the `[data-dyslexic="true"] { --font-doc-body: ... }` rule itself unchanged.

- [ ] **Step 5: Build and verify the assets land in dist**

Run: `cd frontend && trunk build && ls dist/fonts/`
Expected: `dist/fonts/` contains the four `OpenDyslexic-*.woff2` files and `OFL.txt`; build is clean.

- [ ] **Step 6: Manual visual check**

Run: `cd frontend && trunk serve`. Enable the dyslexia toggle in `/settings`. In DevTools: (a) the Network tab shows `OpenDyslexic-Regular.woff2` fetched with HTTP 200 when the toggle turns on; (b) `getComputedStyle(document.querySelector('.editor-content')).fontFamily` starts with `"OpenDyslexic"`; (c) document body text visibly changes face.
Expected: all three hold. Toggle off → no OpenDyslexic in computed font.

- [ ] **Step 7: Commit**

```bash
git add frontend/static/fonts frontend/style/fonts.css frontend/index.html frontend/style/main.css
git commit -m "feat(frontend): bundle OpenDyslexic for the dyslexia toggle

Self-hosted WOFF2 (Regular/Bold/Italic/BoldItalic, SIL-OFL 1.1) +
@font-face in fonts.css, copied to dist/fonts via Trunk. The toggle
CSS already led with OpenDyslexic; it now resolves to a real face
instead of a substitute. Fetched only when the toggle is on.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Verification (wasm test) + docs

**Files:**
- Modify: `frontend/src/i18n.rs` (add a `#[wasm_bindgen_test]` for the localStorage path)
- Modify: `design/i18n.md` (§"Locale selection")

**Interfaces:** none.

- [ ] **Step 1: Add a wasm test for the localStorage-backed resolution**

Append to `frontend/src/i18n.rs` (these run under `wasm-tests.yml` via `wasm-pack`; they exercise the real `web_sys` readers that `pick_locale` unit tests can't):

```rust
#[cfg(test)]
mod wasm_locale_tests {
    use wasm_bindgen_test::*;
    wasm_bindgen_test_configure!(run_in_browser);

    fn set_ls(key: &str, val: &str) {
        web_sys::window().unwrap().local_storage().unwrap().unwrap()
            .set_item(key, val).unwrap();
    }
    fn clear_ls(key: &str) {
        web_sys::window().unwrap().local_storage().unwrap().unwrap()
            .remove_item(key).unwrap();
    }

    #[wasm_bindgen_test]
    fn hint_wins_over_stored_localstorage() {
        set_ls("ogrenotes.locale", "es");
        // No ?locale= in the test URL, so URL layer is empty; the hint
        // (server pref) must beat the cached localStorage value.
        assert_eq!(super::resolve_locale_with_hint(Some("ar")), "ar");
        clear_ls("ogrenotes.locale");
    }

    #[wasm_bindgen_test]
    fn falls_back_to_localstorage_without_hint() {
        set_ls("ogrenotes.locale", "de");
        assert_eq!(super::resolve_locale_with_hint(None), "de");
        clear_ls("ogrenotes.locale");
    }
}
```

- [ ] **Step 2: Run the wasm test**

Run: `cd frontend && wasm-pack test --headless --firefox --lib -- wasm_locale_tests`
(Use the browser CI uses; `--chrome` is fine too.)
Expected: both tests PASS. If the local environment has no headless browser, note it and rely on `wasm-tests.yml` in CI.

- [ ] **Step 3: Update `design/i18n.md`**

In §"Locale selection" (around lines 114-127), replace the description of the reconcile-and-reload path with the auth-response-hint behavior. Suggested text:

```markdown
Applied at bootstrap inside `main.rs`, AFTER the boot refresh
resolves: the stored pref rides the `/auth/refresh` response as
`uiPrefs.locale` (no separate `/users/me` fetch), and
`resolve_locale_with_hint` folds it into tier 2 of the precedence
chain. Because init runs after the (already-awaited) refresh and
before mount, first paint is in the correct locale with no reload.
A logged-out boot (refresh 401) has no hint and falls through to
localStorage → navigator → en-US. In-session locale changes via the
switcher still call `set_locale` + reload (the reactive no-reload
re-render remains a v2 carry-forward).
```

- [ ] **Step 4: Commit**

```bash
git add frontend/src/i18n.rs design/i18n.md
git commit -m "test(frontend)+docs: wasm locale-resolution test; i18n.md sync

Adds a wasm_bindgen_test covering hint-vs-localStorage precedence and
updates design/i18n.md to describe the auth-response locale hint path
(deliberate doc update alongside the behavior change).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-review notes (author)

- **Spec coverage:** Font bundling (Task 6) ✓; backend `uiPrefs` on `TokenResponse` (Task 1) ✓; frontend decode + boot reorder + single-init-after-refresh + delete `apply_stored_prefs`/`/users/me` (Tasks 2, 4) ✓; strip reconcile-and-reload / kill infinite-loop vector (Tasks 4, 5) ✓; `resolve_locale_with_hint` precedence match to `design/i18n.md` (Task 3) ✓; backend regression test (Task 1) ✓; wasm behavior test (Task 7) ✓; `design/i18n.md` update (Task 7) ✓.
- **Boot-path-only prefs:** `ensure_token` keeps calling `try_refresh_token() -> bool`; only `try_hydrate_from_cookie` surfaces prefs (Task 2) ✓ — mid-session refresh won't yank locale.
- **Type consistency:** `UiPrefsDto` fields (`theme/locale/dyslexic_font/reduce_motion`) match `apply_boot_prefs` usage and `theme::{cache_prefs,apply_explicit_theme,pref_from_str,apply_a11y_prefs}` signatures ✓. `try_hydrate_from_cookie() -> Option<UiPrefsDto>` consumed consistently in Task 4 ✓.
- **Not covered by an automated test (accepted):** the no-reload / no-`/users/me`-on-boot behavior and the font rendering are verified manually (Task 4 Step 5, Task 6 Step 6); WASM boot ordering and font paint aren't unit-testable. A frontend-doctor scenario is a reasonable follow-up but depends on the deployed-stack DEV_MODE auth caveat.
```
