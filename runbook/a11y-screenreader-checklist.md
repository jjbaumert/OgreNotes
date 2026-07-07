# Screen-reader manual test checklist

A human-run pass for screen-reader and keyboard accessibility.
Complement to the axe-core CI gate (`playwright.yml`), which
catches static violations. This checklist catches the things axe
can't reason about: announcement quality, interaction flow, and
"does this actually work for someone using a screen reader."

Run once before each release. Record results in the release notes
under "Accessibility audit."

## Setup

Pick ONE primary AT + browser combo per platform. The
recommendations below are the most widely deployed pairs as of
2026; substitute if your environment differs.

| Platform | Primary | Secondary |
|----------|---------|-----------|
| macOS    | VoiceOver + Safari | VoiceOver + Chrome |
| Windows  | NVDA 2024.x + Firefox | NVDA + Chrome |
| Linux    | Orca + Firefox | n/a |
| iOS      | VoiceOver + Safari | n/a |
| Android  | TalkBack + Chrome | n/a |

Verify in a clean profile (no extensions). Run against the
**deployed test env**, not localhost — service workers and
HttpOnly cookies behave differently behind ALB.

Cheat sheet of the AT commands you'll need:

| Action | VoiceOver | NVDA |
|--------|-----------|------|
| Start/stop reading | VO + A | NVDA + Down arrow |
| Next/prev element | VO + ←/→ | Down/Up arrow |
| Next/prev heading | VO + Cmd + H | H / Shift+H |
| Next/prev landmark | VO + Cmd + U → Landmarks rotor | D / Shift+D |
| Form-fields mode | (auto) | (auto, in forms) |
| Read modal title | VO + Cmd + L | NVDA + T |

## Scenarios

Each scenario lists the **expected** AT behavior. A failure to
match is a finding worth filing.

### S1 — Skip to content (every page)

1. Load home; press **Tab** once.
2. Expect: focus moves to a visible "Skip to main content" link
   above all other chrome. NVDA / VO should announce
   "Skip to main content, link".
3. Press **Enter**.
4. Expect: focus moves into the main page content. The next Tab
   from this point should land on the first interactive element
   inside `<main>` (mobile menu button or "New document" on home).

### S2 — Landmark navigation (home)

1. Open the landmark rotor (VO Rotor → Landmarks; NVDA "D").
2. Expect at minimum: **main**, **navigation** (sidebar), and
   **navigation** (breadcrumb).
3. Activate the breadcrumb landmark.
4. Expect: AT lands on the breadcrumb container and announces
   "Folder breadcrumb, navigation". The current folder should be
   announced with "current page" or equivalent.

### S3 — Document editor toolbar

1. Open any document; press **Tab** until focus lands on the
   editor toolbar.
2. Expect: announcement is "Document formatting toolbar, toolbar"
   (NVDA may also say "1 of N items").
3. Tab once.
4. Expect: focus moves to the Undo button. Announcement is
   "Undo, button" (or the localized equivalent).
5. Tab through the rest of the toolbar.
6. Expect: every button announces a meaningful name. Color
   pickers, dropdowns, and the more-actions overflow should
   announce their state (expanded / collapsed) when activated.

### S4 — Share dialog modal

1. Open a document. Click "Share" (or trigger via keyboard
   shortcut if one exists).
2. Expect: modal appears; AT announces "Share document, dialog"
   or "Share with, dialog" depending on the title key.
3. Focus should move into the modal — first focusable element
   (the email input) should receive focus automatically.
4. Press **Tab** repeatedly.
5. Expect: focus cycles within the modal. After the last button,
   the next Tab returns to the first focusable element. Focus
   never escapes to elements behind the modal.
6. Press **Escape**.
7. Expect: modal closes. Focus returns to the element that opened
   it (the Share button).

### S5 — Search palette (Ctrl-K)

1. From the home page, press **Ctrl+K** (or Cmd+K on Mac).
2. Expect: palette opens, AT announces "Search documents or run
   commands, dialog". Focus lands inside the search input.
3. Type ">" to enter Action mode.
4. Expect: the list of palette actions becomes navigable. Each
   item announces its label and (if any) keyboard shortcut.
5. Press **Tab** repeatedly.
6. Expect: focus stays within the palette. Esc closes and
   returns focus to the prior position.

### S6 — Confirm dialog (destructive op)

1. Select 1+ documents on home; click the bulk Delete button.
2. Expect: confirm dialog opens with focus on the (recommended:
   Cancel) primary action. AT announces the dialog title and the
   message body.
3. Press **Tab**, then **Shift+Tab**.
4. Expect: focus cycles between the two action buttons; never
   escapes to the document list behind.
5. Press **Escape**.
6. Expect: dialog closes, no destructive action taken.

### S7 — Live regions (save + error)

1. Open a document; edit a single character.
2. Expect: within 500 ms the sync indicator's text changes from
   "Saved" → "Saving…" → "Saved". AT should announce each
   transition without interrupting whatever the user is doing.
3. Disconnect the network (DevTools throttling or unplug Wi-Fi).
4. Edit another character.
5. Expect: indicator changes to "Offline — N pending". AT
   announces it politely.
6. Reconnect; expect "Saved" announcement.
7. (Errors path) Trigger a share failure by entering an
   unverified email.
8. Expect: an alert-role status announces the error immediately
   (assertive — interrupts current reading).

### S8 — Form errors (login)

1. Visit /login.
2. Submit the email form with an obviously bad value (empty or
   malformed).
3. Expect: error appears below the input. AT announces it via
   alert role.
4. Repeat for MFA enroll + MFA challenge pages.

### S9 — Reading order

1. With AT in continuous-read mode, listen to a fresh document
   page from top to bottom.
2. Expected order:
   1. Sidebar navigation
   2. Document title + breadcrumb
   3. Toolbar
   4. Document content (main)
   5. Comment thread sidebar (if open)
3. Anything that reads in an unexpected position is a finding.

### S10 — Color contrast spot check

(Run with browser DevTools' built-in contrast checker, OR axe in
the browser DevTools — this is the AT-independent verification.)

1. On home, check the timestamp column ("Added"). Expected
   contrast ≥ 4.5:1 against the surrounding row background.
2. On a document, check the placeholder "/" prompt in an empty
   paragraph. Expected ≥ 4.5:1.
3. Switch to dark theme. Repeat both.

## Reporting

For each scenario, record:

- **Pass / Fail**.
- If fail: AT + browser + OS version, the announcement that was
  expected, the announcement actually heard, and a screenshot
  / screen recording if it's visible.
- File a GitHub issue with the label `a11y` and reference this
  scenario by number (S1, S2, etc.).

## Quick smoke (15-minute version)

If you only have 15 minutes, run S1 → S4 → S5 → S6 on macOS +
VoiceOver. Those four cover landmarks, modals, palette, and
destructive flows — the dialog states most likely to regress
between releases.
