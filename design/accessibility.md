# Accessibility

OgreNotes targets **WCAG 2.1 Level AA conformance** for the web
client. This doc records the policy, the patterns that uphold it,
the controls already in place, and the known gaps deferred to
future work. It is intentionally light — the canonical living
proof that the code meets the bar is the axe-core gate in
`playwright.yml` (Phase 5 M-P8 piece D).

This doc is a peer of `design/branding.md` and `design/i18n.md`. It
is read by:

- developers landing UI work (consult the patterns section before
  introducing a new modal, status surface, or interactive widget)
- the M-P8 verification pass (the manual screen-reader checklist
  in `runbook/a11y-screenreader-checklist.md` is keyed to this doc)
- security/compliance review (Section 508 + EAA alignment)

## Scope

WCAG 2.1 AA is the bar. WCAG 2.2 additions and AAA-grade contrast
are aspirational. Mobile-specific accessibility is in scope; the
WebView/PWA path uses the same DOM as desktop, so a single fix
covers both.

Out of scope:

- AT-vendor-specific quirks (NVDA × Firefox vs JAWS × Chrome are
  noted in the runbook but not coded around)
- Forced-colors / Windows High Contrast mode (Phase 6 candidate)
- Print stylesheet (we don't ship one)

## Principles

1. **Semantic HTML first.** A `<button>` is always better than a
   `<div role="button">`; a `<nav>` always better than a styled
   `<div>`. Roles are scaffolding for HTML5 elements we didn't pick.
2. **Focus is observable.** Every interactive element must have a
   visible focus indicator. The `--color-focus-outline` token is
   the policy; never `outline: none` without a replacement.
3. **Keyboard parity.** Every mouse action must have a keyboard
   equivalent. The command palette (Ctrl-K) is the universal
   escape hatch for surfaces that don't expose a key binding
   directly.
4. **Live regions are conservative.** `role="status"` (polite) is
   the default for confirmations and progress; `role="alert"`
   (assertive) is reserved for errors and blocking conditions
   that pre-empt other speech.
5. **Color is never the sole signal.** Status, validity, and
   focus must convey through shape/text/icon as well as color.
6. **No keyboard trap-out.** Tab cycles inside a modal back to
   itself; Escape closes the modal and restores focus.

## Controls in place

These are verified to ship (commit references are durable anchors,
not the audit's source of truth — the source of truth is the
axe-core run wired into CI in M-P8 piece D).

### Focus management

- `frontend/src/a11y/focus_trap.rs` exports
  `install_focus_trap(dialog_ref, visible)` and
  `handle_tab_trap(&event, &dialog_el)`. Wired into 5 modals:
  `ShareDialog`, `ConfirmDialog`, `SearchDialog` (palette),
  `FolderPickerDialog`, `CommentPopup`. Save-on-open, restore-on-
  close, Tab cycle within.
- Escape closes every modal listed above.
- The `<a class="skip-to-content">` link in `app.rs` is the first
  focusable element on the page; hides at `top: -40px`, surfaces
  at `top: 8px` on `:focus`. Anchors `#main-content`.

### Landmarks and labels

- `<main id="main-content" tabindex="-1">` on every top-level
  route: home, document, login, auth-complete, mfa-enroll,
  mfa-challenge, workspace-saml, workspace-scim, admin-users,
  admin-metrics, admin-audit. `tabindex="-1"` is necessary so the
  skip-link's programmatic focus target works (a `<main>` is not
  focusable by default).
- `<nav aria-label="…">` on the Sidebar (`sidebar-aria-main-nav`
  string) and on the home-page breadcrumb trail
  (`a11y-breadcrumb-label`).
- Breadcrumb current item gets `aria-current="page"`.
- Editor toolbar: `role="toolbar"` + `aria-label`. Each
  `.toolbar-group` div gets `role="group"` + an a11y-prefixed
  aria-label.

  > **Caveat — toolbar interaction model.** WAI-ARIA's toolbar
  > pattern recommends arrow-key navigation within a single tab
  > stop. OgreNotes's toolbar still uses one tab stop per button,
  > which is the simpler interaction model. The `role="toolbar"`
  > announcement is informational; replace with `role="group"`
  > on the outer `<div>` if SR users report that the arrow-key
  > expectation is confusing.

- File browser: `<table aria-label="Documents and folders">`. The
  header row uses real `<th>` cells; the empty-checkbox `<th>`
  gets its own aria-label.

### Live regions

- `SyncIndicator` — `role="status" aria-live="polite"
  aria-atomic="true"` on the sync-state pill in the document
  header.
- `ShareDialog` status row — `role="status"` (polite) on success,
  `role="alert"` (assertive) on error.
- `workspace_saml.rs` status messages — `role="status"` (polite).
- Auth + form error messages — `role="alert"` on every error
  banner in home, document, login, mfa-enroll, mfa-challenge.

### Modal ARIA

Every modal carries:

- `role="dialog"`
- `aria-modal="true"`
- Either `aria-labelledby` pointing at the title `<h3>` (or
  equivalent) or `aria-label` when the title is implicit.

### Color contrast (light + dark token audit)

Audited 2026-05-21 against WCAG 2.1 AA (4.5:1 body / 3:1 large +
non-text). All adjustments below ship with this piece; see token
diff in `frontend/style/tokens-{light,dark}.css`.

#### Light mode

| Pair                          | Ratio | Grade |
|-------------------------------|-------|-------|
| text on bg                    | 15.34 | AAA |
| text on surface               | 17.40 | AAA |
| text-secondary on bg          | 4.70  | AA |
| text-secondary on surface     | 5.33  | AA |
| **text-tertiary on surface**  | **4.54** | AA *(was 2.85, raised #999999 → #767676)* |
| on-primary on primary         | 7.54  | AAA |
| on-brand on brand-anchor      | 9.71  | AAA |
| **on-brand-muted on brand-anchor** | **5.72** | AA *(was 3.76 at alpha 0.5 — uppercase sidebar titles failed; new alpha 0.7 token)* |
| error-fg on error-bg          | 7.97  | AAA |
| success-fg on success-bg      | 7.36  | AAA |
| warning-fg on warning-bg      | 7.96  | AAA |
| **link on surface**           | **5.32** | AA *(was 4.30, darkened #2980B9 → #1A6FB0)* |
| primary on surface            | 7.54  | AAA |
| text-error on surface         | 5.44  | AA |

#### Dark mode

| Pair                          | Ratio | Grade |
|-------------------------------|-------|-------|
| text on bg                    | 13.66 | AAA |
| text on surface               | 11.71 | AAA |
| text-secondary on bg          | 7.72  | AAA |
| text-secondary on surface     | 6.62  | AA |
| **text-tertiary on surface**  | **4.79** | AA *(was 4.05, raised #888888 → #959595)* |
| on-primary on primary         | 8.31  | AAA |
| on-brand on brand-anchor      | 9.71  | AAA |
| on-brand-muted on brand-anchor | 5.72 | AA *(same as light mode — brand-anchor is theme-stable)* |
| error-fg on error-bg          | 9.06  | AAA |
| success-fg on success-bg      | 8.92  | AAA |
| warning-fg on warning-bg      | 8.77  | AAA |
| link on surface               | 5.25  | AA |
| primary on surface            | 7.13  | AAA |
| **text-error on surface**     | **4.73** | AA *(was 3.90, lifted #E0594B → #E87060)* |

## Patterns

### Adding a new modal

1. Add `NodeRef::<Div>::new()` for the container.
2. `a11y::install_focus_trap(dialog_ref, visible)` near the top
   of the component body.
3. Set `node_ref=dialog_ref` on the dialog wrapper.
4. `role="dialog"`, `aria-modal="true"`.
5. Add `aria-labelledby="<unique-id>"` pointing at the title, or
   `aria-label` if no visible title.
6. Wire `on:keydown` with Escape close + `a11y::handle_tab_trap`.

### Adding a new status message

- Confirmation, progress, non-blocking → `role="status"` +
  `aria-live="polite"`.
- Error, validation failure, blocking condition →
  `role="alert"`.
- Don't set both `role="alert"` and `aria-live` — alert is
  implicit-assertive; setting both is redundant and some screen
  readers double-announce.

### Adding a new color token

1. Check WCAG ratio against every surface it will sit on. Any
   online checker works; the formula is L = 0.2126R' + 0.7152G' +
   0.0722B' (gamma-corrected sRGB), contrast = (L1+0.05)/(L2+0.05)
   with L1 the lighter.
2. Body text needs ≥ 4.5:1; large text (≥ 18 pt or 14 pt bold)
   needs ≥ 3:1; non-text UI (icons, focus indicators, form-field
   borders) needs ≥ 3:1.
3. Add the token + computed ratio to the table above.

## Known gaps

These are knowingly carried forward; tracked in future-milestone
planning.

- **Arrow-key navigation inside the editor toolbar.** WAI-ARIA
  toolbar pattern recommends roving tabindex with arrow keys; we
  have one tab stop per button instead. Not a WCAG violation;
  changing it would be a usability tradeoff vs current behavior.
- **`prefers-reduced-motion`** — landed in M-P8 piece E. Global
  `@media (prefers-reduced-motion: reduce)` block in
  `frontend/style/main.css` collapses every animation duration
  + iteration-count + transition duration to 0.01ms (not 0ms —
  that breaks `transitionend` listeners). Future components
  should respect the user choice automatically; a re-introduced
  transition inside the media query is the documented escape
  hatch for the rare case where motion is the content.
- **High-contrast and Windows forced-colors mode.** Tokens render
  correctly under macOS Increase Contrast but Windows
  `forced-colors: active` removes our custom palette entirely.
  Phase 6 candidate.
- **Audio captions.** No audio/video produced by the product
  today. If video embeds or recorded narration land, captions and
  transcripts become a hard requirement (WCAG 1.2.1, 1.2.2).
- **Mobile screen-reader (VoiceOver iOS / TalkBack Android).** The
  M-P3 mobile checklist (`runbook/mobile-test-checklist.md`)
  references the manual a11y checklist here, but a documented
  device-by-device pass has not happened.

## Verification

Three layers, in increasing rigor:

1. **CI axe-core gate** (`playwright.yml`, M-P8 piece D) — fails
   the PR if any new "serious" or "critical" violation lands. Runs
   on home, editor, spreadsheet, and command palette open.
2. **Manual screen-reader checklist** (`runbook/a11y-screenreader-checklist.md`)
   — keyboard-only walkthrough plus NVDA + macOS VoiceOver. Done
   pre-release; results recorded in the release notes.
3. **External audit** — once per major release. Out of scope for
   Phase 5; tracked for Phase 6.
