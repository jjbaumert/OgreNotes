# OpenDyslexic font bundling + UI-prefs on the auth response

**Date:** 2026-07-12
**Status:** design approved, pre-implementation
**Milestone:** Phase 5 (polish) — accessibility + i18n completion

Two related Phase-5 polish items, sharing one mechanism (`ui_prefs`
delivered on the auth response):

1. **Bundle the OpenDyslexic font** so the dyslexia-friendly toggle
   renders a real dyslexic typeface instead of falling through to a
   substitute face.
2. **Carry the user's `ui_prefs` on the auth response** so the
   frontend applies the stored locale (and theme / accessibility)
   from the first paint — no separate `/users/me` boot fetch, no
   reconcile-and-reload.

Because `dyslexic_font` *is* a field of `UiPrefs`, item 2 also fixes
cross-device application of item 1's toggle: the same auth-response
change delivers the dyslexic-font preference on boot.

---

## Feature 1 — Bundle OpenDyslexic (self-hosted)

### Current state

- The toggle works end-to-end: `AccessibilitySettings`
  (`frontend/src/components/accessibility_settings.rs`) drives
  `data-dyslexic="true"` on `<html>`, which the stylesheet keys off
  at `frontend/style/main.css:5663`:

  ```css
  [data-dyslexic="true"] {
    --font-doc-body: "OpenDyslexic", "Comic Neue", "Comic Sans MS", sans-serif;
  }
  ```

- **No font binaries exist** anywhere in the repo, and there is **no
  `@font-face` declaration**. `"OpenDyslexic"` is named first in the
  stack but nothing loads it, so the browser falls through to
  `Comic Neue` → `Comic Sans MS` → generic `sans-serif`. The comment
  at `main.css:5663` explicitly flags bundling as an outstanding
  follow-up "that needs no code change — it's first in the stack
  already."

### Design

Self-host the font (the only viable option under the app's strict
CSP + offline/PWA posture — no external CDN):

1. **Assets.** Add four WOFF2 files under a new `frontend/static/fonts/`
   directory:
   - `OpenDyslexic-Regular.woff2`
   - `OpenDyslexic-Bold.woff2`
   - `OpenDyslexic-Italic.woff2`
   - `OpenDyslexic-BoldItalic.woff2`

   Plus `frontend/static/fonts/OFL.txt` (the SIL Open Font License
   1.1 text that ships with OpenDyslexic). OpenDyslexic is
   OFL-licensed and freely redistributable; bundling the binaries +
   license satisfies the license's redistribution terms.

2. **Trunk copy.** Add a `data-trunk rel="copy-dir"
   href="static/fonts"` link to `frontend/index.html` so the whole
   `fonts/` directory is copied to `dist/fonts/` verbatim (Trunk does
   not hash `copy-dir` output, so the URLs below are stable).

3. **`@font-face`.** Add a new `frontend/style/fonts.css`, linked via
   `data-trunk rel="css"` in `index.html`, with four faces:

   ```css
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

4. **Comment.** Update the stale note at `main.css:5663` to reflect
   that OpenDyslexic is now bundled (drop the "not bundled yet"
   language). The `[data-dyslexic]` rule itself is unchanged —
   `"OpenDyslexic"` already leads the stack.

### Notes / rationale

- **No Rust or toggle change.** The DOM attribute, the CSS variable,
  the settings component, and the pre-mount stamp are all already in
  place and correct.
- **Zero cost for non-users.** A browser only downloads an
  `@font-face` resource when a rendered element actually matches it.
  Users who never enable the toggle never fetch the WOFF2 files.
- **Not part of the WASM budget.** The fonts are separate,
  independently HTTP-cached assets — they do not count against the
  850 KB gzipped WASM ceiling (M-P9).
- **Glyph coverage.** OpenDyslexic covers Latin + common European
  scripts. Non-Latin document content (e.g. an Arabic document) has
  no OpenDyslexic glyphs, so the browser falls back per-glyph through
  the existing family stack. Acceptable — the toggle targets Latin
  document text.
- **Weights.** All four faces (Regular / Bold / Italic / Bold-Italic)
  so bold and italic runs in document body text render in true
  OpenDyslexic rather than browser-synthesised obliques.

### Implementation dependency

The actual WOFF2 binaries must be obtained (official OpenDyslexic
release). At implementation time, fetch them via `curl`; if outbound
network is unavailable in the environment, request the four WOFF2
files + `OFL.txt` from the user. A valid font binary cannot be
authored by hand.

---

## Feature 2 — `ui_prefs` on the auth response (locale, no reload)

### Current state

- **Cross-device locale sync already partly works**, contrary to the
  "not built" framing. `resolve_locale()`
  (`frontend/src/i18n.rs:142`) walks URL → `localStorage` →
  `navigator.language` → `en-US`; it never consults the server pref.
  The server pref is instead reconciled *after* the fact:
  `main.rs::apply_stored_prefs()` (line 182) and
  `locale_selector.rs` (line 79) both fetch `/users/me` on load and,
  if `uiPrefs.locale` differs, call `i18n::set_locale()` +
  `window.location.reload()`. `set_locale` writes `localStorage`, so
  the reload settles the chain.

- **Consequences of the current approach:**
  - An extra `/users/me` round trip on every authenticated boot
    (`main.rs:160`).
  - A full page reload on first login per device (and after any
    cross-device pref change).
  - Deviates from `design/i18n.md`'s stated precedence, which puts
    the stored pref at tier 2 (above `navigator.language`).
  - **Bug:** if `?locale=` is pinned in the URL and differs from the
    server pref, the `same_locale` guard never settles because
    `resolve_locale()` keeps returning the URL value after each
    reload → **infinite reload loop**.

- **The auth flow already has the data.** `TokenResponse`
  (`crates/api/src/routes/auth.rs:334`) is returned by `dev_login`,
  `refresh`, and `issue_session_response`; each handler already holds
  the full `User` row (refresh loads it at `auth.rs:823`), so
  `user.ui_prefs` is in hand with **zero extra DB reads**.

### Design — "Shape B", carrying the whole `ui_prefs`

Deliver `ui_prefs` on the auth response and apply it on the frontend
boot path *before mount*, so first paint is already correct.

#### Backend

- Add an **additive, optional** field to `TokenResponse`
  (`auth.rs:334`):

  ```rust
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub ui_prefs: Option<UiPrefs>,   // serialized as `uiPrefs`
  ```

  Reuse the existing `UiPrefs` type
  (`crates/storage/src/models/user.rs:124`, already `camelCase`).

- Populate it in all three constructors — `dev_login`
  (`auth.rs:1080`), `refresh` (`auth.rs:849`), and
  `issue_session_response` (`auth.rs:452`) — from the already-loaded
  `User`:

  ```rust
  ui_prefs: user.ui_prefs.clone(),
  ```

- **Wire-shape change — deliberate.** Additive, optional, omitted
  when absent (`skip_serializing_if`), and backward-compatible with
  older clients (they ignore the unknown field). Per project policy
  this is flagged as an intentional wire change, not bundled silently.

#### Frontend

- **Mirror struct.** Add `ui_prefs: Option<UiPrefsDto>` (with
  `#[serde(default)]`) to the client `TokenResponse`
  (`frontend/src/api/client.rs:167`), where `UiPrefsDto` is a slim
  `camelCase` decode of `{ theme, locale, dyslexicFont, reduceMotion }`
  following the existing per-consumer-slim-decode pattern.

- **Thread prefs out of the boot refresh only.** `try_refresh_token`
  (`client.rs:119`) is called from **two** places: boot
  (`try_hydrate_from_cookie`) and mid-session token-expiry recovery
  (`ensure_token`, `client.rs:159`). Prefs must be applied on the
  **boot path only** — a mid-session silent refresh must not re-init
  the locale or theme. Approach: `try_hydrate_from_cookie` returns
  `Option<UiPrefsDto>` (decoded from the refresh response);
  `ensure_token`'s path continues to ignore prefs.

- **Boot reorder (`main.rs`).** Move locale init from its current
  synchronous line-41 position to *inside* the post-refresh window,
  before mount. Single init, no provisional double-init:

  ```
  main():
    install_panic_hook(); debug::init_from_url();
    register_defaults();            // stores label_keys, no translate() — bundle not needed
    load_recent_from_storage();
    apply_cached_prefs() / apply_system_theme();   // pre-mount theme from localStorage cache (flash guard)

    spawn_local(async {
      let prefs = try_hydrate_from_cookie().await;    // refresh resolves HERE (Option<UiPrefsDto>)
      let locale = resolve_locale_with_hint(&prefs);  // URL > server hint > localStorage > navigator > en-US
      i18n::init(&locale);                            // single init: AFTER refresh, BEFORE mount
      apply_prefs_authoritatively(&prefs);            // theme + a11y; then cache_prefs() for next load
      mount_to_body(App);                             // first paint already correct — no reload
      // ... existing boot-skeleton removal, rum::init(), observability flush ...
    });
  ```

  - `resolve_locale_with_hint`: if a `?locale=` URL param is present
    it wins (design tier 1); else the server hint (design tier 2);
    else `localStorage`; else `navigator.language`; else `en-US`.
  - `apply_prefs_authoritatively`: apply explicit theme
    (`apply_explicit_theme`), a11y (`apply_a11y_prefs`), and refresh
    the localStorage cache (`cache_prefs`) so the *next* load's
    synchronous pre-mount stamp is correct. This is what
    `apply_stored_prefs` did, minus the `/users/me` fetch and minus
    the locale reload.

- **Deletions / simplifications.**
  - Delete `apply_stored_prefs()` and its `/users/me` boot fetch
    (`main.rs:159`).
  - Strip the reconcile-and-reload block from `locale_selector.rs`
    (lines 79-93); the selector becomes purely a switcher (its
    `on_change` save-and-reload path stays — that is the intended
    in-session locale-change UX).

#### Invariant (load-bearing)

**Refresh resolves → `i18n::init` → `mount`.** The no-reload
guarantee depends entirely on this order. It holds for free because
`mount_to_body` already sat *after* the awaited
`try_hydrate_from_cookie` (`main.rs:85-86`); init is slotted into that
same post-refresh, pre-mount gap. Verified safe because nothing in the
pre-mount window resolves a translation at call-time —
`register_defaults` stores `label_key` strings and labels are only
translated in the palette *query* path
(`commands/mod.rs:181,193`), which runs post-mount.

#### Outcomes

- Correct locale (+ theme + a11y) from first paint; **no reload** for
  the OAuth/production boot path.
- **No `/users/me` boot round trip** — prefs ride the refresh call the
  boot already makes.
- Precedence matches `design/i18n.md`: URL → stored pref → navigator
  → en-US.
- **The infinite-reload bug is structurally eliminated** — the
  reconcile-and-reload logic that caused it no longer exists.

#### Edge cases / accepted limitations

- **Logged-out boot** (login page; refresh returns 401): `prefs` is
  `None`; resolution falls through to `localStorage`/`navigator`
  exactly as today.
- **Dev-login** (dev-only JSON path): the SPA `navigate("/")` does not
  remount the shell, so a locale that differs from the provisional one
  still needs a reload to re-render already-mounted `t!()` strings —
  mirror the existing switcher's set-locale + reload. Production OAuth
  is clean because `/auth/complete` triggers a full page load whose
  boot refresh carries the hint.
- **Cross-device change to an already-open session**: propagates on
  that device's *next* boot, not live. Unchanged from today; live
  push would require the reactive-render work `design/i18n.md` defers.

---

## Testing

- **Backend integration (concrete regression coverage).** Extend the
  API tests (alongside `crates/api/tests/test_users.rs:399`, which
  already covers the `/users/me` locale round-trip) to assert that
  `/auth/refresh` and `/auth/dev-login` responses include
  `uiPrefs.locale` (and the full `uiPrefs`) for a user who has set a
  preference. This is the durable regression guard for cross-device
  sync, which previously depended only on `/users/me`.

- **frontend-doctor scenarios** (boot behavior isn't unit-testable in
  WASM):
  1. **Locale from auth response, no reload.** Seed a user with a
     stored non-default locale (e.g. `ar`), load fresh with
     `localStorage` cleared → the app paints in the stored locale,
     performs **no page reload**, and makes **no `/users/me`** call
     (assert via the doctor's network capture). Include the pinned
     `?locale=` case to prove no reload loop.
  2. **Dyslexic font loads.** Enable the toggle → assert
     `document.fonts` reports `OpenDyslexic` loaded and
     `getComputedStyle('.editor-content').fontFamily` resolves to
     `OpenDyslexic`; assert the four `/fonts/OpenDyslexic-*.woff2`
     assets return HTTP 200.

- **Immutability.** No existing test is modified — only new backend
  assertions and new doctor scenarios are added. If any existing test
  encodes the old reconcile-and-reload behavior, that surfaces as a
  separate behavior-change finding rather than a silent edit.

## Docs

- `design/i18n.md` §"Locale selection" currently describes the
  reconcile-and-reload path. Update it to describe the
  auth-response-hint path (stored pref delivered on refresh, applied
  pre-mount). Per project policy, `design/` is not edited as a side
  effect — this is a deliberate doc update proposed alongside the
  change, applied in the same PR with the doc diff called out
  explicitly.

## Out of scope

- Reactive (no-reload) in-session locale switching — remains the
  `design/i18n.md` v2 carry-forward.
- Live cross-device pref push to already-open sessions.
- Backend string i18n, additional locales, lazy-loaded bundles.
