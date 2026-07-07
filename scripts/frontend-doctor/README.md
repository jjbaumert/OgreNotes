# frontend-doctor

Headless-browser diagnostic harness for OgreNotes. Driven by the
`.claude/agents/frontend-doctor.md` subagent; can also be run directly.

## One-time setup

```bash
cd scripts/frontend-doctor
npm install
npx playwright install chromium
```

The Chromium download is ~170 MB; it's a one-time cost.

## Target requirements

Dev-login is the only supported auth path. That means the target stack must
have `DEV_MODE=true` set on the ECS task. For production-style stacks, flip
`DEV_MODE=true` in `scripts/aws-test-config.env`, run
`./scripts/aws-redeploy.sh`, run the doctor, then flip it back.

The `DEV_MODE=true` endpoint is `/api/v1/auth/dev-login` (returns a signed
JWT for any email — obviously not safe on a real internet-facing stack).

## Running a scenario manually

### collab-sync (needs an existing docId)

```bash
node doctor.js \
    --scenario collab-sync \
    --base-url https://ogrenotes.example.com \
    --doc-id <documentId> \
    --out /tmp/fe-doctor-$(date +%s)
```

### trash-flow (creates its own doc)

Exercises delete → Trash → read-only banner → restore → purge. No existing
doc needed — the harness POSTs `/api/v1/documents` itself.

```bash
node doctor.js \
    --scenario trash-flow \
    --base-url https://ogrenotes.example.com \
    --out /tmp/fe-doctor-$(date +%s)
```

### spreadsheet-paste (creates its own spreadsheet)

Verifies Ctrl+V in the spreadsheet doesn't trigger a clipboard-permission
popup and that relative refs translate on paste. Creates a spreadsheet,
types the scenario from the original bug report (A1..A3 = 1,2,3;
A4 = `=SUM(A1:A3)`; B1..B3 = 3,4,5), copies A4 → pastes B4, and asserts
the formula bar shows `=SUM(B1:B3)` with displayed value `12`. The
browser context is created WITHOUT `clipboard-read` permission — the
whole point is that the paste path doesn't reach for `readText()`.

```bash
node doctor.js \
    --scenario spreadsheet-paste \
    --base-url https://ogrenotes.example.com \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps in the report: `gridMounted`, `cellsPopulated`,
`pasteExecuted`, `pastedFormulaTranslated`, `pastedValueIs12`,
`noPermissionDialog`.

### spreadsheet-lifecycle (creates two spreadsheets)

Exercises the spreadsheet view's mount/unmount reclaim path (#76). After
#4 finding 1, the view reclaims its engine + `fetched_ids` on `on_cleanup`
via an `Arc<AtomicBool>` liveness flag whose safety rests on WASM
single-threaded scheduling — and there's no `cargo test` coverage of the
unmount path. The scenario creates docs A and B, navigates back and forth
ten times (each navigation unmounts the previous grid and runs its
cleanup), then on the surviving doc A runs a copy/paste — a *second*
`spawn_local` that must find the engine still live. Asserts structural
correctness (no panic, no use-after-free), not heap numbers, since
Chrome's `usedJSHeapSize` is too noisy to gate on.

```bash
node doctor.js \
    --scenario spreadsheet-lifecycle \
    --base-url http://127.0.0.1:3000 \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps in the report: `bothDocsCreated`, `loopsCompleted`,
`postUnmountPasteWorks`, `noPageErrors`, `noPanicConsole`.

### focus-mode (creates its own document)

Exercises the #134 focus/expand toggle. The single header button
(`.focus-toggle-btn`) flips to its opposite (⤢ enter ↔ ✕ exit) and toggles
`.focus-mode` on `.app-layout` (hiding the sidebar + menu bar). The scenario
enters and exits focus mode via both the button and `Ctrl+Shift+F`, asserts
the chrome hides/shows and the toggle stays mounted, and — most importantly —
that nothing panics on teardown (this area has a history of the Firefox
"closure invoked recursively or after being dropped" class).

```bash
node doctor.js \
    --scenario focus-mode \
    --base-url http://127.0.0.1:3000 \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps: `startsUnfocused`, `menuVisibleInitially`, `buttonEntersFocus`,
`menuHiddenInFocus`, `toggleStillPresentInFocus`, `buttonExitsFocus`,
`menuVisibleAfterExit`, `shortcutEntersFocus`, `shortcutExitsFocus`,
`noPageErrors`, `noPanicConsole`.

### menu-switch (creates its own document)

Regression for the menu-bar backdrop-intercept bug. With a menu open, its
full-screen `.menu-bar-backdrop` used to cover the other menu names, so
clicking one only closed the current menu (the click hit the backdrop) — you
had to click twice. The scenario opens the Document menu, then clicks View,
then Format, asserting each switches in a single click (`.menu-bar-item.open`
follows + exactly one `.menu-bar-dropdown`), and that clicking the open
menu's own name closes it.

```bash
node doctor.js \
    --scenario menu-switch \
    --base-url http://127.0.0.1:3000 \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps: `documentOpened`, `switchedToViewInOneClick`,
`switchedToFormatInOneClick`, `sameNameCloses`, `noPageErrors`.

### doc-actions (creates its own spreadsheet)

Exercises the #146 Document-menu **Rename** + **Duplicate** actions. Runs on a
spreadsheet because spreadsheets carry an explicit, assertable title (a
document's title is derived from its first line, so it's not a clean witness
for a name check). Types a value in A1, renames via the menu (the prompt is
auto-accepted) and asserts the header title input updates, then opens the
**Duplicate dialog** (asserting the name is pre-filled from the current doc
name), gives it a distinct name, confirms the default destination, and asserts
it navigated to a new doc with that name and the copied A1 cell. (The dialog's
shared-folder warning isn't covered here — it needs a multi-user shared folder
to set up.)

```bash
node doctor.js \
    --scenario doc-actions \
    --base-url http://127.0.0.1:3000 \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps: `renameUpdatesTitle`, `duplicateDialogPrefillsName`,
`duplicateNavigatedToNewDoc`, `duplicateUsesEnteredName`,
`duplicateCopiedContent`, `noPageErrors`.

### favorites (creates its own document)

Exercises the #144 favorites feature. Stars the doc via the header toggle and
asserts the button goes active and the doc appears in the sidebar Favorites
section (live, via the `favorites_refresh` tick), then unstars and asserts
both revert.

```bash
node doctor.js \
    --scenario favorites \
    --base-url http://127.0.0.1:3000 \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps: `startsUnstarred`, `starButtonGoesActive`, `appearsInSidebar`,
`unstarButtonGoesInactive`, `leavesSidebar`, `noPageErrors`.

### mfa-flow (creates one user, exercises full MFA lifecycle)

Phase 4 M-E3 verification. Dev-logs in a fresh user, navigates the
enrollment page, scrapes the Base32 secret + first recovery code
out of the rendered UI, generates a TOTP via `otplib`, completes
enrollment, logs out, logs back in (expecting a 202 + MFA-pending
handle), drives the TOTP challenge page, then logs out + back in
once more to exercise the recovery-code fallback.

```bash
node doctor.js \
    --scenario mfa-flow \
    --base-url https://ogrenotes.example.com \
    --email-a doctor-mfa@ogrenotes.example.com \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps in the report: `initialLogin`,
`enrollPageRenderedSecret`, `recoveryCodesDisplayed`,
`verifyFinalizedEnrollment`, `loggedOutAfterEnroll`,
`reloginReturnedMfaPending202`, `totpChallengeMintedSession`,
`postChallengeMeWorks`, `recoveryCodeMintedSession`.

The email is suffixed with `+<timestamp>` so re-runs against the
same stack don't collide on existing enrollment state. The captured
TOTP secret and recovery codes are NOT written into `report.json` —
the scenario records boolean step pass/fail only.

### a11y-audit (creates one document)

Phase 5 M-P8 piece D. Walks three high-traffic surfaces (home,
document editor, command palette open) and runs `@axe-core/playwright`
with the WCAG 2.1 A + AA rule set on each. Threshold is **zero
serious + zero critical** violations per surface; minor / moderate
findings land in `axe-results.json` but do not fail the scenario.

```bash
node doctor.js \
    --scenario a11y-audit \
    --base-url https://ogrenotes.example.com \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps in the report: `homeMounted`,
`homeNoSeriousOrCritical`, `editorMounted`,
`editorNoSeriousOrCritical`, `paletteMounted`,
`paletteNoSeriousOrCritical`.

When a `*NoSeriousOrCritical` step fails, open
`<out>/axe-results.json` for the per-surface violation detail —
each entry carries the axe rule id, the impact, the count, and
the first three offending element selectors. Cross-reference axe's
rule documentation (https://dequeuniversity.com/rules/axe) for fix
guidance.

### ask-flow (creates one document, calls real Claude API)

Phase 6 M-6.2 piece E. Walks the full agentic Ask flow: opens
the dialog via the command palette (`Ctrl+K → ">ask" → Enter`),
asks a question targeting a seeded document with a distinctive
title (`PineappleAuth-<timestamp>`), waits for the streaming
SSE response, and asserts at least one source citation appears
with the seeded doc's id.

Costs real Claude tokens (~$0.01 per run with `claude-sonnet-4-6`).
Run locally only when you actually want to test the agent
end-to-end; the CI job skips this scenario on PR builds from
forks since GitHub doesn't expose `secrets.ANTHROPIC_API_KEY`
to forked-PR contexts.

```bash
ANTHROPIC_API_KEY=sk-ant-... node doctor.js \
    --scenario ask-flow \
    --base-url https://ogrenotes.example.com \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps in the report: `homeMounted`, `paletteOpened`,
`askDialogOpened`, `questionSubmitted`, `askEndpointAvailable`,
`answerStreamed`, `sourcesAppeared`, `firstCitationMatchesSeed`.
A missing `askEndpointAvailable` means the API server received
the request but couldn't reach Claude — check the server log
for `ask.claude_api_errors_total` events and the
`ANTHROPIC_API_KEY` env var on the task.

### admin-console (creates two users)

Phase 4 M-E2 verification. Dev-logs in as an admin email, dev-logs
in a peer, navigates `/admin/users`, searches by the peer's email
prefix, clicks Disable then Enable on the peer row, then checks
`/admin/audit` shows both events for the peer.

**Prerequisite on the target stack:** the admin email must be in
`ADMIN_EMAILS` (the auth-callback path auto-promotes matching
emails on dev-login). Default email is
`doctor-admin@ogrenotes.example.com` — override via `--email-a`.

```bash
node doctor.js \
    --scenario admin-console \
    --base-url https://ogrenotes.example.com \
    --email-a doctor-admin@ogrenotes.example.com \
    --email-b doctor-peer@ogrenotes.example.com \
    --out /tmp/fe-doctor-$(date +%s)
```

Required steps in the report: `adminUsersPageMounted`,
`peerRowVisibleAfterSearch`, `peerDisabled`, `peerReEnabled`,
`auditRowsVisible`. A missing `adminUsersPageMounted` typically
means the admin email isn't in `ADMIN_EMAILS`; check `report.json`
for the explicit error message including the current path.

The `report.json` / trailing `FRONTEND_DOCTOR_REPORT` line contains a
`scenario.steps` map (`docLoaded`, `deletedAndHomeNav`, `trashBannerShown`,
`editorReadonly`, `restoredAndHomeNav`, `purgedApi404`, …). Any missing step
flips `ok` to `false`.

Output directory contains:

- `report.json` — machine-readable summary (agents parse this).
- `tab-a.har` / `tab-b.har` — full HAR archives; drop into Chrome devtools'
  Network panel to replay.
- `tab-a.png` / `tab-b.png` — final-state screenshots.

Final stdout line is `FRONTEND_DOCTOR_REPORT <json>` with the report inline
for quick piping to `jq`.
