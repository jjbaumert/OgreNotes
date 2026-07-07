# Mobile hardware test checklist

Phase 5 M-P3 piece F. Manual checklist for verifying the
mobile-touched surfaces on **real devices**. The headless
Playwright doctor scenarios cover most of the same ground but
emulation can't reproduce: real OS soft-keyboard layout shifts,
PWA install prompts, iOS visualViewport reflow under keyboard,
real touch-target hit detection, real network conditions.

Run this before tagging a release that touches anything under:

- `frontend/src/components/formula_keyboard.rs`
- `frontend/src/components/sync_indicator.rs`
- `frontend/static/manifest.webmanifest` / `service-worker.js`
- `frontend/index.html` (PWA wiring)
- `frontend/style/responsive.css`
- Any touch-events code under `frontend/src/touch.rs`

## Device matrix

The intent is "two iOS major versions + Android current" — not an
exhaustive grid. Adjust as device baseline shifts.

| Device | Browser | Why |
|---|---|---|
| iPhone (any 14-pro+ shape) | Safari current | iOS visualViewport behaviour, safe-area-inset |
| iPad (any 10"+) | Safari current | Wider mobile viewport, no notch — touch path still hits |
| Android phone | Chrome current | Different soft-keyboard model than iOS |

If only one device is available, **iPhone Safari** is the priority
target — it has the strictest PWA + viewport handling.

## Before you start

- Use the deployed test environment (e.g.
  `https://ogrenotes.example.com`), not localhost — the service worker
  requires HTTPS and the PWA install prompt won't show on `http://`.
- Sign in with a clean test account (no pre-loaded documents).
- Open dev-tools / Web Inspector if available — console errors are
  often the only signal for silently-broken PWA wiring.

## 1. PWA install + offline shell  (M-P3 piece A)

| # | Step | Expected |
|---|---|---|
| 1.1 | Visit the home page | Page loads; no console errors mentioning "service-worker" or "manifest" |
| 1.2 | iOS Safari: tap Share → "Add to Home Screen" | Install sheet shows the OgreNotes icon (Swamp Green with layered marks); name reads "OgreNotes" |
| 1.3 | Android Chrome: open `⋮` menu | "Install app" or "Add to Home Screen" entry present (Chrome surfaces it once the manifest is parsed) |
| 1.4 | Install + launch from home-screen icon | App opens in standalone shell (no Safari/Chrome browser chrome); status bar / nav uses the theme-color metas |
| 1.5 | While installed, kill the radio (airplane mode) and re-launch | App-shell HTML + WASM still loads; the page renders past the title; further data fetches fail visibly (see §3) |
| 1.6 | Re-enable network, reload | Service worker fetches fresh shell on next nav (stale-while-revalidate); no broken-cache symptoms (blank page, mismatched CSS) |

## 2. Spreadsheet 3-mode keyboard  (M-P3 piece C)

Open or create a spreadsheet doc.

| # | Step | Expected |
|---|---|---|
| 2.1 | Tap an empty cell | Cell selects; OS soft keyboard appears (Standard mode default) |
| 2.2 | Type a letter | Cell enters edit; characters land normally via OS keyboard |
| 2.3 | Commit (Enter / arrow), then tap an empty cell and type `=` | OS keyboard hides; in-page keyboard appears in Formula mode (chips + operators) |
| 2.4 | Tap "Numeric" tab in the in-page keyboard while value starts with `=` | Tab is **disabled / locked** (auto-Formula override holds) |
| 2.5 | Backspace until value is empty, then tap "Numeric" tab | Tab unlocks; layout flips to number pad (7-9, 4-6, 1-3, 0/./-/%) |
| 2.6 | Tap the digits 1-2-3 | Cell value reads `123`; OS keyboard stays hidden (`inputmode="none"` honored) |
| 2.7 | Tap "Standard" tab | In-page keyboard collapses to a thin strip (mode tabs + commit/cancel); OS keyboard re-appears |
| 2.8 | Tap commit (↵) | Cell value committed; in-page keyboard hides |

**iOS-specific Standard-mode caveat** (v1 known limitation): the
Standard-mode strip uses `position: fixed; bottom: 0` and is
covered by the OS keyboard on iPhone Safari, so the mode tabs
aren't tappable while the OS keyboard is up. To switch out of
Standard mode, commit the edit first (mode persists), then tap a
new cell and start typing `=` for Formula or tap the Numeric tab
*before* the OS keyboard appears. The proper fix needs a
`visualViewport` reposition pass — see the M-P3 piece C commit
message.

## 3. Sync indicator pill  (M-P3 piece B)

Open any document. The header should show a green "Saved" pill.

| # | Step | Expected |
|---|---|---|
| 3.1 | Sit idle 5 s on a loaded doc | Pill reads "Saved" (green) |
| 3.2 | Type into the doc | Pill briefly flips to "Saving…" (amber, pulsing) then back to "Saved" within ~1 s |
| 3.3 | Enable airplane mode | Pill flips to "Offline" (red) within ~500 ms; if you typed during the transition, suffix reads `Offline — N pending` |
| 3.4 | Type while offline | Pill updates the pending count; text accumulates locally (no error) |
| 3.5 | Re-enable network | Pill returns to "Saved" once the WebSocket reconnects + the queue drains |
| 3.6 | Toggle `prefers-reduced-motion` in OS settings, repeat 3.2 | "Saving…" pill does **not** pulse (animation muted) |

## 4. Formula autocomplete reanchor  (M-P3 piece D)

In a spreadsheet doc, tap a cell **near the bottom of the viewport**
(below the screen midpoint).

| # | Step | Expected |
|---|---|---|
| 4.1 | Type `=SU` | In-page Formula keyboard shows AND the function picker appears |
| 4.2 | Visually locate the picker | Picker sits **above** the in-page keyboard band (anchored to the keyboard's top edge), not below the cell. Not covered or clipped |
| 4.3 | First match displayed | `SUM` is the top item |
| 4.4 | Tap `SUM` chip | Cell value becomes `=SUM(` ; picker closes |

## 5. Touch ergonomics

Spot-check, not exhaustive.

| # | Step | Expected |
|---|---|---|
| 5.1 | Tap every header-level button (file menu, share, sync pill, bell) | Each is at least 44×44 CSS px (iOS HIG); no accidental neighbor-tap |
| 5.2 | Pinch-zoom the editor | Pinch is allowed up to `maximum-scale=5` per index.html viewport meta |
| 5.3 | Rotate device portrait ↔ landscape | Layout reflows without overflow / horizontal scrollbar |

## 6. Reporting findings

If a row above produces an unexpected result, capture:

- Device model + OS version + browser version
- Steps that diverged from "Expected"
- Screenshot (especially for visual issues — keyboard overlap, popup clipping, theme-color mis-render)
- Console log if available

File via the usual issue path; tag with `area:mobile` and reference
this checklist. A regression in any §1-§4 row is a Phase 5 close-
criteria blocker.
