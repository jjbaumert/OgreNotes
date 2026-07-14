---
name: verify
description: Build, launch, and drive the OgreNotes app locally to verify frontend/backend changes end-to-end (local compose stack + debug API serving the trunk dist + Playwright doctor scenarios).
---

# Verifying OgreNotes locally

Recipe proven 2026-07-13 (menus overhaul). Total cold time ≈ 5 min with
a warm cargo target.

## Bring-up

1. Infra: `docker compose up -d` at the repo root (DynamoDB Local
   :8000, MinIO :9000, Redis :6379). Ports may already be served by
   another checkout's stack — that's fine, share it, but use your own
   `DYNAMODB_TABLE_PREFIX`/`S3_BUCKET` so you never touch other data.
2. Env (mirror `.github/workflows/playwright.yml`): AWS shims
   (`minioadmin` keys, `AWS_ENDPOINT_URL_DYNAMODB/S3`),
   `DYNAMODB_TABLE_PREFIX=verifyN-`, `S3_BUCKET=verifyN-ogrenotes`,
   `REDIS_URL`, `DEV_MODE=true`, a ≥32-byte `JWT_SECRET`, dummy
   OAuth vars, `SEARCH_INDEX_PATH` in scratch, `API_PORT=3100`.
   `ADMIN_EMAILS=<email>` promotes that dev-login user to admin.
3. Provision: `cargo run --bin setup_dev -p ogrenotes-api` (needs the
   env; idempotent-ish — use a fresh prefix per campaign).
4. Frontend: `cd frontend && trunk build` (dev profile is what CI
   uses).
5. Serve: `FRONTEND_DIST=$PWD/frontend/dist ./target/debug/ogrenotes-api`
   (debug build links in seconds when the workspace is warm). Ready
   when `POST /api/v1/auth/dev-login` succeeds.

## Gotchas

- **Restart the API after every `trunk build`.** CSP inline-script
  hashes are computed once at startup from `index.html`; a stale
  server serves a CSP that blocks the new bootstrap script and the
  WASM never mounts (every scenario then times out on first selector).
- `cargo test`/`cargo check` for the frontend must run **inside
  `frontend/`** (excluded from the workspace). Native-testable logic
  must live in the lib (`src/lib.rs` modules) — `components/` is
  binary-only and CI's `cargo test --lib` never sees tests there.
- Playwright's `page.waitForFunction`/string evals are blocked by the
  app's CSP — poll `page.url()` or use locators instead.
- Known pre-existing flake: the first keystrokes typed into a
  just-mounted editor can be dropped/reordered under load (reproduced
  on main). Settle ~1s after `[data-editor-ready]` before typing.
- Doctor scenarios assume a clean per-user state (e.g. trash-flow's
  purge clicks the FIRST trash row); reruns against an accumulated
  table can fail spuriously — use a fresh table prefix.

## Drive

- Scenario harness: `scripts/frontend-doctor` (`npm install` once,
  browsers cached in `~/.cache/ms-playwright`):
  `node doctor.js --scenario <name> --base-url http://127.0.0.1:3100 --out <dir>`
  Menu-touching scenarios: menu-switch, trash-flow, doc-actions,
  find-replace, line-numbers, document-details, delete-document,
  comment-popup, spreadsheet-features, spreadsheet-freeze,
  spreadsheet-sheet-tabs, pivot-editor.
- Ad-hoc probes: drop a `probe-*.mjs` into `scripts/frontend-doctor/`
  (for its node_modules), auth via dev-login **cookies** (the app
  ignores localStorage tokens): forward the response's Set-Cookie
  headers into `context.addCookies`. Mobile: `devices["iPhone 13"]`
  context gives `hover: none`; long-press needs CDP
  `Input.dispatchTouchEvent` (touchStart → 700ms → touchEnd).
- Menu UI selectors: `.ui-menu`, `.ui-menu-item` (role=menuitem —
  does NOT match getByRole("button")), submenu parents
  `.ui-menu-item[aria-haspopup="menu"]`, fly-outs `.ui-menu-sub`.

## Teardown

Stop the API task, `docker compose down` (only if you started the
containers), delete probe scripts.
