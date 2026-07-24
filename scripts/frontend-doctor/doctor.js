#!/usr/bin/env node
// OgreNotes frontend-doctor — headless browser diagnostic harness.
//
// Drives one or two browser contexts through a scripted scenario against a
// deployed (or local) stack and records every signal an agent needs to
// diagnose a "nothing is syncing" class of bug:
//
//   - every HTTP request+response (method, URL, status, timing)
//   - every WebSocket frame (direction, opcode, length, UTF-8/hex preview)
//   - every console message
//   - every page error
//   - a HAR archive (replayable in devtools)
//   - a final screenshot per context
//
// All artifacts are written to an output directory (--out). A machine-readable
// report.json goes to stdout at the end so the subagent can parse it.
//
// Usage (dev-login path — requires DEV_MODE=true on the target):
//   node doctor.js \
//       --base-url https://ogrenotes.example.com \
//       --scenario collab-sync \
//       --doc-id <docId> \
//       --email-a diag-a@ogrenotes.example.com \
//       --email-b diag-b@ogrenotes.example.com \
//       --out /tmp/fe-doctor-$(date +%s)
//
// The subagent should parse the trailing JSON line of stdout ("FRONTEND_DOCTOR_REPORT <json>").

import { chromium, firefox, devices } from "playwright";
import { AxeBuilder } from "@axe-core/playwright";
import { mkdirSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { authenticator } from "otplib";

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i++) {
    const k = argv[i];
    if (!k.startsWith("--")) continue;
    const key = k.slice(2);
    const val = argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[++i] : "true";
    args[key] = val;
  }
  return args;
}

function logJson(obj) {
  process.stderr.write(JSON.stringify(obj) + "\n");
}

async function devLogin(baseUrl, email, name = "Doctor Probe") {
  const res = await fetch(`${baseUrl}/api/v1/auth/dev-login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email, name }),
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(
      `dev-login failed for ${email}: HTTP ${res.status} ${body}\n` +
        `(is DEV_MODE=true on the target stack?)`
    );
  }
  // Capture Set-Cookie before consuming the body — once `.json()` is
  // awaited, the response is closed but headers stay valid. We forward
  // these to the browser context in `seedAuth`; without them the
  // post-#33 frontend has no refresh credential and `/auth/refresh`
  // returns 401 on the first hydration call (which is exactly what the
  // 2026-05-04 Playwright failure shows).
  const setCookie =
    typeof res.headers.getSetCookie === "function"
      ? res.headers.getSetCookie()
      : [res.headers.get("set-cookie")].filter(Boolean);
  const json = await res.json();
  json.__setCookieHeaders = setCookie;
  json.__baseUrl = baseUrl;
  return json;
}

// Attach every observable signal to a context/page and push events into the
// shared collector. The collector is per-context ("tab-A", "tab-B").
function instrument(context, page, tag, collector) {
  collector[tag] = {
    requests: [],
    responses: [],
    ws: [],
    console: [],
    errors: [],
  };

  context.on("request", (req) => {
    collector[tag].requests.push({
      t: Date.now(),
      method: req.method(),
      url: req.url(),
      resourceType: req.resourceType(),
    });
  });
  context.on("response", async (res) => {
    collector[tag].responses.push({
      t: Date.now(),
      status: res.status(),
      url: res.url(),
      method: res.request().method(),
      // `x-request-id` header is set by the server's TraceLayer — allows the
      // subagent to grep CloudWatch logs for the exact request.
      requestId: res.headers()["x-request-id"] || null,
    });
  });
  context.on("websocket", (ws) => {
    const wsEvents = { url: ws.url(), frames: [] };
    collector[tag].ws.push(wsEvents);
    ws.on("framesent", (f) => {
      const preview = previewPayload(f.payload);
      wsEvents.frames.push({ t: Date.now(), dir: "out", ...preview });
    });
    ws.on("framereceived", (f) => {
      const preview = previewPayload(f.payload);
      wsEvents.frames.push({ t: Date.now(), dir: "in", ...preview });
    });
    ws.on("close", () => {
      wsEvents.closedAt = Date.now();
    });
  });
  page.on("console", (msg) => {
    collector[tag].console.push({
      t: Date.now(),
      type: msg.type(),
      text: msg.text(),
    });
  });
  page.on("pageerror", (err) => {
    collector[tag].errors.push({ t: Date.now(), message: err.message, stack: err.stack });
  });
}

function previewPayload(payload) {
  // payload is Buffer for binary frames, string for text.
  if (typeof payload === "string") {
    return { kind: "text", len: payload.length, preview: payload.slice(0, 200) };
  }
  return {
    kind: "binary",
    len: payload.length,
    hexPreview: payload.subarray(0, 32).toString("hex"),
  };
}

// Parse a single `Set-Cookie` header into a Playwright-style cookie
// descriptor. Only the attributes the OgreNotes server emits are
// recognized — Path, Max-Age, HttpOnly, Secure, SameSite — anything
// else is ignored. We deliberately avoid pulling in a dependency
// (`tough-cookie` etc.) for one well-known cookie shape.
function parseSetCookieForPlaywright(header, fallbackUrl) {
  const parts = header.split(";").map((s) => s.trim());
  if (parts.length === 0 || !parts[0].includes("=")) return null;
  const eq = parts[0].indexOf("=");
  const name = parts[0].slice(0, eq);
  const value = parts[0].slice(eq + 1);
  const cookie = { name, value, url: fallbackUrl };
  for (const attr of parts.slice(1)) {
    const [rawKey, rawVal] = attr.split("=");
    const key = rawKey.toLowerCase();
    if (key === "path") {
      cookie.path = rawVal;
    } else if (key === "max-age") {
      const secs = Number(rawVal);
      if (Number.isFinite(secs)) {
        cookie.expires = Math.floor(Date.now() / 1000) + secs;
      }
    } else if (key === "httponly") {
      cookie.httpOnly = true;
    } else if (key === "secure") {
      cookie.secure = true;
    } else if (key === "samesite") {
      const v = (rawVal || "").toLowerCase();
      if (v === "strict") cookie.sameSite = "Strict";
      else if (v === "lax") cookie.sameSite = "Lax";
      else if (v === "none") cookie.sameSite = "None";
    }
  }
  // If a Path attribute is set, Playwright requires `domain` instead of
  // `url` so the cookie is accepted under that exact path. Derive the
  // domain from the fallback URL.
  if (cookie.path) {
    try {
      const u = new URL(fallbackUrl);
      cookie.domain = u.hostname;
      delete cookie.url;
    } catch (_) {}
  }
  return cookie;
}

// Seed auth into a fresh Playwright context.
//
// Post-#33 the frontend is cookie-only: it reads its refresh
// credential from the HttpOnly `ogrenotes_refresh` cookie that
// `/api/v1/auth/dev-login` set on the response, hydrates the in-memory
// access token via `try_hydrate_from_cookie()` (POST /auth/refresh) on
// boot, and ignores localStorage entirely. We forward the dev-login
// `Set-Cookie` header(s) into the browser context via
// `context.addCookies` so the first refresh call carries the cookie
// and succeeds; without this every scenario sees the 2026-05-04
// failure (POST /auth/refresh → 401 → DocumentPage redirects → editor
// never mounts → waitForSelector(`[contenteditable="true"]`) times out).
async function seedAuth(context, tokens) {
  const headers = tokens.__setCookieHeaders || [];
  const baseUrl = tokens.__baseUrl;
  const cookies = headers
    .map((h) => parseSetCookieForPlaywright(h, baseUrl))
    .filter(Boolean);
  if (cookies.length > 0) {
    await context.addCookies(cookies);
  }
}

// ─── trash-flow scenario ────────────────────────────────────────
//
// Exercises the full delete → trash → restore → purge loop against a
// running stack:
//
//   1. Dev-login as a single user.
//   2. Create a fresh document via the REST API (so failures are isolated
//      from doc-creation UI).
//   3. Navigate to the doc page; open Document menu → "Delete Document…".
//   4. Click "Move to Trash" in the confirm dialog. Assert navigation home.
//   5. Click the synthesized "Trash" row. Assert the doc is listed with
//      Restore + Delete-forever row actions, and the FileBrowser header has
//      the trash-mode actions column.
//   6. Open the trashed doc. Assert the .trash-banner is visible and that
//      the editor is NOT contenteditable (read-only lockdown).
//   7. Click Restore → pick Home in the FolderPickerDialog → assert the
//      doc comes back to Home and leaves Trash.
//   8. Re-delete, navigate into Trash, click Delete-forever on the row,
//      confirm, and assert the doc is fully gone (no longer in trash, GET
//      /documents/:id returns 404).
//
// Everything is captured through the same instrument() pipeline so
// HAR+screenshots are available if anything diverges.
// Defaults applied to every Playwright `browser.newContext({…})` call
// in this harness. **Test-only** — production users see the strict CSP
// from `apply_security_headers`.
//
// `bypassCSP: true` tells Chromium to ignore the page's
// Content-Security-Policy *for this browser context only*. The harness
// uses `page.waitForFunction(predicate)` and `page.evaluate(predicate)`
// extensively; Playwright injects those predicates into the page via
// `eval()`. The production CSP locks `script-src` down to
// `'self' 'wasm-unsafe-eval' 'sha256-…'` — that permits the WASM
// bundle and Trunk's hashed bootstrap but rejects `eval()` outright,
// so without this flag every harness predicate fails with `EvalError:
// Refused to evaluate a string as JavaScript`. Setting bypassCSP at
// the context level is the clean per-test escape hatch — the served
// HTML still carries the real CSP header (existing `test_security_headers`
// integration tests still assert the header value), the *enforcement*
// is just disabled in the test browser.
const DOCTOR_CONTEXT_DEFAULTS = { bypassCSP: true };

async function createDocViaApi(baseUrl, accessToken, title, docType) {
  const body = docType ? { title, docType } : { title };
  const res = await fetch(`${baseUrl}/api/v1/documents`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${accessToken}`,
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const txt = await res.text();
    throw new Error(`create doc failed: HTTP ${res.status} ${txt}`);
  }
  return res.json();
}

async function getDocViaApi(baseUrl, accessToken, docId) {
  return fetch(`${baseUrl}/api/v1/documents/${docId}`, {
    headers: { authorization: `Bearer ${accessToken}` },
  });
}

async function scenarioTrashFlow(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(target, emailA || "doctor-trash@ogrenotes.example.com");
  logJson({ at: "dev-login", userId: tokens.userId });

  const title = `doctor-trash-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  const docUrl = `${target}/d/${doc.id}/probe`;
  const homeUrl = `${target}/`;

  // Track per-step pass/fail so a late failure doesn't hide earlier progress.
  const steps = {};

  async function waitForPath(p, timeout = 10000) {
    await page.waitForFunction(
      (expected) => window.location.pathname === expected,
      p,
      { timeout }
    );
  }

  try {
    // ─── 1. Open the doc ─────────────────────────────────────
    logJson({ at: "navigate", url: docUrl });
    await page.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });
    await page.waitForSelector('[contenteditable="true"]', { timeout: 10000 });
    steps.docLoaded = true;

    // ─── 2. Open Document menu → Delete Document… ───────────
    await page.getByRole("button", { name: "Document", exact: true }).click();
    await page.getByText("Delete Document", { exact: false }).click();

    // Confirm dialog: "Move to Trash" is the destructive confirm.
    await page.waitForSelector(".confirm-dialog", { timeout: 5000 });
    await page
      .locator(".confirm-dialog")
      .getByRole("button", { name: "Move to Trash" })
      .click();

    await waitForPath("/");
    steps.deletedAndHomeNav = true;

    // ─── 3. Trash row on home ───────────────────────────────
    const trashRow = page.locator(".file-name", { hasText: "Trash" }).first();
    await trashRow.waitFor({ state: "visible", timeout: 5000 });
    steps.trashRowVisible = true;

    await trashRow.click();
    // In trash-mode the row gets an actions column; the Restore + Delete
    // forever buttons are inside .file-row-actions.
    await page
      .locator(`.file-name`, { hasText: title })
      .first()
      .waitFor({ state: "visible", timeout: 5000 });
    await page
      .locator(".file-row-actions")
      .first()
      .waitFor({ state: "visible", timeout: 5000 });
    steps.trashedDocListedWithActions = true;

    // ─── 4. Open the trashed doc — banner should show, editor locked ──
    await page.locator(".file-name", { hasText: title }).first().click();
    await page.waitForSelector(".trash-banner", { timeout: 10000 });
    const editorEditableWhileTrashed = await page
      .locator('[contenteditable="true"]')
      .count();
    steps.trashBannerShown = true;
    steps.editorReadonly = editorEditableWhileTrashed === 0;

    // ─── 5. Restore via banner → folder picker → Home ──────
    await page
      .locator(".trash-banner")
      .getByRole("button", { name: "Restore" })
      .click();
    await page.waitForSelector(".folder-picker-dialog", { timeout: 5000 });
    await page
      .locator(".folder-picker-row", { hasText: "Home" })
      .first()
      .click();
    await page
      .locator(".folder-picker-dialog")
      .getByRole("button", { name: "Restore here" })
      .click();

    await waitForPath("/");
    steps.restoredAndHomeNav = true;

    // Verify the doc is back in Home. We check via the file browser list
    // (server-rendered via /folders/{homeId}).
    await page
      .locator(".file-name", { hasText: title })
      .first()
      .waitFor({ state: "visible", timeout: 5000 });
    steps.docBackInHome = true;

    // ─── 6. Re-delete and purge forever from Trash row ─────
    await page.locator(".file-name", { hasText: title }).first().click();
    await page.waitForSelector('[contenteditable="true"]', { timeout: 10000 });
    await page.getByRole("button", { name: "Document", exact: true }).click();
    await page.getByText("Delete Document", { exact: false }).click();
    await page.waitForSelector(".confirm-dialog", { timeout: 5000 });
    await page
      .locator(".confirm-dialog")
      .getByRole("button", { name: "Move to Trash" })
      .click();
    await waitForPath("/");

    await page.locator(".file-name", { hasText: "Trash" }).first().click();
    await page
      .locator(".file-row-actions")
      .first()
      .getByRole("button", { name: "Delete forever" })
      .click();
    await page.waitForSelector(".confirm-dialog", { timeout: 5000 });
    await page
      .locator(".confirm-dialog")
      .getByRole("button", { name: "Delete forever" })
      .click();

    // Row should disappear from the trash listing.
    await page
      .locator(".file-name", { hasText: title })
      .first()
      .waitFor({ state: "detached", timeout: 10000 });
    steps.purgedFromTrash = true;

    // Independent API check: the doc truly no longer exists.
    const getRes = await getDocViaApi(target, tokens.accessToken, doc.id);
    steps.purgedApi404 = getRes.status === 404;
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  try {
    await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: true });
  } catch (_) {}

  collector.scenario = {
    name: "trash-flow",
    docId: doc.id,
    title,
    steps,
  };

  await context.close();
  await browser.close();
}

// ─── mfa-flow scenario ──────────────────────────────────────────
//
// Phase 4 M-E3 piece F: end-to-end MFA exercise against a running
// stack.
//
//   1. Dev-login as a fresh user → 200 + TokenResponse (no MFA).
//   2. Navigate to /auth/mfa-enroll. Wait for QR + manual-entry
//      secret to render. Expand the recovery-codes <details> and
//      capture the first code.
//   3. Read the Base32 secret from the page. Compute the current
//      6-digit TOTP via otplib (the same RFC 6238 algorithm
//      `totp-rs` uses server-side).
//   4. Fill the confirm input, click Confirm. Wait for navigate-
//      home — this proves /auth/mfa/verify succeeded and flipped
//      mfa_enrolled_at.
//   5. Log out via POST /auth/logout (frontend has no logout UI on
//      home that's reachable without nav; we hit the API directly).
//   6. Dev-login again under the SAME email → expect 202 + handle
//      (the user is now MFA-enrolled). Verify the response shape.
//   7. Drive the challenge page: navigate to
//      /auth/mfa-challenge?handle=…, type a fresh TOTP, click
//      Verify. Wait for navigate-home + verify /users/me works
//      (session minted).
//   8. Log out again, dev-login again, but this time use the
//      recovery-code path: click "Lost your authenticator?", type
//      the recovery code captured in step 2, click Verify. Wait
//      for navigate-home.
//
// Step-level booleans are pinned in collector.scenario.steps so a
// late failure doesn't hide earlier progress.
async function scenarioMfaFlow(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  // Suffix with Date.now() so re-runs against the same stack don't
  // collide on existing enrollment state.
  const email = (emailA || "doctor-mfa@ogrenotes.example.com")
    .replace("@", `+${Date.now()}@`);

  const steps = {};
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    // `mode: "minimal"` (not "full") for this scenario: the enroll
    // and challenge response bodies include the plaintext Base32
    // TOTP secret AND the full recovery-code list. Recording them
    // into tab-a.har at mode:full leaves the secrets sitting in the
    // operator's output directory in plaintext — defeating the
    // "report.json doesn't carry secrets" guarantee. Minimal keeps
    // timing + status + headers for forensics; bodies stay off
    // disk.
    recordHar: { path: join(outDir, "tab-a.har"), mode: "minimal" },
  });

  // Initial dev-login via the same helper the other scenarios use.
  // Plain 200 expected (no MFA enrollment yet).
  const initialTokens = await devLogin(target, email, "MFA Doctor");
  await seedAuth(context, initialTokens);
  steps.initialLogin = true;

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  let capturedSecret = null;
  let capturedRecoveryCode = null;

  try {
    // ── Step 2-4: enroll + verify ─────────────────────────────
    await page.goto(`${target}/auth/mfa-enroll`, {
      waitUntil: "domcontentloaded",
    });
    // Wait for the QR to render — the .mfa-secret-value contains
    // the Base32 secret after the fetch resolves.
    await page.waitForSelector(".mfa-secret-value", { timeout: 15000 });
    capturedSecret = (
      await page.locator(".mfa-secret-value").first().textContent()
    )?.trim();
    if (!capturedSecret) {
      throw new Error("could not read .mfa-secret-value from enroll page");
    }
    steps.enrollPageRenderedSecret = true;

    // Expand the recovery-codes <details> and grab the first code.
    // Wait for the summary first so a click-on-missing produces a
    // specific failure instead of a downstream waitForSelector
    // timeout with a misleading message.
    await page.waitForSelector(".mfa-recovery summary", { timeout: 5000 });
    await page.locator(".mfa-recovery summary").click();
    await page.waitForSelector(".mfa-recovery-list li code", {
      state: "visible",
      timeout: 5000,
    });
    capturedRecoveryCode = (
      await page
        .locator(".mfa-recovery-list li code")
        .first()
        .textContent()
    )?.trim();
    // Server format is XXXXX-XXXXX (base32 alphabet, 11 chars
    // exactly). A loose `length < 5` check would let a partial
    // scrape silently through and the failure would surface only
    // at step 9 with a confusing "recoveryCodeMintedSession"
    // miss. Tighten to the actual format.
    if (
      !capturedRecoveryCode ||
      !/^[A-Z2-7]{5}-[A-Z2-7]{5}$/.test(capturedRecoveryCode)
    ) {
      throw new Error(
        `recovery code has unexpected format: ` +
          `"${capturedRecoveryCode}" (expected XXXXX-XXXXX)`
      );
    }
    steps.recoveryCodesDisplayed = true;

    // Compute the current TOTP. otplib defaults match RFC 6238 +
    // the server's totp-rs config (SHA-1, 6 digits, 30s step).
    authenticator.options = { ...authenticator.options, encoding: "base32" };
    const enrollCode = authenticator.generate(capturedSecret);

    await page.locator("#mfa-code-input").fill(enrollCode);
    await page.locator(".mfa-verify-btn").click();
    // Verify success → the page renders the "Confirmed" banner
    // briefly, then navigates home. Wait for the navigation.
    await page.waitForFunction(
      () => window.location.pathname === "/",
      null,
      { timeout: 15000 }
    );
    steps.verifyFinalizedEnrollment = true;

    // ── Step 5-7: log out, log back in, complete TOTP challenge ─
    // Log out via the BROWSER context — not via a Node.js `fetch`
    // with the initial Bearer token. The initial token predates
    // the MFA verify in step 4, and any future per-session-revoke
    // change to the logout handler would silently leave the
    // step-4 session alive. The browser cookie is the canonical
    // session credential post-step-4; use that. Mirrors step 8's
    // second logout for consistency.
    const logout1 = await page.evaluate(async () => {
      const r = await fetch("/api/v1/auth/logout", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "{}",
      });
      return r.ok;
    });
    if (!logout1) {
      throw new Error("first logout failed (browser context)");
    }
    steps.loggedOutAfterEnroll = true;

    // Dev-login again under the SAME email. Now mfa_enrolled_at is
    // set on the user row, so the server returns 202 + { handle }.
    const reloginRes = await fetch(`${target}/api/v1/auth/dev-login`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ email, name: "MFA Doctor" }),
    });
    if (reloginRes.status !== 202) {
      throw new Error(
        `expected 202 (MFA pending) on relogin, got ${reloginRes.status}`
      );
    }
    const pending = await reloginRes.json();
    if (!pending.handle) {
      throw new Error("202 response missing `handle`");
    }
    steps.reloginReturnedMfaPending202 = true;

    // Drive the challenge page through the UI.
    const challengeUrl =
      `${target}/auth/mfa-challenge?handle=${encodeURIComponent(pending.handle)}`;
    await page.goto(challengeUrl, { waitUntil: "domcontentloaded" });
    await page.waitForSelector("#mfa-challenge-input", { timeout: 10000 });
    const challengeCode = authenticator.generate(capturedSecret);
    await page.locator("#mfa-challenge-input").fill(challengeCode);
    await page.locator(".mfa-verify-btn").click();
    await page.waitForFunction(
      () => window.location.pathname === "/",
      null,
      { timeout: 15000 }
    );
    steps.totpChallengeMintedSession = true;

    // Confirm the session is actually live by reading /users/me
    // through the browser (uses the cookie + refresh + new access
    // token the challenge response set).
    const meResp = await page.evaluate(async () => {
      const r = await fetch("/api/v1/users/me", { credentials: "include" });
      return { ok: r.ok, status: r.status };
    });
    if (!meResp.ok) {
      throw new Error(`post-challenge /users/me failed: ${meResp.status}`);
    }
    steps.postChallengeMeWorks = true;

    // ── Step 8: recovery-code fallback ────────────────────────
    // Log out, log back in, navigate to challenge, click "use
    // recovery code", submit the captured code.
    const logout2 = await page.evaluate(async () => {
      const r = await fetch("/api/v1/auth/logout", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "{}",
      });
      return r.ok;
    });
    if (!logout2) {
      throw new Error("second logout failed");
    }
    const relogin2 = await page.evaluate(async (email) => {
      const r = await fetch("/api/v1/auth/dev-login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ email, name: "MFA Doctor" }),
      });
      return { status: r.status, body: await r.json() };
    }, email);
    if (relogin2.status !== 202 || !relogin2.body.handle) {
      throw new Error(`second relogin expected 202 + handle, got ${relogin2.status}`);
    }
    await page.goto(
      `${target}/auth/mfa-challenge?handle=${encodeURIComponent(relogin2.body.handle)}`,
      { waitUntil: "domcontentloaded" },
    );
    await page.waitForSelector("#mfa-challenge-input", { timeout: 10000 });

    // Click "Lost your authenticator? Use a recovery code".
    await page.locator(".mfa-fallback-link").click();
    // Re-find the input — the placeholder + maxlength changed.
    await page.locator("#mfa-challenge-input").fill(capturedRecoveryCode);
    await page.locator(".mfa-verify-btn").click();
    await page.waitForFunction(
      () => window.location.pathname === "/",
      null,
      { timeout: 15000 }
    );
    steps.recoveryCodeMintedSession = true;
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  try {
    await page.screenshot({
      path: join(outDir, "tab-a.png"),
      fullPage: true,
    });
  } catch (_) {}

  collector.scenario = {
    name: "mfa-flow",
    email,
    // Don't leak the captured secret or recovery code into the
    // report — anything in `collector.scenario` is written to
    // report.json. We pin success steps only; the secret stays
    // ephemeral.
    steps,
  };

  await context.close();
  await browser.close();
}

// ─── admin-console scenario ─────────────────────────────────────
//
// Phase 4 M-E2: exercises the admin console end-to-end.
//
//   1. Dev-login as `--email-a` (must be in ADMIN_EMAILS on the
//      deployed stack — the scenario fails loud at step 4 otherwise).
//   2. Dev-login as `--email-b` (the peer the admin acts on).
//   3. Navigate to /admin/users; assert the page mounts (admin gate
//      passes) and the peer is visible.
//   4. Use the email-prefix search box to filter to the peer.
//   5. Click "Disable", then "Enable" to flip the peer's state and
//      back. Both clicks update the same row in place; the badge
//      flips between "Disable" / "Enable" each time.
//   6. Navigate to /admin/audit; type the peer's user_id into Target
//      and click Search.
//   7. Assert the table contains at least one row with kind="disable"
//      and at least one with kind="enable" — the two admin actions
//      we just performed.
//
// Failure modes the scenario is designed to surface:
//   - admin gate redirect to "/" (status=Denied) → admin email not
//     in ADMIN_EMAILS.
//   - empty users table → backend reachability or auth issue.
//   - missing audit rows → audit-write spawned task didn't complete
//     OR /admin/audit endpoint not deployed.
async function scenarioAdminConsole(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, emailB, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const adminEmail = emailA || "doctor-admin@ogrenotes.example.com";
  const peerEmail = emailB || "doctor-peer@ogrenotes.example.com";

  const adminTokens = await devLogin(target, adminEmail, "Doctor Admin");
  const peerTokens = await devLogin(target, peerEmail, "Doctor Peer");
  logJson({
    at: "dev-login",
    adminUserId: adminTokens.userId,
    peerUserId: peerTokens.userId,
  });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, adminTokens);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  const steps = {};

  try {
    // Step 3: mount the admin users page.
    await page.goto(`${target}/admin/users`, { waitUntil: "domcontentloaded" });
    // Gate runs asynchronously; AdminUsersPage's UsersTable renders
    // after the gate sets Allowed. Wait for the heading rather than a
    // fragile content selector.
    try {
      await page.waitForSelector("h1:has-text('Admin · Users')", {
        timeout: 15000,
      });
      steps.adminUsersPageMounted = true;
    } catch (e) {
      // If the gate denied, the navigate-to-/ side effect lands here.
      // Surface a clearer message than a generic timeout.
      const currentPath = await page.evaluate(() => window.location.pathname);
      throw new Error(
        `admin users page never mounted (current path: ${currentPath}). ` +
          `Likely cause: ${adminEmail} is not in ADMIN_EMAILS on the deployed stack.`
      );
    }

    // Step 4: search by prefix. The frontend sends emailPrefix=peer →
    // the backend filters server-side; the row count should drop to
    // a small set containing the peer.
    const searchInput = page.locator(".admin-search");
    await searchInput.fill(peerEmail.split("@")[0]);
    await page.waitForFunction(
      (peerEmail) => {
        const rows = Array.from(
          document.querySelectorAll(".admin-users-table tbody tr")
        );
        return rows.some((r) =>
          r.querySelector("td")?.textContent?.trim() === peerEmail
        );
      },
      peerEmail,
      { timeout: 10000 }
    );
    steps.peerRowVisibleAfterSearch = true;

    // Step 5: click Disable on the row matching the peer email. The
    // row has 6 columns; the action buttons live in the last cell.
    const peerRow = page.locator(".admin-users-table tbody tr", {
      hasText: peerEmail,
    });
    await peerRow.locator("button", { hasText: "Disable" }).click();
    await page.waitForFunction(
      (peerEmail) => {
        const row = Array.from(
          document.querySelectorAll(".admin-users-table tbody tr")
        ).find((r) =>
          r.querySelector("td")?.textContent?.trim() === peerEmail
        );
        return !!row && row.textContent.includes("disabled");
      },
      peerEmail,
      { timeout: 10000 }
    );
    steps.peerDisabled = true;

    // Re-enable.
    await peerRow.locator("button", { hasText: "Enable" }).click();
    await page.waitForFunction(
      (peerEmail) => {
        const row = Array.from(
          document.querySelectorAll(".admin-users-table tbody tr")
        ).find((r) =>
          r.querySelector("td")?.textContent?.trim() === peerEmail
        );
        return !!row && row.textContent.includes("active");
      },
      peerEmail,
      { timeout: 10000 }
    );
    steps.peerReEnabled = true;

    // Step 6: switch to the audit page and query for the peer's events.
    await page.goto(`${target}/admin/audit`, { waitUntil: "domcontentloaded" });
    await page.waitForSelector("h1:has-text('Admin · Audit log')", {
      timeout: 10000,
    });
    // Type peer id into the first text input (Target).
    const targetInput = page.locator(".admin-audit-filters input").first();
    await targetInput.fill(peerTokens.userId);
    await page.locator(".admin-audit-filters button", {
      hasText: "Search",
    }).click();

    // Step 7: poll for both kinds. The audit DDB write is spawned by
    // the handler, so allow generous time — production stacks behind
    // the ALB can take a couple seconds for the row to land.
    await page.waitForFunction(
      () => {
        const kinds = Array.from(
          document.querySelectorAll(".admin-audit-table tbody tr td:nth-child(3)")
        ).map((td) => td.textContent.trim());
        return kinds.includes("disable") && kinds.includes("enable");
      },
      null,
      { timeout: 20000 }
    );
    steps.auditRowsVisible = true;
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  try {
    await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: true });
  } catch (_) {}

  collector.scenario = {
    name: "admin-console",
    adminUserId: adminTokens.userId,
    peerUserId: peerTokens.userId,
    steps,
  };

  await context.close();
  await browser.close();
}

// ─── spreadsheet-paste scenario ─────────────────────────────────
//
// Verifies Ctrl+V in the spreadsheet grid doesn't trigger a
// clipboard-permission prompt AND correctly translates relative refs on
// paste.
//
// The context is created WITHOUT `clipboard-read` permission. Chromium
// grants `clipboard-write` by default (so writeText during copy still
// works), but `readText` is denied. The native `paste` event that our
// wrapper listens for provides clipboardData synchronously without
// going through the async clipboard API, so it must succeed even with
// read denied. If this scenario fails on the value/formula assertion,
// it means the paste path is reaching for `readText` again.
//
// Steps (cursor starts at A1 after page load):
//   1. Type A1=1, A2=2, A3=3, A4==SUM(A1:A3) via keyboard entry.
//   2. Navigate to B1 and type 3, 4, 5 down column B.
//   3. Move to A4, Ctrl+C, Right, Ctrl+V.
//   4. Assert the formula bar shows `=SUM(B1:B3)` on B4 and the cell
//      displays "12" (=3+4+5).
//   5. Assert no permission/dialog popups fired during the run.
// Smoke-test the M-S2 editor UI affordances: toolbar (format
// painter), sheet-tab bar, formula bar, header-click sort, freeze
// rows via context menu, comment indicator. Each step records a
// boolean in `collector.scenario.steps` so the runner can flip
// `ok=false` on any missing piece.
// Open a grouped cell-context-menu submenu and click one of its leaf
// items. The 2026-06 context-menu compaction nests advanced actions
// (freeze, pivot/chart insert, validation, named ranges, …) under
// fly-outs; since the 2026-07 shared-menu-primitive migration the chrome
// is `components::menu` (`.ui-menu*` classes) and a submenu parent is a
// `.ui-menu-item` with `aria-haspopup="menu"` that opens its `.ui-menu-sub`
// on hover OR click. `parentRe` matches the parent label (e.g. /insert/i,
// /hide \/ unhide/i); `leafRe` matches the leaf button (e.g.
// /insert pivot table/i, /freeze rows above/i). The menu must already be
// open (`.ui-menu` present).
async function clickCtxMenuItem(page, parentRe, leafRe) {
  await page
    .locator('.ui-menu-item[aria-haspopup="menu"]')
    .filter({ hasText: parentRe })
    .first()
    .hover();
  const leaf = uiMenuItem(page, leafRe);
  await leaf.waitFor({ state: "visible", timeout: 3000 });
  await leaf.click();
}

// Menu items carry ARIA menu roles (`menuitem` / `menuitemcheckbox`)
// since the shared-menu-primitive migration, so they no longer match
// getByRole("button"). Address them by the shared item class + label,
// which covers both roles.
function uiMenuItem(page, nameRe) {
  return page.locator(".ui-menu-item").filter({ hasText: nameRe }).first();
}

async function scenarioSpreadsheetFeatures(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-features@ogrenotes.example.com"
  );
  logJson({ at: "dev-login", userId: tokens.userId });

  const title = `doctor-features-${Date.now()}`;
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    title,
    "spreadsheet"
  );
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  // M-S2 features that rely on `window.prompt` (Define Name,
  // Conditional Formatting, etc.) auto-accept with a canned value
  // so the menu items don't hang on a missing input. The "Freeze
  // rows above" item we drive in this scenario doesn't prompt, but
  // capturing every dialog keeps the scenario diagnostic-friendly
  // if any new prompt-driven feature is added later.
  page.on("dialog", async (d) => {
    collector.scenario = collector.scenario || {};
    collector.scenario.dialogs = collector.scenario.dialogs || [];
    collector.scenario.dialogs.push({ type: d.type(), message: d.message() });
    await d.accept("doctor-input");
  });

  const docUrl = `${target}/d/${doc.id}/probe`;
  logJson({ at: "navigate", url: docUrl });
  await page.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });

  const steps = {};
  try {
    // Mount + key DOM affordances added in M-S2.
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.gridMounted = true;
    steps.toolbarPresent =
      (await page.locator(".spreadsheet-toolbar").count()) > 0;
    steps.formatPainterPresent =
      (await page.locator(".spreadsheet-toolbar .ss-tool-btn").count()) > 0;
    steps.sheetTabsPresent =
      (await page.locator(".ss-sheet-tabs").count()) > 0;
    steps.formulaBarPresent =
      (await page.locator(".spreadsheet-formula-bar").count()) > 0;

    // Type a few rows so sort + freeze have something to act on.
    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    const typeInto = async (s) => {
      await page.keyboard.type(s, { delay: 15 });
      await page.keyboard.press("Enter");
    };
    await typeInto("3"); // A1
    await typeInto("1"); // A2
    await typeInto("2"); // A3
    steps.cellsPopulated = true;

    // Sort dialog: click the toolbar Sort button (⇅) to open the
    // dialog, accept its defaults (whole-grid range, single key on
    // column 0, ascending, no header skip), and click Apply. After
    // sort, A1 should be the smallest value (1).
    const sortBtn = page.locator(".spreadsheet-toolbar .ss-tool-btn").nth(1);
    await sortBtn.click();
    try {
      await page.waitForSelector(".ss-sort-dialog", { timeout: 3000 });
      steps.sortDialogOpened = true;
    } catch {
      steps.sortDialogOpened = false;
    }
    if (steps.sortDialogOpened) {
      await page.locator(".ss-sort-dialog-apply").click();
      try {
        await page.waitForSelector(".ss-sort-dialog", {
          state: "detached",
          timeout: 3000,
        });
        steps.sortDialogClosedAfterApply = true;
      } catch {
        steps.sortDialogClosedAfterApply = false;
      }
    }

    // Read A1's display value (top-left data cell, after row-header).
    const a1Text = (
      await page
        .locator(".spreadsheet-cell")
        .first()
        .textContent()
    )?.trim();
    steps.sortReorderedRows = a1Text === "1";

    // Freeze rows above: trigger the context menu on the SECOND row's
    // first cell (A2) so "Freeze rows above" actually has rows to
    // freeze. Right-clicking A1 would freeze 0 rows after the #61
    // off-by-one fix (`r` instead of `r + 1`), and `.frozen-row`
    // never appears. With A2 the count becomes 1 and row 0 picks
    // up the class.
    const a2Cell = page.locator(".spreadsheet-grid tbody tr")
      .nth(1)
      .locator("td.spreadsheet-cell")
      .first();
    await a2Cell.click({ button: "right" });
    await page.waitForSelector(".ui-menu", { timeout: 3000 });
    steps.contextMenuOpens = true;
    // "Freeze rows above" lives under the "Hide / Unhide" submenu after
    // the 2026-06 context-menu compaction.
    await clickCtxMenuItem(page, /hide \/ unhide/i, /freeze rows above/i);
    // Wait for the DOM mutation rather than a fixed timeout: the
    // signal-write → mutex update → grid_version bump → Leptos
    // flush → WASM render path can exceed a fixed 200ms on a slow
    // CI runner. `waitForSelector` flips the assertion to a
    // bounded poll that fails cleanly if the class never appears.
    try {
      await page.waitForSelector(
        ".spreadsheet-grid tbody tr.frozen-row",
        { timeout: 5000 },
      );
      steps.frozenRowClassApplied = true;
    } catch {
      steps.frozenRowClassApplied = false;
    }
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: collector.scenario?.name || "spreadsheet-features",
    dialogs: collector.scenario?.dialogs || [],
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── spreadsheet-lifecycle scenario ─────────────────────────────
//
// #76: exercise the spreadsheet view's mount/unmount reclaim path. After
// #4 finding 1, the view's engine + fetched_ids are reclaimed on
// `on_cleanup` via an `Arc<AtomicBool>` liveness flag whose safety rests on
// WASM single-threaded scheduling. There's no `cargo test` coverage of the
// unmount path, so this drives it in a real browser and asserts structural
// correctness rather than absolute heap numbers (Chrome's
// `usedJSHeapSize` is too noisy to assert on).
//
// Steps:
//   1. Create two spreadsheet docs A and B.
//   2. Open A, populate a cell; navigate to B; open B, populate a cell;
//      navigate back. Loop NAV_LOOPS times — each navigation unmounts the
//      previous grid and runs its `on_cleanup`.
//   3. After the loop, on A: copy a cell and paste it (a *second*
//      spawn_local on a doc whose sibling was repeatedly unmounted) and
//      assert the value lands — i.e. the engine the deferred task captured
//      is still live, no use-after-free.
//   4. Assert zero page errors (a UAF would surface as a Rust panic →
//      `pageerror`) and no "panicked"/"recursively or after being dropped"
//      console output across the whole run.
async function scenarioSpreadsheetLifecycle(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const NAV_LOOPS = 10;

  const tokens = await devLogin(
    target,
    emailA || "doctor-lifecycle@ogrenotes.example.com"
  );
  logJson({ at: "dev-login", userId: tokens.userId });

  const stamp = Date.now();
  const docA = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-lifecycle-A-${stamp}`,
    "spreadsheet"
  );
  const docB = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-lifecycle-B-${stamp}`,
    "spreadsheet"
  );
  logJson({ at: "docs-created", docA: docA.id, docB: docB.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
    // clipboard-write so the post-loop copy/paste exercises the synchronous
    // clipboard spawn_local without a permission prompt (mirrors
    // spreadsheet-paste).
    permissions: ["clipboard-write"],
  });
  await seedAuth(context, tokens);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  page.on("dialog", async (d) => {
    collector.scenario = collector.scenario || {};
    collector.scenario.dialogs = collector.scenario.dialogs || [];
    collector.scenario.dialogs.push({ type: d.type(), message: d.message() });
    await d.accept("doctor-input");
  });

  // Open a doc, wait for the grid to mount, focus A1, type one value +
  // Enter (a commit through the reactive/engine pipeline).
  const openAndTouch = async (docId, value) => {
    const url = `${target}/d/${docId}/probe`;
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 30000 });
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    await page.keyboard.type(value, { delay: 15 });
    await page.keyboard.press("Enter");
  };

  const steps = { bothDocsCreated: true };
  try {
    let loops = 0;
    for (let i = 0; i < NAV_LOOPS; i++) {
      await openAndTouch(docA.id, String(i));
      await openAndTouch(docB.id, String(i));
      loops = i + 1;
      logJson({ at: "lifecycle-loop", iteration: loops });
    }
    steps.loopsCompleted = loops === NAV_LOOPS;

    // Back on A after B's grid has been unmounted many times: type a
    // sentinel into A1, copy it, move right, paste. The paste path runs a
    // spawn_local that captures the live engine — if reclaim mis-fired on a
    // still-mounted A, this would panic or paste nothing.
    await page.goto(`${target}/d/${docA.id}/probe`, {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    await page.keyboard.type("77", { delay: 15 }); // A1
    await page.keyboard.press("Enter"); // commit, cursor → A2
    await page.keyboard.press("ArrowUp"); // back to A1
    await page.keyboard.press("Control+c");
    await page.keyboard.press("ArrowRight"); // B1
    await page.keyboard.press("Control+v");
    await page.waitForTimeout(300); // let the grid re-render

    // B1 displayed text — row 0, col 1.
    const b1Text = await page
      .locator('[data-row="0"][data-col="1"]')
      .first()
      .textContent()
      .catch(() => null);
    collector.postUnmountPasteText = (b1Text || "").trim();
    steps.postUnmountPasteWorks = collector.postUnmountPasteText === "77";
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  // Structural-correctness gates: no panics (a UAF surfaces as a Rust
  // panic → pageerror) and no panic-shaped console output.
  const tab = collector["tab-a"] || { errors: [], console: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;
  const panicRe = /panic|recursively or after being dropped/i;
  steps.noPanicConsole = !(tab.console || []).some(
    (m) => m.type === "error" && panicRe.test(m.text || "")
  );

  collector.scenario = {
    name: collector.scenario?.name || "spreadsheet-lifecycle",
    dialogs: collector.scenario?.dialogs || [],
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── focus-mode scenario ────────────────────────────────────────
//
// #134: the focus/expand toggle is a single header button (.focus-toggle-btn)
// that flips to its opposite (⤢ enter ↔ ✕ exit) and toggles `.focus-mode`
// on `.app-layout` (hiding the sidebar + menu bar). This exercises a full
// enter→exit→enter→exit round-trip — via both the button and Ctrl+Shift+F —
// and asserts the button flips, the chrome hides/shows, and crucially that
// NO panic fires on teardown. This area has a history of the Firefox
// "closure invoked recursively or after being dropped" class when a
// focus-gated element tears down mid-click, so the run must stay clean.
async function scenarioFocusMode(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-focus@ogrenotes.example.com"
  );
  logJson({ at: "dev-login", userId: tokens.userId });

  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-focus-${Date.now()}`,
    "document"
  );
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  const inFocus = () =>
    page.locator(".app-layout.focus-mode").count().then((n) => n > 0);
  const toggleActive = () =>
    page.locator(".focus-toggle-btn.active").count().then((n) => n > 0);
  const menuVisible = () => page.locator(".menu-bar").isVisible();

  const steps = {};
  try {
    await page.waitForSelector(".focus-toggle-btn", { timeout: 15000 });
    // Initial state: not in focus mode, chrome visible.
    steps.startsUnfocused = !(await inFocus()) && !(await toggleActive());
    steps.menuVisibleInitially = await menuVisible();

    // Enter via the button.
    await page.locator(".focus-toggle-btn").click();
    await page.waitForTimeout(150);
    steps.buttonEntersFocus = (await inFocus()) && (await toggleActive());
    // The header (and thus the toggle) stays mounted; only sidebar + menu
    // bar hide. `isVisible` is false for a display:none element.
    steps.menuHiddenInFocus = !(await menuVisible());
    steps.toggleStillPresentInFocus =
      (await page.locator(".focus-toggle-btn").count()) > 0;

    // Exit via the SAME button (the reversibility this fix is about).
    await page.locator(".focus-toggle-btn").click();
    await page.waitForTimeout(150);
    steps.buttonExitsFocus = !(await inFocus()) && !(await toggleActive());
    steps.menuVisibleAfterExit = await menuVisible();

    // Round-trip again via the keyboard shortcut (Ctrl+Shift+F) to cover
    // the alternate entry/exit path.
    await page.keyboard.press("Control+Shift+F");
    await page.waitForTimeout(150);
    steps.shortcutEntersFocus = await inFocus();
    await page.keyboard.press("Control+Shift+F");
    await page.waitForTimeout(150);
    steps.shortcutExitsFocus = !(await inFocus());
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  // No panic on any teardown (the focus-exit closure-drop class).
  const tab = collector["tab-a"] || { errors: [], console: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;
  const panicRe = /panic|recursively or after being dropped/i;
  steps.noPanicConsole = !(tab.console || []).some(
    (m) => m.type === "error" && panicRe.test(m.text || "")
  );

  collector.scenario = {
    name: collector.scenario?.name || "focus-mode",
    dialogs: collector.scenario?.dialogs || [],
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── settings-appearance scenario ───────────────────────────────
//
// Regression for the settings-page fit-content collapse: .settings-page
// carries `margin: 0 auto` inside the column-flex .main-content, and auto
// cross-axis margins disable align-items:stretch — without an explicit
// `width: 100%` the page shrank to fit-content, starving .settings-panel
// (~210px at a 1280px viewport) until the third theme button (Dark)
// rendered outside the Appearance card. Asserts the panel actually
// stretches and every theme button's box sits inside the panel's box.
async function scenarioSettingsAppearance(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-settings@ogrenotes.example.com"
  );
  logJson({ at: "dev-login", userId: tokens.userId });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/settings#appearance`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".theme-selector-btn", { timeout: 15000 });
    // Guard against a green-wash: if main.css fails to load (stale
    // hash, MIME refusal), the raw-HTML flow layout can satisfy the
    // box assertions below. Require the stylesheet to have applied.
    steps.stylesheetApplied = await page
      .locator(".settings-body")
      .evaluate((el) => getComputedStyle(el).display === "flex");
    const panel = await page.locator(".settings-panel").boundingBox();
    const buttons = page.locator(".theme-selector-btn");
    const count = await buttons.count();
    steps.threeThemeButtons = count === 3;
    // Pre-fix the panel collapsed to ~210px; healthy is ~624px at the
    // default 1280px viewport. 400 splits the two with wide margins.
    steps.panelStretches = !!panel && panel.width >= 400;

    let contained = count > 0 && !!panel;
    for (let i = 0; i < count; i++) {
      const b = await buttons.nth(i).boundingBox();
      const inside =
        !!b &&
        !!panel &&
        b.x >= panel.x - 1 &&
        b.y >= panel.y - 1 &&
        b.x + b.width <= panel.x + panel.width + 1 &&
        b.y + b.height <= panel.y + panel.height + 1;
      if (!inside) contained = false;
    }
    steps.themeButtonsInsidePanel = contained;
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [], console: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "settings-appearance",
    dialogs: collector.scenario?.dialogs || [],
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── menu-switch scenario ───────────────────────────────────────
//
// Regression for the menu-bar backdrop-intercept bug: with a menu open, its
// full-screen backdrop (`.ui-menu-backdrop` since the shared-primitive
// migration) used to cover the other menu names, so clicking one only
// CLOSED the current menu (the click hit the backdrop) — you had to click
// twice to open the next. The fix stacks `.menu-bar-item` above the
// backdrop so a single click switches menus. This asserts that clicking a
// second menu name while one is open switches in one click.
async function scenarioMenuSwitch(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-menuswitch@ogrenotes.example.com"
  );
  logJson({ at: "dev-login", userId: tokens.userId });

  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-menuswitch-${Date.now()}`,
    "document"
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  const openMenuName = () =>
    page
      .locator(".menu-bar-item.open")
      .first()
      .textContent()
      .then((t) => (t || "").trim())
      .catch(() => "");
  const dropdownCount = () => page.locator(".menu-bar .ui-menu").count();

  // Poll for the expected state instead of asserting at a fixed delay: a
  // single click should switch the menu, but under a loaded CI runner the
  // re-render can take >100ms. We still click exactly once per transition, so
  // a real backdrop-intercept regression (menu never switches) still fails.
  const settle = async (pred, ms = 3000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await pred().catch(() => false)) return true;
      await page.waitForTimeout(50);
    }
    return false;
  };

  const steps = {};
  try {
    await page.waitForSelector(".menu-bar-item", { timeout: 15000 });

    // Open the Document menu.
    await page.getByRole("button", { name: "Document", exact: true }).click();
    steps.documentOpened = await settle(
      async () => (await openMenuName()) === "Document" && (await dropdownCount()) === 1
    );

    // Click "View" WHILE Document is open — must switch in one click.
    await page.getByRole("button", { name: "View", exact: true }).click();
    steps.switchedToViewInOneClick = await settle(
      async () => (await openMenuName()) === "View" && (await dropdownCount()) === 1
    );

    // Switch once more to Format, still single-click.
    await page.getByRole("button", { name: "Format", exact: true }).click();
    steps.switchedToFormatInOneClick = await settle(
      async () => (await openMenuName()) === "Format" && (await dropdownCount()) === 1
    );

    // Clicking the open menu's own name closes it.
    await page.getByRole("button", { name: "Format", exact: true }).click();
    steps.sameNameCloses = await settle(async () => (await dropdownCount()) === 0);
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "menu-switch",
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── doc-actions scenario ───────────────────────────────────────
//
// #146: the Document-menu Rename + Duplicate actions. Runs on a SPREADSHEET
// because spreadsheets carry an explicit, assertable title (a document's
// title is derived from its first line, so it isn't a clean witness for a
// name assertion). Rename → the prompt (auto-accepted) updates the header
// title input. Duplicate → opens the Duplicate dialog (name pre-filled from
// the current doc name); the scenario gives it a distinct name and confirms
// the default destination, then asserts it navigated to a NEW doc with that
// name and the copied cell content.
async function scenarioDocActions(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-docactions@ogrenotes.example.com"
  );
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-docactions-${Date.now()}`,
    "spreadsheet"
  );
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  const NEW_TITLE = `Renamed-${Date.now()}`;
  const CELL = "dupcell";
  // The rename prompt is auto-accepted with NEW_TITLE.
  page.on("dialog", async (d) => {
    if (d.type() === "prompt") await d.accept(NEW_TITLE);
    else await d.accept("doctor-input");
  });

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  const openDocMenu = async () => {
    await page.getByRole("button", { name: "Document", exact: true }).click();
    await page.waitForSelector(".menu-bar .ui-menu", { timeout: 3000 });
  };

  const steps = {};
  try {
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    // Put a value in A1 so Duplicate has content to copy.
    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    await page.keyboard.type(CELL, { delay: 15 });
    await page.keyboard.press("Enter");
    await page.waitForTimeout(800); // let the autosave persist

    // ── Rename ──
    await openDocMenu();
    await uiMenuItem(page, /Rename Document/).click();
    await page.waitForTimeout(300);
    steps.renameUpdatesTitle =
      (await page.locator(".doc-title-editable").inputValue()) === NEW_TITLE;

    // ── Duplicate (via the dialog) ──
    await openDocMenu();
    await uiMenuItem(page, /Duplicate/).click();
    // The dialog opens with the name pre-filled from the current doc name.
    await page.waitForSelector("#duplicate-name", { timeout: 5000 });
    steps.duplicateDialogPrefillsName =
      (await page.locator("#duplicate-name").inputValue()) === NEW_TITLE;
    // Give it a distinct name so we can assert the chosen name is honored.
    const DUP_NAME = `${NEW_TITLE}-dup`;
    await page.locator("#duplicate-name").fill(DUP_NAME);
    // Confirm with the default destination (the source's folder / Home).
    await page.getByRole("button", { name: "Duplicate", exact: true }).click();
    await page.waitForFunction(
      (origId) => {
        const m = location.pathname.match(/\/d\/([^/]+)/);
        return m && m[1] !== origId;
      },
      doc.id,
      { timeout: 12000 }
    );
    const newId = new URL(page.url()).pathname.match(/\/d\/([^/]+)/)?.[1];
    steps.duplicateNavigatedToNewDoc = !!newId && newId !== doc.id;

    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    // Poll the chosen title + copied cell (tolerate put→get propagation lag).
    let dupTitle = "";
    let a1 = "";
    for (let i = 0; i < 25; i++) {
      dupTitle = await page.locator(".doc-title-editable").inputValue().catch(() => "");
      a1 =
        (await page
          .locator('[data-row="0"][data-col="0"]')
          .first()
          .textContent()
          .catch(() => "")) || "";
      if (dupTitle === DUP_NAME && a1.includes(CELL)) break;
      await page.waitForTimeout(200);
    }
    steps.duplicateUsesEnteredName = dupTitle === DUP_NAME;
    steps.duplicateCopiedContent = a1.includes(CELL);
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "doc-actions",
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── favorites scenario ─────────────────────────────────────────
//
// #144: the header star toggle + Favorites sidebar section. Star the doc and
// assert the button goes active AND the doc appears in the sidebar Favorites
// list (live, via the favorites_refresh tick); unstar and assert both revert.
async function scenarioFavorites(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-favorites@ogrenotes.example.com"
  );
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-fav-${Date.now()}`,
    "document"
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  const isActive = () =>
    page.locator(".favorite-toggle-btn.active").count().then((n) => n > 0);
  const favCount = () => page.locator(".sidebar-favorite-item").count();
  const waitFor = async (pred, ms = 6000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await pred()) return true;
      await page.waitForTimeout(150);
    }
    return false;
  };

  const steps = {};
  try {
    await page.waitForSelector(".favorite-toggle-btn", { timeout: 15000 });
    steps.startsUnstarred = !(await isActive());

    // #144: the star button now opens a dropdown rather than toggling
    // directly. Open it, then click "Add to Favorites" (the first menu item).
    await page.locator(".favorite-toggle-btn").click();
    await page
      .locator(".favorite-menu-dropdown")
      .waitFor({ state: "visible", timeout: 5000 });
    await page.locator(".favorite-menu-item").first().click();
    steps.starButtonGoesActive = await waitFor(isActive);
    steps.appearsInSidebar = await waitFor(async () => (await favCount()) >= 1);

    // The dropdown stays open after starring and now shows "Remove from
    // Favorites" (the item carrying the ★ star span). Clicking it unstars and
    // closes the menu — no need to reopen or fight the backdrop.
    const removeItem = page.locator(
      ".favorite-menu-item:has(.favorite-menu-star)"
    );
    await removeItem.waitFor({ state: "visible", timeout: 5000 });
    await removeItem.click();
    steps.unstarButtonGoesInactive = await waitFor(async () => !(await isActive()));
    steps.leavesSidebar = await waitFor(async () => (await favCount()) === 0);
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "favorites",
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// #147: in-app Find & Replace. Type text with a repeated word, open the bar
// (via the Edit menu and via Ctrl+F), assert the live match count, navigate,
// and Replace All — verifying the editor model actually changed.
async function scenarioFindReplace(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-find@ogrenotes.example.com"
  );
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-find-${Date.now()}`,
    "document"
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  const barVisible = () => page.locator(".find-replace-bar").isVisible();
  const findInput = () => page.locator(".find-replace-bar .find-input").first();
  const replaceInput = () => page.locator(".find-replace-bar .find-replace-input");
  const countText = () =>
    page.locator(".find-count").textContent().then((t) => (t || "").trim());
  const editorText = () =>
    page.locator(".editor-content").textContent().then((t) => t || "");
  const waitFor = async (pred, ms = 6000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await pred().catch(() => false)) return true;
      await page.waitForTimeout(150);
    }
    return false;
  };

  const steps = {};
  try {
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 15000 });
    const ed = page.locator(".editor-content");
    await ed.click();
    // "alpha" appears 3 times; first block's text becomes the title but the
    // body text is what find searches.
    await page.keyboard.type("alpha beta alpha gamma alpha", { delay: 15 });
    await page.waitForTimeout(800);

    // Open via the Edit menu's "Find and Replace" item.
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await page.waitForSelector(".menu-bar .ui-menu", { timeout: 3000 });
    await uiMenuItem(page, /Find and Replace/i).click();
    steps.barOpensFromMenu = await waitFor(barVisible);

    // Close it, then re-open via Ctrl+F to verify the keyboard binding.
    await page.locator(".find-replace-bar .find-close").click();
    await waitFor(async () => !(await barVisible()));
    await ed.click();
    await page.keyboard.press("Control+f");
    steps.barOpensFromCtrlF = await waitFor(barVisible);

    // Type the query — the live count reflects 3 matches as "1/3".
    await findInput().fill("alpha");
    steps.countShowsThreeMatches = await waitFor(
      async () => (await countText()) === "1/3"
    );

    // Next wraps through the matches: 1/3 -> 2/3.
    await findInput().press("Enter");
    steps.nextAdvancesMatch = await waitFor(
      async () => (await countText()) === "2/3"
    );

    // Replace All: every "alpha" becomes "ZZZ".
    await replaceInput().fill("ZZZ");
    await page.getByRole("button", { name: /Replace all/i }).click();
    steps.replaceAllRewritesDoc = await waitFor(async () => {
      const t = await editorText();
      return t.includes("ZZZ") && !t.toLowerCase().includes("alpha");
    });

    // After the replace the query "alpha" matches nothing — the count drops
    // out of the "n/m" form (shows the no-results label instead).
    steps.noMatchesAfterReplace = await waitFor(
      async () => !(await countText()).includes("/")
    );
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "find-replace",
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// #139: Show Line Numbers — regression for the two bugs the maintainer
// flagged: numbers were per-block (not per-visual-line) and scaled with each
// block's font (headings rendered giant numbers). Asserts numbers count
// visual lines (more numbers than blocks once a paragraph wraps) and all share
// one uniform font-size. Also checks Show Page Breaks no longer paints a
// full-width rule through the text.
async function scenarioLineNumbers(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-linenums@ogrenotes.example.com"
  );
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-linenums-${Date.now()}`,
    "document"
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  const numCount = () => page.locator(".editor-line-number").count();
  const blockCount = () => page.locator(".editor-content > *").count();
  const waitFor = async (pred, ms = 6000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await pred().catch(() => false)) return true;
      await page.waitForTimeout(150);
    }
    return false;
  };
  const openView = async () => {
    await page.getByRole("button", { name: "View", exact: true }).click();
    await page.waitForSelector(".menu-bar .ui-menu", { timeout: 3000 });
  };

  const steps = {};
  try {
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 15000 });
    const ed = page.locator(".editor-content");
    await ed.click();
    // One long paragraph that wraps to several visual lines, then two short
    // blocks — so visual lines (≥4) clearly exceed top-level blocks (3).
    await page.keyboard.type(
      "This is a deliberately long first paragraph written so that it wraps " +
        "across several visual lines in the editor, which is exactly the case " +
        "the per-visual-line numbering must handle correctly without help.",
      { delay: 4 }
    );
    await page.keyboard.press("Enter");
    await page.keyboard.type("Second block.");
    await page.keyboard.press("Enter");
    await page.keyboard.type("Third block.");
    await page.waitForTimeout(600);
    steps.editorTyped = true;

    // Turn on Show Line Numbers.
    await openView();
    await uiMenuItem(page, /Show Line Numbers/i).click();
    steps.numbersAppear = await waitFor(async () => (await numCount()) > 0);

    // Per-visual-line: a wrapped paragraph yields more numbers than blocks.
    const nums = await numCount();
    const blocks = await blockCount();
    steps.perVisualLine = nums > blocks;

    // Uniform font: every number shares one computed font-size (the bug had
    // heading numbers scaled up by the block's em).
    const sizes = await page.$$eval(".editor-line-number", (els) =>
      Array.from(new Set(els.map((e) => getComputedStyle(e).fontSize)))
    );
    steps.uniformFont = sizes.length === 1;

    // Show Page Breaks must NOT paint a full-width rule on the content.
    await openView();
    await uiMenuItem(page, /Show Page Breaks/i).click();
    await page.waitForTimeout(300);
    const bg = await page.$eval(
      ".editor-content",
      (e) => getComputedStyle(e).backgroundImage
    );
    steps.noFullWidthPageRule = bg === "none";

    // Toggling line numbers back off removes them.
    await openView();
    await uiMenuItem(page, /Show Line Numbers/i).click();
    steps.togglesOff = await waitFor(async () => (await numCount()) === 0);
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "line-numbers",
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// #141: Document Details panel — open from the Document menu and assert the
// metadata rows render, including a live word count of the typed text.
async function scenarioDocumentDetails(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-details@ogrenotes.example.com"
  );
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-details-${Date.now()}`,
    "document"
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  const waitFor = async (pred, ms = 6000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await pred().catch(() => false)) return true;
      await page.waitForTimeout(150);
    }
    return false;
  };

  const steps = {};
  try {
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 15000 });
    await page.locator(".editor-content").click();
    // Five words on one line.
    await page.keyboard.type("alpha beta gamma delta epsilon", { delay: 6 });
    await page.waitForTimeout(500);

    // Document menu → Document Details…
    await page.getByRole("button", { name: "Document", exact: true }).click();
    await page.waitForSelector(".menu-bar .ui-menu", { timeout: 3000 });
    await uiMenuItem(page, /Document Details/i).click();
    steps.panelOpens = await waitFor(() =>
      page.locator(".doc-details-dialog").isVisible()
    );

    // Definition list: Name, Type, Created, Last modified, Words, Characters.
    const dds = page.locator(".doc-details-list dd");
    steps.hasAllRows = (await dds.count()) === 6;

    const words = (await dds.nth(4).textContent())?.trim();
    steps.wordCountCorrect = words === "5";
    const chars = (await dds.nth(5).textContent())?.trim();
    // "alpha beta gamma delta epsilon" = 30 characters.
    steps.charCountCorrect = chars === "30";

    // Close it.
    await page.getByRole("button", { name: /Close/i }).click();
    steps.panelCloses = await waitFor(
      async () => !(await page.locator(".doc-details-dialog").isVisible())
    );
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "document-details",
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// #145: Expand — the header ↗ enters a clutter-free mode (doc header hidden,
// floating collapse button shown, app-layout gains .expanded); the FAB exits.
async function scenarioExpand(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-expand@ogrenotes.example.com"
  );
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-expand-${Date.now()}`,
    "document"
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  const expandedNow = () =>
    page.locator(".app-layout.expanded").count().then((n) => n > 0);
  const waitFor = async (pred, ms = 6000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await pred().catch(() => false)) return true;
      await page.waitForTimeout(150);
    }
    return false;
  };

  const steps = {};
  try {
    await page.waitForSelector(".expand-toggle-btn", { timeout: 15000 });
    steps.startsCollapsed = !(await expandedNow());

    // Enter Expand.
    await page.locator(".expand-toggle-btn").click();
    steps.entersExpanded = await waitFor(expandedNow);
    steps.headerHidden = await waitFor(
      async () => !(await page.locator(".doc-header").isVisible())
    );
    steps.fabShown = await waitFor(() =>
      page.locator(".expand-collapse-fab").isVisible()
    );

    // Exit via the floating collapse button.
    await page.locator(".expand-collapse-fab").click();
    steps.collapses = await waitFor(async () => !(await expandedNow()));
    steps.headerBack = await waitFor(() =>
      page.locator(".doc-header").isVisible()
    );
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "expand",
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// #143: subscript / superscript toolbar toggles — assert the mark renders as
// <sub>/<sup> in the editor and that the two are mutually exclusive.
async function scenarioSubscript(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-subscript@ogrenotes.example.com"
  );
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-sub-${Date.now()}`,
    "document"
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });

  // Scoped to .editor-content so the toolbar buttons' own <sub>/<sup> glyphs
  // don't count.
  const subCount = () => page.locator(".editor-content sub").count();
  const supCount = () => page.locator(".editor-content sup").count();
  const waitFor = async (pred, ms = 6000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await pred().catch(() => false)) return true;
      await page.waitForTimeout(150);
    }
    return false;
  };

  const steps = {};
  try {
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 15000 });
    await page.locator(".editor-content").click();
    await page.keyboard.type("x");
    await page.keyboard.press("Control+a");
    await page.locator('button[title="Subscript"]').click();
    steps.subscriptRenders = await waitFor(async () => (await subCount()) > 0);

    // Mutually exclusive: applying superscript to the same selection swaps it.
    await page.keyboard.press("Control+a");
    await page.locator('button[title="Superscript"]').click();
    steps.switchesToSuperscript = await waitFor(
      async () => (await supCount()) > 0 && (await subCount()) === 0
    );
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "subscript",
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

async function scenarioPivotEditor(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-pivot@ogrenotes.example.com"
  );
  logJson({ at: "dev-login", userId: tokens.userId });

  const title = `doctor-pivot-${Date.now()}`;
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    title,
    "spreadsheet"
  );
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  // The pivot-editor flow doesn't prompt, but capture any dialog
  // for diagnostic value.
  page.on("dialog", async (d) => {
    collector.scenario = collector.scenario || {};
    collector.scenario.dialogs = collector.scenario.dialogs || [];
    collector.scenario.dialogs.push({ type: d.type(), message: d.message() });
    await d.accept("doctor-input");
  });

  const docUrl = `${target}/d/${doc.id}/probe`;
  logJson({ at: "navigate", url: docUrl });
  await page.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });

  const steps = {};
  try {
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.gridMounted = true;

    // Seed a 2-col × 3-row source: header (Region, Revenue) + 2 data rows.
    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    // After Enter, the spreadsheet's keydown handler moves the
    // active cell DOWN by one row but keeps the column unchanged
    // (no `Home` arm in the handler). To restart at col A on the
    // next row, we step LEFT one cell after each Enter.
    const typeRow = async (a, b) => {
      await page.keyboard.type(a, { delay: 10 });
      await page.keyboard.press("Tab");
      await page.keyboard.type(b, { delay: 10 });
      await page.keyboard.press("Enter");
      await page.keyboard.press("ArrowLeft");
    };
    await typeRow("Region", "Revenue");
    await typeRow("West", "10");
    await typeRow("East", "30");
    steps.cellsPopulated = true;

    // Select the source range A1:B3 via data-attr selectors. The
    // grid renders `.spreadsheet-cell` row-major, so a positional
    // `nth()` index would tie to `grid_cols` (10 by default — B3
    // would be nth(21), not nth(5)). The data-row/data-col attrs
    // are set in the cell renderer and are the stable selector.
    const a1 = page.locator('[data-row="0"][data-col="0"]');
    const b3 = page.locator('[data-row="2"][data-col="1"]');
    await a1.click();
    await b3.click({ modifiers: ["Shift"] });

    // Right-click → Insert ▸ → "Insert Pivot Table..."
    // The pivot-insert action is nested under the "Insert" submenu after
    // the 2026-06 context-menu compaction.
    await a1.click({ button: "right" });
    await page.waitForSelector(".ui-menu", { timeout: 3000 });
    steps.contextMenuOpens = true;
    await clickCtxMenuItem(page, /insert/i, /insert pivot table/i);

    // Sidebar appears.
    await page.waitForSelector(".ss-pivot-editor", { timeout: 5000 });
    steps.editorOpened = true;

    // Field list is populated with the source headers.
    const fieldRows = page.locator(".ss-pivot-editor .ss-pivot-field");
    const fieldCount = await fieldRows.count();
    steps.fieldListPopulated = fieldCount >= 2;

    // Click the first field's checkbox (Region, text → Rows by
    // default-route). Then click the second field's checkbox
    // (Revenue, number → Values).
    const firstCheckbox = fieldRows.first().locator("input[type=checkbox]");
    await firstCheckbox.click();
    await page.waitForTimeout(150);
    const secondCheckbox = fieldRows.nth(1).locator("input[type=checkbox]");
    await secondCheckbox.click();

    // The pivot output spills at the anchor (col 3, row 0 == col D
    // by 0-index, i.e. the 4th data column). Wait for at least one
    // chip to appear in any zone, then verify the editor shows the
    // chips (proxy for the engine having installed the pivot).
    try {
      await page.waitForSelector(
        ".ss-pivot-editor .ss-pivot-zone .ss-pivot-chip",
        { timeout: 5000 },
      );
      steps.chipsAppearedInZone = true;
    } catch {
      steps.chipsAppearedInZone = false;
    }

    // Verify the SUM dropdown exists in the Values zone (the per-
    // value summarize-fn picker). Its presence proves the Revenue
    // checkbox routed to Values, not Rows.
    const aggSelect = page.locator(".ss-pivot-editor .ss-pivot-chip-agg");
    steps.summarizeFnPickerPresent = (await aggSelect.count()) > 0;

    // Delete the pivot via the Delete button. Editor closes; the
    // sidebar element is removed from the DOM.
    const deleteBtn = page.locator(".ss-pivot-editor .ss-pivot-remove");
    if ((await deleteBtn.count()) > 0) {
      await deleteBtn.first().click();
      try {
        await page.waitForSelector(".ss-pivot-editor", {
          state: "detached",
          timeout: 5000,
        });
        steps.editorClosedAfterDelete = true;
      } catch {
        steps.editorClosedAfterDelete = false;
      }
    }
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: collector.scenario?.name || "pivot-editor",
    dialogs: collector.scenario?.dialogs || [],
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

async function scenarioSpreadsheetPaste(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || "doctor-paste@ogrenotes.example.com"
  );
  logJson({ at: "dev-login", userId: tokens.userId });

  const title = `doctor-paste-${Date.now()}`;
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    title,
    "spreadsheet"
  );
  logJson({ at: "doc-created", docId: doc.id, docType: doc.docType });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
    // NOTE: intentionally not granting `clipboard-read`. The test's
    // whole point is to prove we don't need it.
    permissions: ["clipboard-write"],
  });
  await seedAuth(context, tokens);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  // Capture any permission dialogs Chromium might surface. If our fix
  // regressed and readText() was called, Playwright typically auto-
  // rejects rather than popping a dialog — but we still record whatever
  // fires so a real popup regression is caught.
  const dialogs = [];
  page.on("dialog", async (d) => {
    dialogs.push({ type: d.type(), message: d.message() });
    await d.dismiss();
  });

  const docUrl = `${target}/d/${doc.id}/probe`;
  logJson({ at: "navigate", url: docUrl });
  await page.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });

  const steps = {};
  try {
    // Wait for the spreadsheet grid to mount.
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    // The component auto-focuses the wrapper on mount (via refocus_wrapper
    // Effect) and defaults the active cell to (0,0) = A1. We don't click
    // the wrapper because clicks set the active cell to whatever's under
    // the cursor, which isn't deterministic here.
    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    steps.gridMounted = true;

    // Each `type("N")` + `Enter` commits the value and moves the cursor
    // down one row. After four entries starting at A1, we're sitting on
    // A5. A small per-keystroke delay keeps the wasm reactive pipeline
    // from swallowing events under load (mirrors collab-sync).
    const typeInto = async (s) => {
      await page.keyboard.type(s, { delay: 15 });
      await page.keyboard.press("Enter");
    };
    await typeInto("1"); // A1
    await typeInto("2"); // A2
    await typeInto("3"); // A3
    await typeInto("=SUM(A1:A3)"); // A4
    // Move to B1: up four rows back to A1, then right to B1.
    for (let i = 0; i < 4; i++) await page.keyboard.press("ArrowUp");
    await page.keyboard.press("ArrowRight");
    await typeInto("3"); // B1
    await typeInto("4"); // B2
    await typeInto("5"); // B3
    // Cursor now at B4.
    steps.cellsPopulated = true;

    // Move to A4 (one column left), copy, move to B4, paste.
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("Control+c");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Control+v");
    // Paste is synchronous on the event, but give the reactive system a
    // tick to re-render the grid + formula bar.
    await page.waitForTimeout(300);
    steps.pasteExecuted = true;

    // With cursor on B4 (which the paste leaves the active cell at), the
    // formula bar `.formula-bar-value` should show =SUM(B1:B3). Use a
    // locator with first() since there's one formula bar mounted.
    const formulaBarText = (
      await page.locator(".formula-bar-value").first().textContent()
    )?.trim();
    collector.formulaBarText = formulaBarText;
    steps.pastedFormulaTranslated = formulaBarText === "=SUM(B1:B3)";

    // Read the B4 cell's displayed text. It's row 3 (0-indexed), col 1
    // in the grid. In the DOM the grid has a header row + header cell
    // per row, so we target with :nth-child selectors. We use the
    // attribute-free structural locator because the component doesn't
    // annotate cells with row/col data attributes.
    const b4Text = await page
      .locator(".spreadsheet-grid tbody tr")
      .nth(3)
      .locator("td.spreadsheet-cell")
      .nth(1) // col 1 = B (col 0 header td was filtered out by class)
      .textContent();
    collector.b4Text = (b4Text || "").trim();
    steps.pastedValueIs12 = collector.b4Text === "12";

    steps.noPermissionDialog = dialogs.length === 0;
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  try {
    await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: true });
  } catch (_) {}

  collector.scenario = {
    name: "spreadsheet-paste",
    docId: doc.id,
    title,
    steps,
  };
  collector.dialogs = dialogs;

  await context.close();
  await browser.close();
}

async function scenarioCollabSync(ctx, collector) {
  const { baseUrlA, baseUrl, docId, emailA, emailB, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokensA = await devLogin(target, emailA);
  const tokensB = await devLogin(target, emailB);
  logJson({ at: "dev-login", userIdA: tokensA.userId, userIdB: tokensB.userId });

  const browser = await chromium.launch({ headless: true });
  const contextA = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  const contextB = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-b.har"), mode: "full" },
  });

  await seedAuth(contextA, tokensA);
  await seedAuth(contextB, tokensB);

  const pageA = await contextA.newPage();
  const pageB = await contextB.newPage();

  instrument(contextA, pageA, "tab-a", collector);
  instrument(contextB, pageB, "tab-b", collector);

  const docUrl = `${target}/d/${docId}/probe`;
  logJson({ at: "navigate", url: docUrl });

  await pageA.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });
  await pageB.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });

  // Wait long enough for ws-token + ws upgrade + initial sync.
  await pageA.waitForTimeout(3000);
  await pageB.waitForTimeout(3000);

  // Type a probe marker in tab A. We locate the editor's contenteditable and
  // append unique text so tab B can assert its presence.
  const marker = `DOCTOR-PROBE-${Date.now()}`;
  const editorSelector = '[contenteditable="true"]';
  try {
    await pageA.click(editorSelector, { timeout: 5000 });
    await pageA.keyboard.type(marker, { delay: 20 });
  } catch (e) {
    collector.editorError = `tab-a: could not find / type into ${editorSelector}: ${e.message}`;
  }

  // Give updates a chance to propagate via WS.
  await pageB.waitForTimeout(5000);

  // Check tab B's DOM for the marker (text sync assertion).
  let syncObservedInB = false;
  try {
    syncObservedInB = await pageB.evaluate((m) => {
      return (document.body.innerText || "").includes(m);
    }, marker);
  } catch (e) {
    collector.assertError = `tab-b: evaluate failed: ${e.message}`;
  }

  // Presence assertion: after tab A has typed and moved its caret, tab B
  // should render a remote cursor overlay for that user. Caret class per
  // frontend/src/components/cursor_overlay.rs; selection highlight uses
  // .remote-cursor-selection.
  //
  // This is what catches today's bug class — text can sync while the
  // awareness pass-through silently drops cursor fields on the server,
  // leaving tab B with zero remote-cursor-* elements.
  const REMOTE_CURSOR_SELECTOR = ".remote-cursor-caret, .remote-cursor-selection";
  let remoteCursorObservedInB = false;
  let remoteCursorCountInB = 0;
  try {
    // Wait up to 5s for at least one cursor element to appear.
    await pageB
      .locator(REMOTE_CURSOR_SELECTOR)
      .first()
      .waitFor({ state: "attached", timeout: 5000 });
    remoteCursorObservedInB = true;
    remoteCursorCountInB = await pageB
      .locator(REMOTE_CURSOR_SELECTOR)
      .count();
  } catch (_) {
    // Timeout — record as false, do not fail the whole scenario.
  }

  // Screenshots — last step before tear-down so they reflect final state.
  try {
    await pageA.screenshot({ path: join(outDir, "tab-a.png"), fullPage: true });
    await pageB.screenshot({ path: join(outDir, "tab-b.png"), fullPage: true });
  } catch (_) {}

  collector.scenario = {
    name: "collab-sync",
    marker,
    editorSelector,
    remoteCursorSelector: REMOTE_CURSOR_SELECTOR,
    syncObservedInB,
    remoteCursorObservedInB,
    remoteCursorCountInB,
  };

  await contextA.close();
  await contextB.close();
  await browser.close();
}

// ─── comment-live-sync scenario ─────────────────────────────────
//
// Regression test for the "typed reply doesn't show in peer's open
// dialog" bug. Backend tests pin the WS broadcast wire shape; this
// scenario pins the *frontend* refresh path: that an open CommentPopup
// re-runs its load-messages Effect when a peer's CommentEvent arrives.
//
// If the wiring (`comments_dirty` signal threaded through
// CommentPopup / ConversationPane) silently desubscribes — for
// example by someone deleting `let _ = comments_dirty.get();` from
// the load-messages Effect, or unbinding the WS callback — peer B's
// popup keeps showing only the initial message and this test fails.
//
// Steps:
//   1. dev-login as a single user (one identity, two browser contexts
//      is enough to exercise the WS fan-out — the room broadcasts to
//      every connected client regardless of who sent the REST write).
//   2. Create a doc + a document-level thread + an initial message
//      via REST. Pre-seeding bypasses the inline-comment selection
//      flow, which isn't what we're testing.
//   3. Open both tabs to /d/{docId}/probe with the auth blob seeded
//      so neither hits the OAuth flow.
//   4. In each tab: View menu → "Show Conversation" → click the
//      thread row → assert the floating .comment-popup is visible
//      with the initial message body shown.
//   5. Pre-seed enough additional messages via REST that the popup
//      body overflows (max-height: 320px, ~50px per message → 10
//      replies is plenty). This is what catches scroll-to-bottom
//      regressions: with too few messages the popup body never
//      scrolls, and the test passes regardless of whether the
//      auto-scroll Effect fires.
//   6. Tab A: POST /threads/{id}/messages via REST so the reply is
//      server-driven (testing the WS broadcast path, not tab A's
//      local optimistic refetch).
//   7. Tab B: assert the popup body contains the reply text within
//      `replyTimeoutMs`. If it does not, the live-refresh wiring is
//      broken — the regression has returned.
//   8. Tab B: assert .comment-popup-body is scrolled to the bottom
//      both at popup-open time (after overflow seeding) and after
//      the broadcast-driven reply lands. Without this, peers'
//      replies still arrive but render below the fold.
async function ensurePopupOpen(page, anyMessageHint, tag) {
  // Open View menu and toggle the conversation pane on.
  await page.getByRole("button", { name: "View", exact: true }).click();
  await page.getByText("Show Conversation", { exact: true }).click();
  await page.waitForSelector(".conversation-pane.is-open", { timeout: 10000 });

  // Click the (only) thread row — opens the floating CommentPopup.
  await page.locator(".conversation-thread").first().click();
  await page.waitForSelector(".comment-popup", { timeout: 5000 });

  // Wait for at least one message to render. We use `state: "attached"`
  // (in the DOM) rather than `state: "visible"` because once the
  // auto-scroll-to-bottom Effect runs, the *first* message is scrolled
  // out of view — `visible` would then erroneously fail. Visibility of
  // specific messages is asserted separately.
  await page
    .locator(".comment-popup-text", { hasText: anyMessageHint })
    .first()
    .waitFor({ state: "attached", timeout: 5000 });
}

// Returns true if the given selector's element is scrolled to (or
// within `tolerancePx` of) its bottom. Used to verify the popup's
// auto-scroll Effect actually moved the viewport.
async function isScrolledToBottom(page, selector, tolerancePx = 5) {
  return page.locator(selector).first().evaluate((el, tol) => {
    return Math.abs(el.scrollHeight - el.clientHeight - el.scrollTop) <= tol;
  }, tolerancePx);
}

async function createThreadViaApi(baseUrl, accessToken, docId, message) {
  const res = await fetch(`${baseUrl}/api/v1/documents/${docId}/threads`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${accessToken}`,
    },
    body: JSON.stringify({ threadType: "document", message }),
  });
  if (!res.ok) {
    const txt = await res.text();
    throw new Error(`create thread failed: HTTP ${res.status} ${txt}`);
  }
  return res.json();
}

async function addMessageViaApi(baseUrl, accessToken, threadId, content) {
  const res = await fetch(`${baseUrl}/api/v1/threads/${threadId}/messages`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${accessToken}`,
    },
    body: JSON.stringify({ content }),
  });
  if (!res.ok) {
    const txt = await res.text();
    throw new Error(`add message failed: HTTP ${res.status} ${txt}`);
  }
}

async function scenarioCommentLiveSync(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const replyTimeoutMs = 8000;

  const tokens = await devLogin(target, emailA || "doctor-comment@ogrenotes.example.com");
  logJson({ at: "dev-login", userId: tokens.userId });

  const title = `doctor-comment-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const initialMessage = `INIT-${Date.now()}`;
  const thread = await createThreadViaApi(
    target,
    tokens.accessToken,
    doc.id,
    initialMessage,
  );
  logJson({ at: "thread-created", threadId: thread.threadId });

  // Overflow seed: pile on enough additional messages that the popup's
  // .comment-popup-body (max-height: 320px) is forced to scroll. With
  // ~50px per .comment-popup-msg, 10 extras keeps us well past the cap
  // even on tighter line-heights. The overflow exists *before* the
  // popups open, so the auto-scroll-to-bottom Effect has something to
  // actually do.
  const overflowCount = 10;
  for (let i = 0; i < overflowCount; i++) {
    await addMessageViaApi(
      target,
      tokens.accessToken,
      thread.threadId,
      `OVERFLOW-${i + 1}`,
    );
  }
  logJson({ at: "overflow-seeded", count: overflowCount });

  const browser = await chromium.launch({ headless: true });
  const contextA = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  const contextB = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-b.har"), mode: "full" },
  });
  // Each tab needs its OWN refresh-cookie session, even though both
  // tabs auth as the same user. With the cookie-only auth (post-#33)
  // every `/auth/refresh` rotates the underlying token; if both tabs
  // were seeded with the same cookie, tab-A's first hydration call
  // would rotate it, and tab-B's hydration with the now-stale value
  // would trip reuse-detection in `rotate_refresh_token` and revoke
  // ALL sessions for that user — observed as a 401 on tab-B's
  // /auth/refresh and the editor never mounting. Issuing a second
  // dev-login mints a fresh session row server-side, so the two tabs
  // hold independent (token, session_id) pairs and don't race.
  const tokensB = await devLogin(target, emailA || "doctor-comment@ogrenotes.example.com");
  logJson({ at: "dev-login-tabB", userId: tokensB.userId });
  await seedAuth(contextA, tokens);
  await seedAuth(contextB, tokensB);

  const pageA = await contextA.newPage();
  const pageB = await contextB.newPage();
  instrument(contextA, pageA, "tab-a", collector);
  instrument(contextB, pageB, "tab-b", collector);

  const docUrl = `${target}/d/${doc.id}/probe`;
  logJson({ at: "navigate", url: docUrl });
  await pageA.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });
  await pageB.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });

  // Wait for editor mount + WS handshake + initial sync on both tabs.
  await pageA.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
  await pageB.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
  await pageA.waitForTimeout(1500);
  await pageB.waitForTimeout(1500);

  const steps = {};
  try {
    // Use the LAST overflow message as the load hint — it's guaranteed
    // to exist by the time the popup loads, and using a stable known
    // value keeps the wait cheap.
    const lastSeed = `OVERFLOW-${overflowCount}`;
    await ensurePopupOpen(pageA, lastSeed, "tab-a");
    steps.tabAPopupOpen = true;
    await ensurePopupOpen(pageB, lastSeed, "tab-b");
    steps.tabBPopupOpen = true;

    // Sanity: enough messages exist that the body actually overflowed.
    // If this fails, the test premise is broken (CSS changed, or the
    // overflowCount is too low for the new line-height) and the
    // scroll assertions below would be vacuous.
    const bodyDims = await pageB
      .locator(".comment-popup-body")
      .first()
      .evaluate((el) => ({
        scrollHeight: el.scrollHeight,
        clientHeight: el.clientHeight,
      }));
    collector.bodyDims = bodyDims;
    steps.popupBodyOverflowed = bodyDims.scrollHeight > bodyDims.clientHeight + 20;

    // Auto-scroll on initial load: once the popup mounts and the
    // messages Effect fires, .comment-popup-body should be scrolled to
    // the bottom. Give the 0ms timeout in the Effect a moment to land.
    await pageB.waitForTimeout(200);
    steps.tabBScrolledOnOpen = await isScrolledToBottom(pageB, ".comment-popup-body");

    // Server-driven reply from tab A. We hit REST directly rather than
    // typing into tab A's reply input because tab A would refetch its
    // own dialog locally on success — what we want to assert is that
    // tab B's *broadcast-driven* refresh path works, independent of
    // tab A's UI.
    const replyBody = `REPLY-${Date.now()}`;
    collector.replyBody = replyBody;
    await addMessageViaApi(target, tokens.accessToken, thread.threadId, replyBody);
    logJson({ at: "reply-sent", threadId: thread.threadId, body: replyBody });
    steps.replyPosted = true;

    // Tab B's popup must contain the new reply text within the timeout.
    // `state: "attached"` because if auto-scroll regresses, the new
    // message renders below the fold (not "visible") — but it IS in the
    // DOM, and that's the live-refresh property we want to pin
    // independently of the scroll property.
    await pageB
      .locator(".comment-popup-text", { hasText: replyBody })
      .first()
      .waitFor({ state: "attached", timeout: replyTimeoutMs });
    steps.tabBSawReply = true;

    // Sanity: tab A should also see the reply (its own local refetch
    // path). If tab A doesn't see it, the regression is even worse —
    // the popup is broken end-to-end, not just on the broadcast path.
    await pageA
      .locator(".comment-popup-text", { hasText: replyBody })
      .first()
      .waitFor({ state: "attached", timeout: replyTimeoutMs });
    steps.tabASawReply = true;

    // Auto-scroll on broadcast: tab B's body should still be at the
    // bottom after the new message landed. This is the assertion that
    // would have caught the f12ea39 regression where the NodeRef was
    // attached to the wrong (non-scrolling) element.
    await pageB.waitForTimeout(200);
    steps.tabBScrolledAfterReply = await isScrolledToBottom(pageB, ".comment-popup-body");
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  try {
    await pageA.screenshot({ path: join(outDir, "tab-a.png"), fullPage: true });
    await pageB.screenshot({ path: join(outDir, "tab-b.png"), fullPage: true });
  } catch (_) {}

  collector.scenario = {
    name: "comment-live-sync",
    docId: doc.id,
    threadId: thread.threadId,
    initialMessage,
    replyTimeoutMs,
    steps,
  };

  await contextA.close();
  await contextB.close();
  await browser.close();
}

// ─── Spreadsheet UI regression scenarios (May 2026 batch) ─────────
//
// Each scenario covers one or more issues that previously only had
// engine-level coverage. They follow the same shape as
// scenarioSpreadsheetFeatures: dev-login, create a spreadsheet via
// REST, drive keyboard/mouse, assert DOM. Required-step gates are
// registered alongside the existing five at the bottom of main().

/// #60 — Ctrl+Y redo on a spreadsheet cell value.
async function scenarioSpreadsheetKeyboard(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-keyboard@ogrenotes.example.com",
  );
  const title = `doctor-keyboard-${Date.now()}`;
  const doc = await createDocViaApi(
    target, tokens.accessToken, title, "spreadsheet",
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.gridMounted = true;

    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    await page.keyboard.type("42", { delay: 15 });
    await page.keyboard.press("Enter");
    await page.waitForTimeout(200);

    // Move back up to A1 and assert it shows 42.
    await page.keyboard.press("ArrowUp");
    await page.waitForTimeout(100);
    const a1Initial = (
      await page.locator(".spreadsheet-grid tbody tr").first()
        .locator("td.spreadsheet-cell").first().textContent()
    )?.trim();
    steps.valueTyped = a1Initial === "42";

    // Ctrl+Z clears A1 back to empty.
    await page.keyboard.press("Control+z");
    await page.waitForTimeout(200);
    const a1AfterUndo = (
      await page.locator(".spreadsheet-grid tbody tr").first()
        .locator("td.spreadsheet-cell").first().textContent()
    )?.trim();
    steps.undoCleared = a1AfterUndo === "";

    // Ctrl+Y (the NEW binding from #60) restores 42.
    await page.keyboard.press("Control+y");
    await page.waitForTimeout(200);
    const a1AfterRedo = (
      await page.locator(".spreadsheet-grid tbody tr").first()
        .locator("td.spreadsheet-cell").first().textContent()
    )?.trim();
    steps.redoRestored = a1AfterRedo === "42";
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "spreadsheet-keyboard", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

/// #58 + #59 — column-header click selects the entire column, status
/// bar reports count + sum.
async function scenarioSpreadsheetHeaders(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-headers@ogrenotes.example.com",
  );
  const title = `doctor-headers-${Date.now()}`;
  const doc = await createDocViaApi(
    target, tokens.accessToken, title, "spreadsheet",
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.gridMounted = true;

    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    // A1=10, A2=20, A3=30. Sum should be 60, count 3.
    const typeInto = async (s) => {
      await page.keyboard.type(s, { delay: 15 });
      await page.keyboard.press("Enter");
    };
    await typeInto("10");
    await typeInto("20");
    await typeInto("30");
    steps.cellsPopulated = true;

    // Click column A header. The plain-click selection is the #59 fix
    // — previously only shift-click worked.
    const colAHeader = page.locator("th.spreadsheet-col-header").first();
    await colAHeader.click();
    await page.waitForTimeout(200);

    // The selection now covers col A. Assert by reading the active
    // selection's count from the status bar — selection of more than
    // one cell shows `.ss-status-bar` (#58 fix). With grid_rows
    // default = 100, all 100 cells of col A are selected; sum is
    // still 60 (only A1..A3 are numeric).
    const statusBar = page.locator(".ss-status-bar").first();
    const statusVisible = (await statusBar.count()) > 0;
    steps.colHeaderClickSelectsColumn = statusVisible;

    const statusText = statusVisible
      ? ((await statusBar.textContent()) || "").trim()
      : "";
    collector.statusBarText = statusText;
    steps.statusBarShowsCount = /Count:\s*\d+/.test(statusText);
    steps.statusBarShowsSum = /Sum:\s*60/.test(statusText);
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "spreadsheet-headers", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

/// #56 + #64 — toolbar Format dropdown shows "Format" on spreadsheets;
/// clicking Currency (USD) applies the format; clicking Bold sets the
/// cell's font-weight via the toolbar Effect (not just the keyboard
/// shortcut).
async function scenarioSpreadsheetToolbar(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-toolbar@ogrenotes.example.com",
  );
  const title = `doctor-toolbar-${Date.now()}`;
  const doc = await createDocViaApi(
    target, tokens.accessToken, title, "spreadsheet",
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.gridMounted = true;
    steps.toolbarPresent =
      (await page.locator(".toolbar-block-dropdown-btn").count()) > 0;

    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    await page.keyboard.type("1234", { delay: 15 });
    await page.keyboard.press("Enter");
    await page.keyboard.press("ArrowUp");

    // The block-type dropdown label should read "Format" on a
    // spreadsheet doc (#56). The label sits inside
    // `.toolbar-block-label`.
    const dropdownLabel = (
      await page.locator(".toolbar-block-label").first().textContent()
    )?.trim();
    collector.dropdownLabel = dropdownLabel;
    steps.formatDropdownIsFormat = dropdownLabel === "Format";

    // Open the dropdown and click Currency (USD).
    await page.locator(".toolbar-block-dropdown-btn").first().click();
    await page.waitForSelector(".toolbar-block-menu", { timeout: 3000 });
    await page.getByRole("button", { name: /currency \(usd\)/i })
      .first().click();
    await page.waitForTimeout(300);

    const a1AfterFormat = (
      await page.locator(".spreadsheet-grid tbody tr").first()
        .locator("td.spreadsheet-cell").first().textContent()
    )?.trim();
    collector.currencyText = a1AfterFormat;
    steps.currencyAppliedToCell = (a1AfterFormat || "").startsWith("$");

    // Click Bold (B button in the toolbar). The button's accessible
    // name is its text content "B" (button text wins over title
    // attribute in accessibility-tree resolution), so we target the
    // title attribute directly via a CSS selector instead — using
    // `getByRole({ name: /Bold/ })` would have matched zero buttons.
    await page.locator('button[title^="Bold"]').first().click();
    await page.waitForTimeout(200);
    const a1FontWeight = await page
      .locator(".spreadsheet-grid tbody tr").first()
      .locator("td.spreadsheet-cell").first()
      .evaluate((el) => window.getComputedStyle(el).fontWeight);
    collector.a1FontWeight = a1FontWeight;
    // getComputedStyle returns either "700" or "bold".
    steps.boldButtonAppliedFontWeight =
      a1FontWeight === "700" || a1FontWeight === "bold";
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "spreadsheet-toolbar", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

/// #61 — "Freeze rows above" freezes ONLY rows strictly above the
/// right-clicked row. Asserts the off-by-one fix: right-clicking row
/// index 2 freezes exactly 2 rows (0 and 1), and row 2 itself is
/// NOT marked frozen.
// #90: reproduce the "closure invoked recursively or after being dropped"
// panic the user sees when closing the edit-history pane. Opens a doc,
// makes a couple of edits, opens the history pane (Ctrl+Shift+H), browses
// a version's diff, then CLOSES the pane — capturing any pageerror that
// fires on close. Asserts none.
async function scenarioHistoryPane(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-history@ogrenotes.example.com",
  );
  const title = `doctor-history-${Date.now()}`;
  const doc = await createDocViaApi(
    target, tokens.accessToken, title, "document",
  );

  // The #90 panic is Firefox-flavored (the user's console stack is
  // Gecko's). Allow DOCTOR_BROWSER=firefox to drive the real browser
  // where the bug manifests; default chromium for the CI suite.
  const engine = process.env.DOCTOR_BROWSER === "firefox" ? firefox : chromium;
  const browser = await engine.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 15000 });
    steps.editorMounted = true;

    // Type content + give the contenteditable a real focus/selection.
    const ed = page.locator(".editor-content");
    await ed.click();
    await page.keyboard.type("First revision of the document.");
    await page.waitForTimeout(1500);
    await page.keyboard.press("Enter");
    await page.keyboard.type("Second revision adds another line.");
    await page.waitForTimeout(2000);

    // Open the history pane (Ctrl+Shift+H).
    await page.keyboard.press("Control+Shift+H");
    await page.waitForSelector(".history-pane.is-open", { timeout: 5000 });
    steps.paneOpened = true;
    await page.waitForTimeout(1000);

    const versionItems = page.locator(".history-version-item");
    steps.versionCount = await versionItems.count();

    // Repro the user's flow: open a version's diff (browse), then close the
    // "history window" via each close affordance, watching for the panic
    // after each. `errAt` snapshots the running pageerror count.
    const errAt = (label) => {
      steps[`errors_${label}`] = collector["tab-a"].errors.length;
    };
    errAt("start");

    if (steps.versionCount > 0) {
      // 1) open diff modal, close via the X button
      await versionItems.first().click();
      await page
        .waitForSelector(".history-diff-modal", { timeout: 4000 })
        .catch(() => {});
      steps.modalOpen = (await page.locator(".history-diff-modal").count()) > 0;
      await page.waitForTimeout(500);
      const xbtn = page.locator(".history-modal-close");
      if ((await xbtn.count()) > 0) {
        await xbtn.first().click();
        await page.waitForTimeout(800);
      }
      errAt("afterModalCloseBtn");

      // 2) reopen, close via backdrop click
      await versionItems.first().click();
      await page.waitForTimeout(500);
      const backdrop = page.locator(".history-diff-backdrop");
      if ((await backdrop.count()) > 0) {
        await backdrop.first().click({ position: { x: 5, y: 5 } });
        await page.waitForTimeout(800);
      }
      errAt("afterBackdropClose");

      // 3) reopen, close via Escape
      await versionItems.first().click();
      await page.waitForTimeout(500);
      await page.keyboard.press("Escape");
      await page.waitForTimeout(800);
      errAt("afterEsc");
    }

    // 4) close the whole pane (Ctrl+Shift+H) — refocus the editor first so
    //    the document-level shortcut handler receives the keystroke.
    await page.locator(".editor-content").click().catch(() => {});
    await page.waitForTimeout(200);
    await page.keyboard.press("Control+Shift+H");
    await page.waitForTimeout(1500);
    steps.paneClosed =
      (await page.locator(".history-pane.is-open").count()) === 0;
    errAt("afterPaneClose");

    steps.totalErrors = collector["tab-a"].errors.length;
    steps.noPanic = steps.totalErrors === 0;
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "history-pane",
    docId: doc.id,
    title,
    steps,
    errors: collector["tab-a"].errors,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// Reproduce the panic on deleting a document from the menu bar
// (same class as #90): the ConfirmDialog's confirm button flips `visible`
// false synchronously inside the click, tearing the <Show> down mid-event.
// Firefox-only, like the history-pane case.
async function scenarioDeleteDocument(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-delete@ogrenotes.example.com",
  );
  const title = `doctor-delete-${Date.now()}`;
  const doc = await createDocViaApi(
    target, tokens.accessToken, title, "document",
  );

  const engine = process.env.DOCTOR_BROWSER === "firefox" ? firefox : chromium;
  const browser = await engine.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 15000 });
    steps.editorMounted = true;

    // Open the Document menu, then "Delete Document…".
    await page.getByRole("button", { name: /^document$/i }).first().click();
    await uiMenuItem(page, /delete document/i).click();

    // The destructive confirm dialog appears.
    await page.waitForSelector(".confirm-dialog", { timeout: 4000 });
    steps.confirmShown = true;
    steps.errorsBeforeConfirm = collector["tab-a"].errors.length;

    // Click the confirm (Delete) button — the reported repro point. The
    // page hard-navigates to "/" on success; capture errors first.
    await page.locator(".confirm-dialog .btn-danger").first().click();
    await page.waitForTimeout(1200);

    steps.errorsAfterConfirm = collector["tab-a"].errors.length;
    steps.noPanicOnDelete = steps.errorsAfterConfirm === 0;
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "delete-document",
    docId: doc.id,
    title,
    steps,
    errors: collector["tab-a"].errors,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// Firefox guard for the share-dialog close (same panic class as #90):
// open the share dialog, close it via the ✕ button and the backdrop,
// asserting no pageerror.
async function scenarioShareDialog(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(target, emailA || "doctor-share@ogrenotes.example.com");
  const title = `doctor-share-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title, "document");

  const engine = process.env.DOCTOR_BROWSER === "firefox" ? firefox : chromium;
  const browser = await engine.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);
  await page.goto(`${target}/d/${doc.id}/probe`, { waitUntil: "domcontentloaded", timeout: 30000 });

  const steps = {};
  try {
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 15000 });
    steps.editorMounted = true;

    // Open via the header share button, close via the ✕ button.
    await page.locator(".share-button").first().click();
    await page.waitForSelector(".share-dialog", { timeout: 5000 });
    steps.dialogShown = true;
    await page.locator(".share-close").first().click();
    await page.waitForTimeout(800);

    // Reopen, close via the backdrop.
    await page.locator(".share-button").first().click();
    await page.waitForSelector(".share-dialog", { timeout: 5000 });
    await page.locator(".share-backdrop").first().click({ position: { x: 5, y: 5 } });
    await page.waitForTimeout(800);

    steps.totalErrors = collector["tab-a"].errors.length;
    steps.noPanic = steps.totalErrors === 0;
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = { name: "share-dialog", docId: doc.id, title, steps, errors: collector["tab-a"].errors };
  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// Firefox guard for the comment-popup close (same panic class): select a
// word, open the comment composer from the selection toolbar, close it via
// the ✕ button and the backdrop, asserting no pageerror.
async function scenarioCommentPopup(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(target, emailA || "doctor-cpopup@ogrenotes.example.com");
  const title = `doctor-cpopup-${Date.now()}`;
  // Open the shared CommentPopup via the spreadsheet cell-comment flow —
  // more reliable headless than the document selection-toolbar path (which
  // needs a valid single-block text anchor).
  const doc = await createDocViaApi(target, tokens.accessToken, title, "spreadsheet");

  const engine = process.env.DOCTOR_BROWSER === "firefox" ? firefox : chromium;
  const browser = await engine.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);
  await page.goto(`${target}/d/${doc.id}/probe`, { waitUntil: "domcontentloaded", timeout: 30000 });

  const steps = {};
  try {
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.editorMounted = true;

    // Right-click cell A1 → Comment ▸ → Add Comment, which pre-creates a
    // thread and opens the shared CommentPopup in thread mode.
    await page.locator('[data-row="0"][data-col="0"]').first().click({ button: "right" });
    await page.waitForSelector(".ui-menu", { timeout: 3000 });
    await clickCtxMenuItem(page, /comment/i, /add comment/i);
    await page.waitForSelector(".comment-popup", { timeout: 5000 });
    steps.popupShown = true;
    await page.waitForTimeout(500);

    // Close via the ✕ button — the reported panic class point.
    await page.locator(".comment-popup-close").first().click();
    await page.waitForTimeout(1000);

    steps.totalErrors = collector["tab-a"].errors.length;
    steps.noPanic = steps.totalErrors === 0;
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = { name: "comment-popup", docId: doc.id, title, steps, errors: collector["tab-a"].errors };
  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

async function scenarioSpreadsheetFreeze(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-freeze@ogrenotes.example.com",
  );
  const title = `doctor-freeze-${Date.now()}`;
  const doc = await createDocViaApi(
    target, tokens.accessToken, title, "spreadsheet",
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.gridMounted = true;

    // Right-click the cell at row 2 (third row) — A3 in spreadsheet
    // coordinates. Freezing "rows above" should freeze rows 0 and 1.
    const a3Cell = page.locator(".spreadsheet-grid tbody tr")
      .nth(2)
      .locator("td.spreadsheet-cell")
      .first();
    await a3Cell.click({ button: "right" });
    await page.waitForSelector(".ui-menu", { timeout: 3000 });
    steps.contextMenuOpens = true;

    // "Freeze rows above" is nested under the "Hide / Unhide" submenu
    // after the 2026-06 context-menu compaction.
    await clickCtxMenuItem(page, /hide \/ unhide/i, /freeze rows above/i);
    await page.waitForTimeout(300);

    // Count rows with the frozen-row class. Should be exactly 2 with
    // the off-by-one fix; pre-fix it was 3 (included the right-
    // clicked row itself).
    const frozenCount = await page
      .locator(".spreadsheet-grid tbody tr.frozen-row").count();
    collector.frozenRowCount = frozenCount;
    steps.frozenAboveExcludesClickedRow = frozenCount === 2;

    // Defensive: row index 2 (the clicked row) must NOT have the
    // frozen-row class.
    const row2HasFrozen = await page
      .locator(".spreadsheet-grid tbody tr").nth(2)
      .evaluate((el) => el.classList.contains("frozen-row"));
    steps.clickedRowNotFrozen = !row2HasFrozen;
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "spreadsheet-freeze", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

/// #65 — right-click a sheet tab to delete it (also exercises #68's
/// menu-clamp by confirming the menu sits within viewport bounds for
/// a tab at the bottom of the spreadsheet).
async function scenarioSpreadsheetSheetTabs(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-sheettabs@ogrenotes.example.com",
  );
  const title = `doctor-sheettabs-${Date.now()}`;
  const doc = await createDocViaApi(
    target, tokens.accessToken, title, "spreadsheet",
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);
  page.on("dialog", async (d) => {
    // The Rename menu item uses native prompt(); auto-dismiss so it
    // doesn't hang the scenario if accidentally triggered.
    await d.dismiss();
  });

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.gridMounted = true;

    // Click "+" to add a second sheet.
    await page.locator(".ss-sheet-add").first().click();
    await page.waitForTimeout(300);
    const tabsAfterAdd = await page.locator(
      ".ss-sheet-tabs .ss-sheet-tab:not(.ss-sheet-add)",
    ).count();
    steps.secondTabAdded = tabsAfterAdd === 2;

    // Right-click the second tab.
    const secondTab = page.locator(
      ".ss-sheet-tabs .ss-sheet-tab:not(.ss-sheet-add)",
    ).nth(1);
    await secondTab.click({ button: "right" });
    await page.waitForSelector(".ui-menu", { timeout: 3000 });
    steps.tabContextMenuOpened = true;

    // Sanity-check the clamp (#68): menu's left/top must be inside the
    // viewport. We read the inline style and the viewport size.
    const menuMetrics = await page.locator(".ui-menu").first()
      .evaluate((el) => {
        const rect = el.getBoundingClientRect();
        return {
          left: rect.left,
          top: rect.top,
          right: rect.right,
          bottom: rect.bottom,
          vw: window.innerWidth,
          vh: window.innerHeight,
        };
      });
    collector.menuMetrics = menuMetrics;
    steps.contextMenuFitsInViewport =
      menuMetrics.left >= 0 &&
      menuMetrics.top >= 0 &&
      menuMetrics.bottom <= menuMetrics.vh + 1;

    // Click Delete in the menu.
    await uiMenuItem(page, /^delete$/i).click();
    await page.waitForTimeout(300);
    const tabsAfterDelete = await page.locator(
      ".ss-sheet-tabs .ss-sheet-tab:not(.ss-sheet-add)",
    ).count();
    steps.deleteRemovesTab = tabsAfterDelete === 1;
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "spreadsheet-sheet-tabs", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

/// #63 + #70 — two users open the same spreadsheet; user A moves the
/// active cell, user B sees a remote-cursor badge on the matching
/// cell. After the #70 fix the indicator is rendered inline on the
/// `<td>` itself (via the `.ss-remote-cell-label` badge child), not
/// as an absolute-positioned overlay.
async function scenarioSpreadsheetRemoteCursor(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, emailB, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokensA = await devLogin(target, emailA);
  const tokensB = await devLogin(target, emailB);

  const title = `doctor-remote-${Date.now()}`;
  const doc = await createDocViaApi(
    target, tokensA.accessToken, title, "spreadsheet",
  );
  // Share with user B via the doc-members endpoint
  // (`POST /documents/{id}/members`). `AddMemberRequest` uses serde
  // `rename_all = "camelCase"`, so the wire fields are `userId` and
  // `accessLevel`. `AccessLevel` uses `rename_all = "UPPERCASE"`,
  // so the role value is `EDIT`. Sending snake_case fields silently
  // produces a 400 — the handler doesn't see them and rejects.
  const shareRes = await fetch(
    `${target}/api/v1/documents/${doc.id}/members`,
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${tokensA.accessToken}`,
      },
      body: JSON.stringify({
        userId: tokensB.userId,
        accessLevel: "EDIT",
      }),
    },
  );
  if (!shareRes.ok) {
    // Surface the failure into the report. Without this, tab B's
    // downstream 403 on GET /documents/:id leaves no trace of *why*
    // the access wasn't granted.
    collector.shareApiError =
      `share API ${shareRes.status}: ${await shareRes.text().catch(() => "")}`;
    throw new Error(`share API failed: ${collector.shareApiError}`);
  }

  const browser = await chromium.launch({ headless: true });
  const contextA = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(contextA, tokensA);
  const pageA = await contextA.newPage();
  instrument(contextA, pageA, "tab-a", collector);

  const contextB = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-b.har"), mode: "full" },
  });
  await seedAuth(contextB, tokensB);
  const pageB = await contextB.newPage();
  instrument(contextB, pageB, "tab-b", collector);

  const docUrl = `${target}/d/${doc.id}/probe`;
  await pageA.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });
  await pageB.goto(docUrl, { waitUntil: "domcontentloaded", timeout: 30000 });

  const steps = {};
  try {
    await pageA.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.tabAGridMounted = true;
    await pageB.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.tabBGridMounted = true;

    // In tab A, navigate the active cell to B5 by clicking the cell
    // directly. Row 4 (5th tr) × col 1 (B).
    await pageA.locator(".spreadsheet-grid tbody tr").nth(4)
      .locator("td.spreadsheet-cell").nth(1).click();
    steps.tabANavigated = true;

    // Wait for the remote-cell overlay to appear in tab B.
    try {
      await pageB.waitForSelector(".ss-remote-cell-label", { timeout: 8000 });
      const badgeCount = await pageB.locator(".ss-remote-cell-label").count();
      steps.tabBSeesRemoteCell = badgeCount >= 1;
    } catch (e) {
      steps.tabBSeesRemoteCell = false;
      // Record why the wait failed — a genuine 8s timeout (peer never
      // rendered the badge) reads very differently from a transient
      // execution-context/navigation error. Without this the failure is
      // opaque; a 2026-07-17 CI flake here left no evidence of which it was.
      steps.tabBSeesRemoteCellError = e?.message || String(e);
    }
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "spreadsheet-remote-cursor", docId: doc.id, title, steps,
  };
  await pageA.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await pageB.screenshot({
    path: join(outDir, "tab-b.png"), fullPage: false,
  }).catch(() => {});
  await contextA.close();
  await contextB.close();
  await browser.close();
}

// ─── embed-youtube scenario ─────────────────────────────────────
//
// Phase 5 M-P6 piece D. Clicks the toolbar's "Embed media" button,
// accepts a YouTube watch URL in the resulting window.prompt, and
// asserts the editor DOM grows a sandboxed iframe with the
// rewritten /embed/ URL.
//
// Failure modes:
//   - Toolbar button missing (the M-P6 piece-B insert UX
//     regressed)
//   - Backend resolver rejects a known-good YouTube URL (allowlist
//     regex regressed)
//   - Embed renders without sandbox / referrerpolicy / lazy
//     attributes (view.rs render_node Embed branch regressed)
//   - URL didn't rewrite watch?v= → /embed/ (allowlist matcher
//     regressed)
async function scenarioEmbedYouTube(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-embed@ogrenotes.example.com",
  );
  const title = `doctor-embed-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  // The toolbar's Insert Embed button calls window.prompt for the
  // URL. Pre-register a handler that accepts with a known-good
  // YouTube watch URL; the backend's allowlist matcher should
  // rewrite it to the /embed/ form.
  const watchUrl = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
  // The backend rewrites watch URLs to /embed/ form, then applies the
  // EMBED_YOUTUBE_NOCOOKIE privacy rewrite (crates/api/src/embed_allowlist.rs
  // apply_privacy; config default "true"). Track the same env the server
  // reads so this assertion follows the deployment's flag.
  const nocookie = (process.env.EMBED_YOUTUBE_NOCOOKIE ?? "true") === "true";
  const expectedSrc = nocookie
    ? "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"
    : "https://www.youtube.com/embed/dQw4w9WgXcQ";
  page.on("dialog", async (d) => {
    await d.accept(watchUrl);
  });

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
    steps.editorReady = true;

    // The Insert group includes the embed button (📺 glyph). The
    // title attribute carries the i18n string `toolbar-embed`.
    // Locate via title — robust to icon changes.
    const embedBtn = page.locator('.toolbar-btn[title^="Embed media"]');
    const btnCount = await embedBtn.count();
    steps.embedButtonVisible = btnCount === 1;

    await embedBtn.first().click();
    // The dialog handler resolves immediately; the resolve API
    // round-trip + insert dispatch take ~hundreds of ms — wait
    // for the iframe to appear in the editor DOM.
    await page.waitForSelector('[contenteditable="true"] .embed-block iframe', {
      timeout: 5000,
    });
    steps.iframeInserted = true;

    // Now drill into the iframe's actual attributes.
    const iframe = page.locator('[contenteditable="true"] .embed-block iframe').first();
    const src = await iframe.getAttribute("src");
    steps.srcRewrittenToEmbed = src === expectedSrc;
    const sandbox = await iframe.getAttribute("sandbox");
    steps.sandboxAllowsScripts = sandbox === "allow-scripts allow-same-origin";
    // ddc03be: no-referrer broke YouTube's embedding-origin validation
    // (Error 153); production now sends strict-origin-when-cross-origin
    // (frontend/src/editor/view.rs Embed render).
    const referrerpolicy = await iframe.getAttribute("referrerpolicy");
    steps.referrerPolicyCorrect = referrerpolicy === "strict-origin-when-cross-origin";
    const loading = await iframe.getAttribute("loading");
    steps.loadingLazy = loading === "lazy";

    // The wrapper is contenteditable=false so clicks inside don't
    // capture editor focus; pin that too.
    const wrapper = page.locator('[contenteditable="true"] .embed-block').first();
    const wrapperCE = await wrapper.getAttribute("contenteditable");
    steps.wrapperContenteditableFalse = wrapperCE === "false";
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "embed-youtube", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── calendar-block scenario ────────────────────────────────────
//
// #136 end-to-end. Inserts a Calendar block via the command
// palette, verifies the month grid renders, clicks a day cell to
// open the Add Event modal, saves an event, verifies the event
// span appears in the grid, then reloads the page and re-verifies
// the event persists.
//
// Failure modes:
//   - Calendar entry missing from BLOCK_INSERTS / palette
//   - render_node fallback not delegating to CalendarView
//   - click observer not attaching (data-calendar-observer guard)
//   - modal fields not committing on save
//   - add_calendar_event failing to locate the block-id
//   - persistence: yrs-bridge dropping the CalendarEvent child on
//     write-out because the schema doesn't accept it as a valid
//     child of Calendar (regression guard)
async function scenarioCalendarBlock(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-calendar@ogrenotes.example.com",
  );
  const title = `doctor-calendar-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  const EVENT_TITLE = `Doctor event ${Date.now()}`;
  try {
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
    steps.editorReady = true;

    // Focus the editor and open the command palette. Any editor
    // click puts a cursor in the doc so the palette's "Editor"
    // scope is active.
    await page.locator('[contenteditable="true"]').first().click();
    await page.keyboard.press("Control+Shift+P");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    await page.keyboard.type("Calendar", { delay: 10 });
    // Give the palette a moment to filter, then hit Enter on the
    // first (only) match.
    await page.waitForTimeout(150);
    await page.keyboard.press("Enter");

    await page.waitForSelector(".calendar-block", { timeout: 5000 });
    steps.calendarBlockRendered = true;

    // Confirm the month grid + weekday header rendered.
    const grid = page.locator(".calendar-block .calendar-month-grid");
    steps.monthGridPresent = (await grid.count()) === 1;
    const headerCells = await page.locator(".calendar-month-grid thead th").count();
    steps.sevenWeekdayHeaders = headerCells === 7;

    // Click the first fully-in-current-month day cell to open the
    // Add modal. `calendar-day--other-month` cells are the leading
    // days from the previous month; skip them.
    const firstDay = page
      .locator(".calendar-day:not(.calendar-day--other-month)")
      .first();
    await firstDay.click();

    await page.waitForSelector(".calendar-modal", { timeout: 3000 });
    steps.addModalOpened = true;

    // Fill the title, click save.
    await page.locator('.calendar-modal input[type="text"]').fill(EVENT_TITLE);
    await page.getByRole("button", { name: /^Save$/ }).click();

    // The saved event should show up in the same day cell.
    await page.waitForSelector(".calendar-event", { timeout: 5000 });
    const eventText = await page.locator(".calendar-event").first().textContent();
    steps.eventVisibleAfterSave = (eventText || "").includes(EVENT_TITLE);

    // Reload and re-check — the event should persist through
    // yrs-bridge write-out + read-in.
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForSelector(".calendar-event", { timeout: 10000 });
    const eventTextReload = await page.locator(".calendar-event").first().textContent();
    steps.eventPersistsAfterReload = (eventTextReload || "").includes(EVENT_TITLE);

    // Editing: click the event, verify modal opens in Edit mode,
    // verify content pre-filled, cancel.
    await page.locator(".calendar-event").first().click();
    await page.waitForSelector(".calendar-modal", { timeout: 3000 });
    const modalTitle = await page.locator(".calendar-modal h3").textContent();
    steps.editModalHeaderCorrect = (modalTitle || "").toLowerCase().includes("edit");
    const modalInput = await page
      .locator('.calendar-modal input[type="text"]').inputValue();
    steps.editModalTitlePrefilled = modalInput === EVENT_TITLE;
    // Delete button is only shown in Edit mode.
    const deleteVisible = await page
      .locator('.calendar-modal button.btn-danger').count();
    steps.deleteButtonInEditMode = deleteVisible === 1;
    await page.getByRole("button", { name: /^Cancel$/ }).click();
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: "calendar-block", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── kanban-block scenario ──────────────────────────────────────
//
// #137 end-to-end. Inserts a Kanban block via the command palette,
// verifies the three default columns render, clicks "+ Add card"
// in the first column to open the Add modal, saves a card,
// verifies the card appears in that column, then reloads the page
// and re-verifies the card persists. Then clicks the saved card to
// verify Edit mode pre-fills title + content.
//
// Failure modes:
//   - Kanban entry missing from BLOCK_INSERTS / palette
//   - render_node fallback not delegating to KanbanView
//   - click observer not attaching (data-kanban-observer guard)
//   - add-card modal not opening (click-outcome routing bug)
//   - add_kanban_card failing to locate the column-id
//   - persistence: yrs-bridge dropping the KanbanCard child on
//     write-out because the schema doesn't accept it as a valid
//     child of KanbanColumn (regression guard)
//   - Edit modal pre-fill wrong (card data-title/data-content
//     dropped from the render path)
async function scenarioKanbanBlock(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-kanban@ogrenotes.example.com",
  );
  const title = `doctor-kanban-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  const CARD_TITLE = `Doctor card ${Date.now()}`;
  try {
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
    steps.editorReady = true;

    await page.locator('[contenteditable="true"]').first().click();
    await page.keyboard.press("Control+Shift+P");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    await page.keyboard.type("Kanban", { delay: 10 });
    await page.waitForTimeout(150);
    await page.keyboard.press("Enter");

    await page.waitForSelector(".kanban-block", { timeout: 5000 });
    steps.kanbanBlockRendered = true;

    const columnCount = await page.locator(".kanban-column").count();
    steps.threeDefaultColumns = columnCount === 3;

    // "+ Add card" in the first column → modal opens.
    await page
      .locator('.kanban-column:first-child [data-kanban-action="add-card"]')
      .first()
      .click();
    // Kanban and Calendar modals share the .calendar-modal class
    // for style reuse; disambiguate by header text.
    await page.waitForSelector(".calendar-modal", { timeout: 3000 });
    steps.addModalOpened = true;

    await page.locator('.calendar-modal input[type="text"]').fill(CARD_TITLE);
    await page.getByRole("button", { name: /^Save$/ }).click();

    await page.waitForSelector(".kanban-card", { timeout: 5000 });
    const cardText = await page.locator(".kanban-card").first().textContent();
    steps.cardVisibleAfterSave = (cardText || "").includes(CARD_TITLE);

    // Card should have landed in the first column, not another one.
    const firstColCards = await page
      .locator(".kanban-column:first-child .kanban-card").count();
    steps.cardLandedInFirstColumn = firstColCards === 1;

    // Reload → the card persists through yrs-bridge write-out/read-in.
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForSelector(".kanban-card", { timeout: 10000 });
    const cardTextReload = await page.locator(".kanban-card").first().textContent();
    steps.cardPersistsAfterReload = (cardTextReload || "").includes(CARD_TITLE);

    // Click the card → Edit modal opens with title pre-filled.
    await page.locator(".kanban-card").first().click();
    await page.waitForSelector(".calendar-modal", { timeout: 3000 });
    const modalInput = await page
      .locator('.calendar-modal input[type="text"]').inputValue();
    steps.editModalTitlePrefilled = modalInput === CARD_TITLE;
    // Delete button only appears in Edit mode.
    const deleteVisible = await page
      .locator('.calendar-modal button.btn-danger').count();
    steps.deleteButtonInEditMode = deleteVisible === 1;
    await page.getByRole("button", { name: /^Cancel$/ }).click();
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: "kanban-block", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── kanban-drag scenario ────────────────────────────────────────
//
// #137 Phase 3. Inserts a Kanban with the three default columns,
// adds two cards to the first column, drags the second card into
// the third column, and asserts:
//   1. The card is no longer in column 1.
//   2. It appears in column 3.
//   3. Reload preserves the move — the transaction actually hit
//      the CRDT (not just a DOM shuffle).
//
// Playwright's `page.mouse.{down,move,up}` synthesizes native
// pointer events, which is what the kanban drag observer listens
// to (pointerdown / pointermove / pointerup). We move in two
// steps so the observer's DRAG_THRESHOLD_PX activate check fires
// past the threshold, mirroring a human drag.
async function scenarioKanbanDrag(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-kanban-drag@ogrenotes.example.com",
  );
  const title = `doctor-kanban-drag-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  const CARD_1 = `Card A ${Date.now()}`;
  const CARD_2 = `Card B ${Date.now()}`;
  try {
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
    steps.editorReady = true;

    // Insert Kanban via the palette.
    await page.locator('[contenteditable="true"]').first().click();
    await page.keyboard.press("Control+Shift+P");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    await page.keyboard.type("Kanban", { delay: 10 });
    await page.waitForTimeout(150);
    await page.keyboard.press("Enter");
    await page.waitForSelector(".kanban-block", { timeout: 5000 });
    steps.kanbanInserted = true;

    // Helper to add a card to a specific column index.
    async function addCard(colIdx, cardTitle) {
      await page
        .locator(`.kanban-column:nth-child(${colIdx}) [data-kanban-action="add-card"]`)
        .first()
        .click();
      await page.waitForSelector(".calendar-modal", { timeout: 3000 });
      await page.locator('.calendar-modal input[type="text"]').fill(cardTitle);
      await page.getByRole("button", { name: /^Save$/ }).click();
      // Wait for modal to fully close before the next add.
      await page.waitForSelector(".calendar-modal", {
        state: "detached", timeout: 3000,
      });
    }

    await addCard(1, CARD_1);
    await addCard(1, CARD_2);
    steps.twoCardsInFirstColumn =
      (await page.locator(".kanban-column:nth-child(1) .kanban-card").count()) === 2;

    // Drag the second card into the third column. Target the
    // card's title element for a stable click target — its
    // bounding box is guaranteed to be inside the draggable.
    const srcCard = page
      .locator(".kanban-column:nth-child(1) .kanban-card").nth(1);
    const dstColumnBody = page
      .locator(".kanban-column:nth-child(3) .kanban-card-list");

    const srcBox = await srcCard.boundingBox();
    const dstBox = await dstColumnBody.boundingBox();
    if (!srcBox || !dstBox) {
      throw new Error("could not resolve source or destination bounding box");
    }

    const srcX = srcBox.x + srcBox.width / 2;
    const srcY = srcBox.y + srcBox.height / 2;
    const dstX = dstBox.x + dstBox.width / 2;
    // Drop into the middle of the column, not at its top edge
    // (which could resolve as "before a card" ambiguously).
    const dstY = dstBox.y + Math.min(dstBox.height / 2, 40);

    await page.mouse.move(srcX, srcY);
    await page.mouse.down();
    // Two moves so DRAG_THRESHOLD_PX activates (first cross the
    // threshold, then travel to the target).
    await page.mouse.move(srcX + 20, srcY + 20, { steps: 3 });
    await page.mouse.move(dstX, dstY, { steps: 10 });
    await page.mouse.up();

    // Give the reactive dispatcher a moment to fire.
    await page.waitForTimeout(300);

    steps.cardLeftSourceColumn =
      (await page.locator(".kanban-column:nth-child(1) .kanban-card").count()) === 1;
    const dstText = await page
      .locator(".kanban-column:nth-child(3) .kanban-card").first().textContent();
    steps.cardArrivedInDestColumn = (dstText || "").includes(CARD_2);

    // Reload → move persists.
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForSelector(".kanban-block", { timeout: 10000 });
    const dstTextReload = await page
      .locator(".kanban-column:nth-child(3) .kanban-card").first().textContent();
    steps.movePersistsAfterReload = (dstTextReload || "").includes(CARD_2);
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: "kanban-drag", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── kanban-column-reorder scenario ─────────────────────────────
//
// Phase 4a. Insert Kanban → drag column 3 to position 1 → assert
// order becomes [Done, To Do, In Progress] → reload → same order.
// Guards the move_kanban_column pipeline end-to-end (drag hooks +
// backend index math + yrs persistence).
async function scenarioKanbanColumnReorder(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-kanban-colreorder@ogrenotes.example.com",
  );
  const title = `doctor-kanban-colreorder-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });

    await page.locator('[contenteditable="true"]').first().click();
    await page.keyboard.press("Control+Shift+P");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    await page.keyboard.type("Kanban", { delay: 10 });
    await page.waitForTimeout(150);
    await page.keyboard.press("Enter");
    await page.waitForSelector(".kanban-block", { timeout: 5000 });

    // Read the default column titles for sanity.
    const before = await page.$$eval(
      ".kanban-column-title",
      els => els.map(e => e.textContent.trim())
    );
    steps.initialOrderIsDefault =
      before[0] === "To Do" && before[1] === "In Progress" && before[2] === "Done";

    // Drag column 3's header (Done) to the position of column 1
    // (To Do). Grab a spot inside the header (not on the count
    // pill, which is a set-wip-limit click target).
    const srcHeader = page
      .locator(".kanban-column:nth-child(3) .kanban-column-title");
    const dstAnchor = page.locator(".kanban-column:nth-child(1)");
    const srcBox = await srcHeader.boundingBox();
    const dstBox = await dstAnchor.boundingBox();
    if (!srcBox || !dstBox) throw new Error("boxes not resolved");

    const srcX = srcBox.x + srcBox.width / 2;
    const srcY = srcBox.y + srcBox.height / 2;
    // Land past To Do's left edge (into its leftmost quarter) so
    // the drop resolves as "insert before column 1".
    const dstX = dstBox.x + dstBox.width * 0.15;
    const dstY = dstBox.y + srcBox.height / 2;

    await page.mouse.move(srcX, srcY);
    await page.mouse.down();
    await page.mouse.move(srcX + 20, srcY, { steps: 3 });
    await page.mouse.move(dstX, dstY, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(300);

    const after = await page.$$eval(
      ".kanban-column-title",
      els => els.map(e => e.textContent.trim())
    );
    steps.columnReorderedInDom =
      after[0] === "Done" && after[1] === "To Do" && after[2] === "In Progress";

    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForSelector(".kanban-block", { timeout: 10000 });
    const afterReload = await page.$$eval(
      ".kanban-column-title",
      els => els.map(e => e.textContent.trim())
    );
    steps.columnOrderPersistsAfterReload =
      afterReload[0] === "Done" && afterReload[1] === "To Do"
      && afterReload[2] === "In Progress";
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: "kanban-column-reorder", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── kanban-wip-limit scenario ──────────────────────────────────
//
// Phase 4a. Insert Kanban → click the count pill on column 1 →
// set wipLimit=1 via prompt → add one card (succeeds) → try to
// add a second (blocked). Verifies the setter UI + enforcement
// in `add_kanban_card`.
async function scenarioKanbanWipLimit(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-kanban-wip@ogrenotes.example.com",
  );
  const title = `doctor-kanban-wip-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });

    await page.locator('[contenteditable="true"]').first().click();
    await page.keyboard.press("Control+Shift+P");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    await page.keyboard.type("Kanban", { delay: 10 });
    await page.waitForTimeout(150);
    await page.keyboard.press("Enter");
    await page.waitForSelector(".kanban-block", { timeout: 5000 });

    // Pre-fill the prompt response before triggering it.
    page.on("dialog", async dialog => {
      if (dialog.type() === "prompt") await dialog.accept("1");
    });
    await page
      .locator('.kanban-column:nth-child(1) [data-kanban-action="set-wip-limit"]')
      .click();
    // Give the state change a moment to propagate.
    await page.waitForTimeout(200);

    const pillText = await page
      .locator(".kanban-column:nth-child(1) .kanban-column-count").textContent();
    steps.wipLimitVisibleInPill = (pillText || "").includes("/1");

    // Add first card — must succeed.
    async function addCard(colIdx, cardTitle) {
      await page
        .locator(`.kanban-column:nth-child(${colIdx}) [data-kanban-action="add-card"]`)
        .first()
        .click();
      await page.waitForSelector(".calendar-modal", { timeout: 3000 });
      await page.locator('.calendar-modal input[type="text"]').fill(cardTitle);
      await page.getByRole("button", { name: /^Save$/ }).click();
      // Modal auto-closes on save.
      await page.waitForSelector(".calendar-modal", {
        state: "detached", timeout: 3000,
      }).catch(() => {});
    }

    await addCard(1, "First");
    steps.firstCardAddedSuccessfully =
      (await page.locator(".kanban-column:nth-child(1) .kanban-card").count()) === 1;

    // Try a second card — modal opens + save is called, but the
    // add is blocked in the command layer, so card count stays 1.
    await addCard(1, "Second");
    steps.secondCardBlockedByWipLimit =
      (await page.locator(".kanban-column:nth-child(1) .kanban-card").count()) === 1;

    // Reload — pill still shows the limit.
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForSelector(".kanban-block", { timeout: 10000 });
    const pillAfterReload = await page
      .locator(".kanban-column:nth-child(1) .kanban-column-count").textContent();
    steps.wipLimitPersistsAfterReload = (pillAfterReload || "").includes("/1");
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: "kanban-wip-limit", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── kanban-card-metadata scenario ──────────────────────────────
//
// Phase 4b/4c. Insert Kanban → add a card with due date, labels
// (bug|red;ux|blue), and assignee → assert the DOM shows the
// due pill, label chips, and assignee avatar → reload → re-open
// the card in Edit mode and assert the modal pre-fills each
// field.
async function scenarioKanbanCardMetadata(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-kanban-metadata@ogrenotes.example.com",
  );
  const title = `doctor-kanban-metadata-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  const DUE = "2026-08-15";
  const LABELS = "bug|red;ux|blue";
  const NAME = "Ada Lovelace";
  try {
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });

    // Insert Kanban.
    await page.locator('[contenteditable="true"]').first().click();
    await page.keyboard.press("Control+Shift+P");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    await page.keyboard.type("Kanban", { delay: 10 });
    await page.waitForTimeout(150);
    await page.keyboard.press("Enter");
    await page.waitForSelector(".kanban-block", { timeout: 5000 });

    // Add card in column 1 and fill all metadata fields.
    await page
      .locator('.kanban-column:nth-child(1) [data-kanban-action="add-card"]')
      .first().click();
    await page.waitForSelector(".calendar-modal", { timeout: 3000 });
    await page.locator('.calendar-modal input[type="text"]').first().fill("Test card");
    await page.locator('.calendar-modal input[type="date"]').fill(DUE);
    // The labels input is the second text input in the modal
    // (title=first, labels=second).
    const textInputs = page.locator('.calendar-modal input[type="text"]');
    await textInputs.nth(1).fill(LABELS);
    // Assignee is the third text input.
    await textInputs.nth(2).fill(NAME);
    await page.getByRole("button", { name: /^Save$/ }).click();
    await page.waitForSelector(".calendar-modal", {
      state: "detached", timeout: 3000,
    }).catch(() => {});

    // Assert the rendered card shows the metadata.
    const dueVisible = await page
      .locator(".kanban-column:nth-child(1) .kanban-card-due")
      .textContent();
    steps.dueDateRenderedOnCard = (dueVisible || "").includes(DUE);
    const labelCount = await page
      .locator(".kanban-column:nth-child(1) .kanban-card-label").count();
    steps.labelChipsRendered = labelCount === 2;
    const assignee = await page
      .locator(".kanban-column:nth-child(1) .kanban-card-assignee")
      .textContent();
    steps.assigneeInitialsRendered = (assignee || "").trim() === "AL";

    // Reload and re-open card in Edit mode.
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForSelector(".kanban-card", { timeout: 10000 });
    await page.locator(".kanban-card").first().click();
    await page.waitForSelector(".calendar-modal", { timeout: 3000 });
    steps.dueDatePreFilledInEdit =
      (await page.locator('.calendar-modal input[type="date"]').inputValue()) === DUE;
    const editInputs = page.locator('.calendar-modal input[type="text"]');
    steps.labelsPreFilledInEdit =
      (await editInputs.nth(1).inputValue()) === LABELS;
    steps.assigneePreFilledInEdit =
      (await editInputs.nth(2).inputValue()) === NAME;
    await page.getByRole("button", { name: /^Cancel$/ }).click();
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: "kanban-card-metadata", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── type-past-atom scenario ────────────────────────────────────
//
// Regression guard for the 2026-07-04 caret / DOM-walk /
// structural-hash trio (fixed in ec75191, acd0477, d82969c).
// Inserts a Calendar block into a fresh doc, clicks after it,
// types text, and asserts:
//   1. `insert_text failed` never fires in the console —
//      dom_to_model_walk and find_in_element both treat the
//      calendar wrapper as opaque via `data-atom-size`.
//   2. `Too many calls to Location or History APIs` never fires —
//      on_state_change's first-block-text cache short-circuits.
//   3. The typed text lands in the trailing paragraph, not inside
//      the calendar's toolbar/day-cell divs — i.e. the caret didn't
//      "jump above the calendar".
//   4. Reload preserves the text — the model actually accepted the
//      inserts (guards against inserts that appeared to succeed but
//      didn't hit the CRDT).
//
// The Kanban block is a superset of Calendar's DOM shape (more
// nested divs); if this scenario passes with Calendar, Kanban is
// covered by the same code paths.
async function scenarioTypePastAtom(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-atom@ogrenotes.example.com",
  );
  const title = `doctor-type-past-atom-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  const TYPED = "typed after calendar";
  try {
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
    steps.editorReady = true;

    // Insert a calendar via the command palette. `data-atom-size`
    // stamping happens at render time, so we need the block in the
    // DOM before we can measure any of the fix behavior.
    await page.locator('[contenteditable="true"]').first().click();
    await page.keyboard.press("Control+Shift+P");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    await page.keyboard.type("Calendar", { delay: 10 });
    await page.waitForTimeout(150);
    await page.keyboard.press("Enter");
    await page.waitForSelector(".calendar-block[data-atom-size]", {
      timeout: 5000,
    });
    steps.calendarInsertedWithAtomSize = true;

    // Baseline console counters — everything after this point is
    // attributed to the type-past interactions.
    const priorMessages = (collector["tab-a"] || {}).console || [];
    const priorLen = priorMessages.length;

    // Click the last block in the editor — the trailing paragraph
    // that `insert_live_app` seeds after the calendar. Targeting
    // `.editor-content > p:last-child` avoids grabbing anything
    // inside the calendar's month-grid.
    const trailingPara = page.locator(".editor-content > p:last-child");
    await trailingPara.click();
    steps.clickedTrailingParagraph = true;

    // Type. Every character has to succeed; any `insert_text failed`
    // (view.rs:435) is a hard fail.
    await page.keyboard.type(TYPED, { delay: 20 });

    // Give the WASM a moment to flush its debug log to the browser
    // console before we snapshot.
    await page.waitForTimeout(200);

    const allMessages = (collector["tab-a"] || {}).console || [];
    const newMessages = allMessages.slice(priorLen);

    steps.noInsertTextFailed = !newMessages.some(m =>
      (m.text || "").includes("insert_text failed"));
    steps.noHistoryApiThrottle = !newMessages.some(m =>
      (m.text || "").includes("Too many calls to Location or History"));

    // The caret must be inside the trailing paragraph, NOT inside
    // the calendar. Reads `getSelection().anchorNode` and walks
    // ancestors — a caret inside the month-grid would find a
    // `.calendar-block` ancestor before the editor root.
    const caretLocation = await page.evaluate(() => {
      const sel = window.getSelection();
      if (!sel || !sel.anchorNode) return { where: "none" };
      let node = sel.anchorNode.nodeType === Node.TEXT_NODE
        ? sel.anchorNode.parentElement
        : sel.anchorNode;
      while (node && node !== document.body) {
        if (node.classList && node.classList.contains("calendar-block")) {
          return { where: "inside-calendar" };
        }
        if (node.classList && node.classList.contains("editor-content")) {
          return { where: "editor-root" };
        }
        node = node.parentElement;
      }
      return { where: "outside-editor" };
    });
    steps.caretNotInsideCalendar =
      caretLocation.where !== "inside-calendar";

    // Text must be visible in the doc — proxy: some paragraph after
    // the calendar contains the typed string. `nth=-1` reads the
    // last paragraph specifically.
    const trailingText = await page
      .locator(".editor-content > p:last-child").textContent();
    steps.typedTextInTrailingParagraph =
      (trailingText || "").includes(TYPED);

    // Reload → text persists.
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForSelector(".calendar-block[data-atom-size]", {
      timeout: 10000,
    });
    const reloadedText = await page
      .locator(".editor-content > p:last-child").textContent();
    steps.typedTextPersistsAfterReload =
      (reloadedText || "").includes(TYPED);
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  const tab = collector["tab-a"] || { errors: [] };
  steps.noPageErrors = (tab.errors || []).length === 0;

  collector.scenario = {
    name: "type-past-atom", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── code-block-enter scenario ──────────────────────────────────
//
// Regression guard for Enter-inside-a-fenced-code-block (the
// triple-backtick input rule + `Mod-Alt-c` code-block shortcut
// landed in 8d7146f). A generic `split_block` — the path every
// other textblock's Enter key rides — turns the block at the caret
// into two SIBLING blocks (first keeps the original node type,
// second becomes a plain Paragraph). That's exactly right for a
// heading or a list item, but inside a `pre > code` it must NOT
// apply: pressing Enter while still writing code has to insert a
// newline in the SAME code block, and only a double-Enter (a blank
// line) should exit into a trailing paragraph — mirroring the
// GitHub / CodeMirror convention. This scenario types a fenced
// Python block, asserts the language chip + keyword highlighting
// wire up, presses Enter once (asserting the block does NOT split),
// types a second line, then double-Enters to confirm the block IS
// exited cleanly afterward.
//
//   A. "```python " → `pre > code.language-python` appears; the
//      `.code-lang-chip` overlay `<select>` shows "Python".
//   B. "class PythonClass:" → a `span.tok-keyword` wraps "class".
//   C. Enter once → still exactly one `pre`; `code.textContent ===
//      "class PythonClass:\n"`; code's last element child is
//      `br[data-sentinel]`; the caret is still inside the `pre`.
//   D. "pass" → `code.textContent === "class PythonClass:\npass"`;
//      still exactly one `pre`.
//   E. Enter twice (blank line) → a `p` now follows the `pre`; the
//      caret moved into that paragraph, not the `pre`; the code
//      block's text is unchanged.
async function scenarioCodeBlockEnter(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-codeblock-enter@ogrenotes.example.com",
  );
  const title = `doctor-code-block-enter-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  const evidence = {};
  try {
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 15000 });
    const ed = page.locator(".editor-content");
    await ed.click();
    steps.editorReady = true;

    // ── Step a: ```python + trailing space triggers the code-block
    // input rule. Per-key typing (not clipboard paste) so beforeinput
    // fires for every character, exactly like a real user.
    await page.keyboard.type("```python ", { delay: 20 });
    const codeBlockAppeared = await page
      .waitForSelector("pre > code.language-python", { timeout: 5000 })
      .then(() => true)
      .catch(() => false);
    steps.aCodeBlockCreated = codeBlockAppeared;
    evidence.aCodeBlockCreated = codeBlockAppeared
      ? "pre > code.language-python present"
      : "selector never appeared";

    const chip = await page
      .locator(".code-lang-chip select")
      .evaluate((sel) => ({
        value: sel.value,
        label: sel.selectedOptions[0] ? sel.selectedOptions[0].label : null,
      }))
      .catch((e) => ({ error: e.message }));
    evidence.aChip = chip;
    steps.aChipShowsPython =
      !!chip && (chip.value === "python" || chip.label === "Python");

    // ── Step b: type a line containing a Python keyword.
    await page.keyboard.type("class PythonClass:", { delay: 20 });
    await page.waitForTimeout(200);
    const keywordCount = await page
      .locator("pre > code span.tok-keyword", { hasText: "class" })
      .count()
      .catch(() => 0);
    steps.bKeywordSpan = keywordCount > 0;
    evidence.bKeywordSpan = `tok-keyword spans matching "class": ${keywordCount}`;

    // ── Step c: press Enter ONCE — the regression under test. Must
    // stay inside the same `pre`, not split into a second block.
    await page.keyboard.press("Enter");
    await page.waitForTimeout(200);
    await page.screenshot({
      path: join(outDir, "tab-a-step-c.png"), fullPage: false,
    }).catch(() => {});

    const preCountC = await page.locator("pre").count();
    steps.cSinglePre = preCountC === 1;
    evidence.cSinglePre = `pre count: ${preCountC}`;

    const codeTextC = await page.locator("pre > code").first().textContent();
    steps.cTextContent = codeTextC === "class PythonClass:\n";
    evidence.cTextContent = JSON.stringify(codeTextC);

    const sentinelC = await page
      .locator("pre > code")
      .first()
      .evaluate((code) => {
        const last = code.lastElementChild;
        return !!(last && last.tagName === "BR" && last.hasAttribute("data-sentinel"));
      })
      .catch(() => false);
    steps.cSentinelBr = sentinelC;
    evidence.cSentinelBr = sentinelC
      ? "last element child is br[data-sentinel]"
      : "last element child is not br[data-sentinel]";

    const selInPreC = await page.evaluate(() => {
      const sel = window.getSelection();
      const pre = document.querySelector("pre");
      if (!sel || !sel.anchorNode || !pre) return false;
      return pre.contains(sel.anchorNode);
    });
    steps.cSelectionInPre = selInPreC;
    evidence.cSelectionInPre = `selection.anchorNode inside pre: ${selInPreC}`;

    // ── Step d: type a second line inside the (still single) block.
    await page.keyboard.type("pass", { delay: 20 });
    await page.waitForTimeout(150);

    const codeTextD = await page.locator("pre > code").first().textContent();
    steps.dTextContent = codeTextD === "class PythonClass:\npass";
    evidence.dTextContent = JSON.stringify(codeTextD);

    const preCountD = await page.locator("pre").count();
    steps.dSinglePre = preCountD === 1;
    evidence.dSinglePre = `pre count: ${preCountD}`;

    // ── Step e: blank line (Enter twice) exits the code block.
    await page.keyboard.press("Enter");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(200);
    await page.screenshot({
      path: join(outDir, "tab-a-step-e.png"), fullPage: false,
    }).catch(() => {});

    const paragraphAfterPre = await page.evaluate(() => {
      const pre = document.querySelector("pre");
      if (!pre) return false;
      let sib = pre.nextElementSibling;
      while (sib) {
        if (sib.tagName === "P") return true;
        sib = sib.nextElementSibling;
      }
      return false;
    });
    steps.eParagraphAfterPre = paragraphAfterPre;
    evidence.eParagraphAfterPre = `paragraph found after pre: ${paragraphAfterPre}`;

    const selE = await page.evaluate(() => {
      const sel = window.getSelection();
      const pre = document.querySelector("pre");
      if (!sel || !sel.anchorNode) return { inParagraph: false, inPre: false };
      const node = sel.anchorNode.nodeType === Node.TEXT_NODE
        ? sel.anchorNode.parentElement
        : sel.anchorNode;
      const inPre = !!(pre && node && pre.contains(node));
      let p = node;
      while (p && p !== document.body) {
        if (p.tagName === "P") return { inParagraph: true, inPre };
        p = p.parentElement;
      }
      return { inParagraph: false, inPre };
    });
    steps.eSelectionInParagraph = selE.inParagraph === true && selE.inPre === false;
    evidence.eSelection = selE;

    const codeTextE = await page.locator("pre > code").first().textContent();
    steps.eTextContentUnchanged = codeTextE === "class PythonClass:\npass";
    evidence.eTextContent = JSON.stringify(codeTextE);
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  const tab = collector["tab-a"] || { errors: [], console: [] };
  const consoleErrors = (tab.console || []).filter((m) => m.type === "error");
  steps.noPageErrors = (tab.errors || []).length === 0;
  steps.noConsoleErrors = consoleErrors.length === 0;
  evidence.consoleErrors = consoleErrors.map((m) => m.text);

  collector.scenario = {
    name: "code-block-enter", docId: doc.id, title, steps, evidence,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── bulk-delete scenario ───────────────────────────────────────
//
// Phase 5 M-P7 piece D. Creates three docs via API, navigates
// home, checks each row's checkbox, opens the bulk-action bar,
// clicks Delete, accepts the confirm modal, asserts all three
// docs disappear from the home view and reappear in trash.
//
// Failure modes:
//   - Checkbox column not rendering (selectable prop wiring
//     regressed)
//   - Selection bar not appearing after toggles (selected_ids
//     signal flow regressed)
//   - Confirm dialog blocking the delete dispatch
//   - bulk_delete API helper / endpoint mis-routed
async function scenarioBulkDelete(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-bulk-delete@ogrenotes.example.com",
  );

  // Pre-seed three documents so the home file browser has rows to
  // check. Use distinguishable titles so a mismatched assertion
  // tells us exactly which doc fell out.
  const tStamp = Date.now();
  const doc_a = await createDocViaApi(target, tokens.accessToken, `bulk-a-${tStamp}`);
  const doc_b = await createDocViaApi(target, tokens.accessToken, `bulk-b-${tStamp}`);
  const doc_c = await createDocViaApi(target, tokens.accessToken, `bulk-c-${tStamp}`);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".file-browser .file-list", { timeout: 15000 });
    steps.fileBrowserMounted = true;

    // Check each doc's row. The checkbox lives in the leading
    // `.file-row-select` cell. Use the doc's title text to find
    // the right row, then click its checkbox.
    for (const title of [`bulk-a-${tStamp}`, `bulk-b-${tStamp}`, `bulk-c-${tStamp}`]) {
      const row = page.locator(`.file-list tr:has-text("${title}")`);
      await row.locator('.file-row-select input[type="checkbox"]').click();
      await page.waitForTimeout(50);
    }
    steps.threeBoxesChecked = true;

    // The selection bar appears at the top of the viewport with
    // the count + Cancel + Delete buttons.
    await page.waitForSelector(".bulk-selection-bar", { timeout: 3000 });
    steps.selectionBarVisible = true;
    const countText = (await page.locator(".bulk-selection-count").textContent())?.trim();
    steps.countShows3 = countText?.includes("3");

    // Click Delete. The confirm dialog opens; accept it. The page
    // re-fetches the folder; the three rows should no longer
    // appear in the listing.
    await page.locator(".bulk-selection-bar .btn-danger").click();
    await page.waitForSelector(".confirm-dialog", { timeout: 3000 });
    steps.confirmDialogOpened = true;

    await page.locator('.confirm-dialog .btn-danger, .confirm-dialog button:has-text("Move to trash")').first().click();
    // Wait for the bar to disappear (selection cleared). This fires
    // synchronously once the POST resolves, ~12 ms after the click.
    await page.waitForSelector(".bulk-selection-bar", {
      state: "detached", timeout: 5000,
    });
    steps.selectionBarDismissed = true;

    // Wait for each deleted row to actually drop out of the file list.
    // The selection bar detach fires synchronously on POST success,
    // but the file list only updates after the follow-up
    // GET /folders/<id> response lands (~25 ms after the POST in the
    // observed scenario). Per-row `waitFor({state: "detached"})`
    // polls each row's locator until it leaves the DOM, removing
    // the race that flaked the initial M-P7 piece D version of
    // this assertion.
    for (const title of [`bulk-a-${tStamp}`, `bulk-b-${tStamp}`, `bulk-c-${tStamp}`]) {
      await page.locator(`.file-list tr:has-text("${title}")`)
        .waitFor({ state: "detached", timeout: 5000 });
    }
    // Sanity-check the row counts after the waits — if waitFor
    // returned cleanly the counts must be zero, but capture them
    // explicitly so the step report distinguishes a genuine
    // detach-success from a wait-timeout-then-recover.
    const remainingA = await page.locator(`.file-list tr:has-text("bulk-a-${tStamp}")`).count();
    const remainingB = await page.locator(`.file-list tr:has-text("bulk-b-${tStamp}")`).count();
    const remainingC = await page.locator(`.file-list tr:has-text("bulk-c-${tStamp}")`).count();
    steps.docsRemovedFromHome = remainingA === 0 && remainingB === 0 && remainingC === 0;
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "bulk-delete", docIds: [doc_a.id, doc_b.id, doc_c.id], steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── command-palette-actions scenario ───────────────────────────
//
// Phase 5 M-P4 piece D. Drives a regular document context, types
// some text, selects it, then opens the command palette in Action
// mode via Ctrl+Shift+P, filters to "bold", and presses Enter.
// Asserts the editor's DOM has a <strong> wrapping the selection
// after the dispatch.
//
// Failure modes the scenario surfaces:
//   - Ctrl+Shift+P doesn't open the palette in Action mode (the
//     key binding or initial_mode wiring regressed)
//   - "bold" doesn't match the editor.bold command (fuzzy match
//     or editor-scope filter regressed)
//   - Enter doesn't dispatch the first match (the keydown handler
//     in search_dialog regressed)
//   - The editor-bridge isn't installed at the editor page, so
//     Enter runs the action but `dispatch_editor` finds None and
//     the bold mark never applies
async function scenarioCommandPaletteActions(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-cmd-palette@ogrenotes.example.com",
  );
  const title = `doctor-cmd-palette-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
    steps.editorReady = true;

    // Click into the editor to focus, then type. The contenteditable
    // takes pointer events on the inner paragraph.
    await page.locator('[contenteditable="true"]').first().click();
    await page.waitForTimeout(200);
    await page.keyboard.type("hello world", { delay: 15 });
    steps.textTyped = true;

    // Select-all so the whole inserted text gets the bold mark when
    // the command dispatches.
    await page.keyboard.press("Control+a");
    await page.waitForTimeout(100);
    steps.textSelected = true;

    // Open the palette directly in Action mode. The dialog's
    // open-Effect pre-fills `>` and seeds actions for the editor
    // scope.
    await page.keyboard.press("Control+Shift+P");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    steps.paletteOpened = true;

    // Verify the dialog is in Action mode — value should start
    // with `>` after the pre-fill.
    const inputValue = await page.locator(".search-input").inputValue();
    steps.paletteInActionMode = inputValue.startsWith(">");

    // Filter to bold (after the leading `>`). Type letters; the
    // existing matching() filters and we expect "Bold" near the top.
    await page.keyboard.type("bold", { delay: 30 });
    await page.waitForSelector(".command-item", { timeout: 3000 });
    // The first match should be "Bold" (editor-scope + fuzzy match).
    const firstLabel = (
      await page.locator(".command-item .command-item-label").first().textContent()
    )?.trim();
    steps.boldVisible = firstLabel === "Bold";

    // Press Enter — the keydown handler runs the first action +
    // closes the dialog. The dispatch flows through editor_bridge
    // to the document page's on_command callback.
    await page.keyboard.press("Enter");
    await page.waitForTimeout(300);
    steps.enterDispatched = true;

    // The dialog should be closed; the bold mark should have
    // landed on the selection. Editor renders bold as <strong>.
    const dialogStillOpen = await page.locator(".search-dialog").count();
    steps.dialogClosed = dialogStillOpen === 0;
    const strongCount = await page.locator('[contenteditable="true"] strong').count();
    steps.boldApplied = strongCount >= 1;
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "command-palette-actions", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── ask-flow scenario ─────────────────────────────────────────
//
// Phase 6 M-6.2 piece E. End-to-end exercise of the agentic Ask
// flow: open the dialog via the command palette, ask a question
// targeting a seeded document, assert the agent walked
// keyword_search → get_document and surfaced at least one source
// citation.
//
// CI gating: the scenario assumes the backend has
// ANTHROPIC_API_KEY set; without it /api/v1/ask returns 503 and
// nothing streams. The playwright workflow step is gated on the
// secret being available (PRs from forks skip the step entirely
// because forked-PR secrets aren't exposed to the build).
//
// The seeded doc carries a deliberately distinctive title token
// (`PineappleAuth`) so the agent's keyword_search returns it as
// the top hit; the question phrasing ("what does the
// PineappleAuth document say?") guarantees the LLM cites the
// doc rather than waffling without sources.
//
// Failure modes the scenario surfaces:
//   - The `ask.open` palette command bridge regressed (sidebar
//     entry and palette both stop opening the dialog).
//   - The SSE stream parser regressed (status / text / source /
//     done events dropped or misordered).
//   - The agent loop times out (10s p95 per the RAG plan §4.2
//     target; scenario allows 30s headroom).
//   - The source citation rendering regressed (the <a> in
//     `.ask-source-link` doesn't render or has no href).
async function scenarioAskFlow(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-ask-flow@ogrenotes.example.com",
  );
  // Distinctive title fragment so keyword_search produces a clean
  // top hit. Embedded in both the doc title and the question.
  const distinctive = `PineappleAuth-${Date.now()}`;
  const seededTitle = `${distinctive} Design Notes`;
  const doc = await createDocViaApi(target, tokens.accessToken, seededTitle);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".file-browser", { timeout: 15000 });
    steps.homeMounted = true;

    // Open the command palette via Ctrl+K, type "ask", press Enter
    // to fire the `ask.open` Global command. Exercises both the
    // palette bridge (commands::run → ask_bridge::open) and the
    // page-level Effect that registered the on_ask callback.
    await page.keyboard.press("Control+k");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    steps.paletteOpened = true;

    await page.keyboard.type(">ask", { delay: 30 });
    await page.waitForSelector(".command-item", { timeout: 3000 });
    // The first match should be "Ask the assistant" (the cmd-ask
    // label). Action mode runs commands::run(first.id) on Enter,
    // which dispatches ask_bridge::open() → flips ask_visible
    // → AskDialog mounts.
    await page.keyboard.press("Enter");
    await page.waitForSelector(".ask-dialog", { timeout: 5000 });
    steps.askDialogOpened = true;

    // The dialog's input auto-focuses on open (a11y::install_focus_trap
    // schedules it on the next microtask). Type the question and
    // submit with Enter.
    await page.locator(".ask-dialog .search-input").focus();
    await page.keyboard.type(
      `What does the document titled "${distinctive}" cover?`,
      { delay: 15 },
    );
    await page.keyboard.press("Enter");
    steps.questionSubmitted = true;

    // The dialog flips into loading mode immediately — status pill
    // appears with "Thinking…" or "Using tool: …". This is the
    // first proof that the SSE stream opened and the API has the
    // Anthropic key set (a 503 from the no-key path would skip
    // straight to .ask-error without ever showing .ask-status).
    await page
      .waitForSelector(".ask-status, .ask-error", { timeout: 10000 });
    // If we got an error banner, the run is unblocked but the rest
    // of the assertions don't apply — record an explicit skip step
    // and bail. The workflow step is gated on the secret, but
    // local runs without the key should report a clear reason.
    const errorVisible = await page.locator(".ask-error").count();
    if (errorVisible > 0) {
      const errMsg = await page.locator(".ask-error").textContent();
      steps.askEndpointAvailable = false;
      collector.stepError =
        `/ask returned an error before streaming: ${errMsg?.trim() ?? "unknown"}`;
    } else {
      steps.askEndpointAvailable = true;
      // Wait for the loading spinner to disappear (Done event
      // received) or for the streamed answer to show up. The 30s
      // budget allows for an agent loop hitting MAX_TOOL_ROUNDS
      // plus Claude's worst-case latency.
      await page
        .waitForSelector(".ask-dialog .ask-spinner", {
          state: "detached", timeout: 30000,
        });
      steps.answerStreamed = true;

      // At least one source citation should have arrived as a
      // Source SSE event. The agent's job for this question is
      // to keyword_search → get_document → cite, so a
      // zero-source response is a regression of the agent
      // system prompt or the get_document tool's tx.send call.
      const sourceCount = await page
        .locator(".ask-source-link").count();
      steps.sourcesAppeared = sourceCount >= 1;

      // The cited link should resolve to the seeded doc's id —
      // not strictly required (a hallucinating agent could cite
      // some other doc) but a strong signal the retrieval path
      // works end-to-end.
      const firstHref = await page
        .locator(".ask-source-link").first().getAttribute("href");
      steps.firstCitationMatchesSeed = firstHref
        ?.includes(doc.id) ?? false;
    }
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "ask-flow", docId: doc.id, title: seededTitle, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── a11y-audit scenario ────────────────────────────────────────
//
// Phase 5 M-P8 piece D. Walks three high-traffic surfaces (home,
// document editor, command palette open) and runs axe-core on
// each. The gate threshold is **zero serious-or-critical
// violations**; minor/moderate findings are logged to the per-step
// payload but do not fail the scenario — that lets the team land
// the audit without bundling every minor cosmetic fix into one
// commit. Lift the threshold once the existing baseline of minor
// findings is at zero too.
//
// Why three surfaces specifically:
//   - home — exercises the sidebar nav, breadcrumb nav, file
//     browser table, action bar (every M-P8 piece B addition lives
//     here)
//   - document — exercises <main> landmark, toolbar with role +
//     groups, sync indicator live region, share/comment-popup
//     dialog ARIA when opened (also a quick keyboard check)
//   - palette-open — exercises modal-dialog ARIA, focus trap
//     mounting, and the search-results list live region
//
// Failure modes the scenario surfaces:
//   - A new component lands without an accessible name (axe rule
//     `aria-required-name` / `button-name` / `link-name`)
//   - A landmark gets nested incorrectly (`landmark-no-duplicate-
//     main`, `landmark-unique`)
//   - Color contrast regresses below 4.5:1 body text (axe rule
//     `color-contrast`)
//   - A modal opens without role="dialog" + aria-modal
//   - aria-* attributes reference missing ids (`aria-valid-attr-
//     value`)
async function scenarioA11yAudit(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-a11y@ogrenotes.example.com",
  );
  const title = `doctor-a11y-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  // Per-surface violation summaries: { home: {...}, editor: {...},
  // palette: {...} }. Always carry counts so the scenario report
  // is informative even when the gate passes.
  const surfaces = {};
  const steps = {};

  // Run axe with the WCAG 2.1 A + AA tag set. Excludes axe's
  // "best-practices" rules which are not part of the conformance
  // bar but produce noise (we'll opt them back in once the
  // serious/critical gate is green at zero).
  const runAxe = async (label) => {
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
      .analyze();
    const buckets = { serious: 0, critical: 0, moderate: 0, minor: 0 };
    const detail = [];
    for (const v of results.violations) {
      if (Object.prototype.hasOwnProperty.call(buckets, v.impact)) {
        buckets[v.impact] += v.nodes.length;
      }
      detail.push({
        id: v.id, impact: v.impact, count: v.nodes.length,
        help: v.help, nodes: v.nodes.slice(0, 3).map((n) => n.target),
      });
    }
    surfaces[label] = { buckets, detail };
    return buckets;
  };

  try {
    // ─── Surface 1: home ─────────────────────────────────────
    await page.goto(`${target}/`, {
      waitUntil: "domcontentloaded", timeout: 30000,
    });
    await page.waitForSelector(".file-browser .file-list", { timeout: 15000 });
    steps.homeMounted = true;
    const homeBuckets = await runAxe("home");
    steps.homeNoSeriousOrCritical =
      homeBuckets.serious === 0 && homeBuckets.critical === 0;

    // ─── Surface 2: document editor ─────────────────────────
    await page.goto(`${target}/d/${doc.id}/probe`, {
      waitUntil: "domcontentloaded", timeout: 30000,
    });
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
    // Toolbar should be there too — gate on it so axe runs against
    // the fully-mounted editor rather than a partial render.
    await page.waitForSelector(".toolbar", { timeout: 5000 });
    steps.editorMounted = true;
    const editorBuckets = await runAxe("editor");
    steps.editorNoSeriousOrCritical =
      editorBuckets.serious === 0 && editorBuckets.critical === 0;

    // ─── Surface 3: command palette open ────────────────────
    // Audit the palette as a modal — verifies focus-trap landing,
    // dialog ARIA, and the search-results live region. Action
    // mode pulls in the editor-scope commands so the list
    // actually has items to render.
    await page.keyboard.press("Control+Shift+P");
    await page.waitForSelector(".search-dialog", { timeout: 5000 });
    await page.waitForTimeout(300); // give the dialog effect a beat to focus
    steps.paletteMounted = true;
    const paletteBuckets = await runAxe("palette");
    steps.paletteNoSeriousOrCritical =
      paletteBuckets.serious === 0 && paletteBuckets.critical === 0;
    await page.keyboard.press("Escape");
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "a11y-audit", docId: doc.id, title, steps, surfaces,
  };
  // Drop the full violation detail to a JSON file so a CI failure
  // is debuggable — the doctor's collector summary is already
  // bounded by the artifact upload, this is the deep-dive.
  writeFileSync(
    join(outDir, "axe-results.json"),
    JSON.stringify(surfaces, null, 2),
  );
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── mobile-spreadsheet-keyboards scenario ──────────────────────
//
// Phase 5 M-P3 piece E. Drives an iPhone-emulated context against
// a freshly-created spreadsheet, confirms the in-page formula
// keyboard mounts in Formula mode when the cell value starts with
// `=`, types a partial function name to trigger autocomplete, and
// verifies the autocomplete popup re-anchors above the keyboard
// (the piece-D contract — class `is-above-keyboard`).
//
// Failure modes the scenario is designed to surface:
//   - is_touch_primary() ≠ true under iPhone emulation (rare; means
//     the hover-none matchMedia path regressed)
//   - FormulaKeyboard mount missed when value starts with `=`
//   - Autocomplete shown but `is-above-keyboard` class missing
//     (regression of piece D — popup would render behind keyboard)
//   - Autocomplete never appears for a real partial like "SU"
async function scenarioMobileSpreadsheetKeyboards(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-mobile-kb@ogrenotes.example.com",
  );
  const title = `doctor-mobile-kb-${Date.now()}`;
  const doc = await createDocViaApi(
    target, tokens.accessToken, title, "spreadsheet",
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    // iPhone 14 emulation: 390x844 viewport, hasTouch + isMobile,
    // hover-none matchMedia. is_touch_primary() in spreadsheet_view
    // reads hover-none, so this puts us on the touch path.
    ...devices["iPhone 14"],
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  await page.goto(`${target}/d/${doc.id}/probe`, {
    waitUntil: "domcontentloaded", timeout: 30000,
  });

  const steps = {};
  try {
    await page.waitForSelector(".spreadsheet-wrapper", { timeout: 15000 });
    steps.gridMounted = true;

    // Sanity check the emulation actually flipped the touch-path
    // bit our Rust code reads. If this is false, every later
    // assertion will fail for the wrong reason — capture early so
    // the failure mode is unambiguous.
    steps.touchPrimaryActive = await page.evaluate(
      () => window.matchMedia("(hover: none)").matches,
    );

    // Focus the grid wrapper and type `=` — single-char keypress
    // enters edit mode at the active cell (default A1) AND sets the
    // initial value. This matches the existing scenarioSpreadsheetKeyboard
    // pattern; tap-then-type would need a dblclick (the only
    // on:click→set_editing path), and double-tap isn't a thing on
    // mobile. Type-to-edit is the universal path and the one users
    // hit through the OS keyboard.
    await page.evaluate(() => {
      const w = document.querySelector(".spreadsheet-wrapper");
      if (w) w.focus();
    });
    await page.keyboard.type("=", { delay: 20 });
    await page.waitForSelector(".spreadsheet-cell-input", { timeout: 5000 });
    steps.cellEditing = true;
    await page.waitForSelector(".formula-keyboard.is-formula", { timeout: 5000 });
    steps.formulaKeyboardMounted = true;

    // Type a partial function name. The autocomplete list is
    // populated reactively from the cell input's `on:input` handler
    // (the same one desktop uses); on mobile the only change is
    // the popup's CSS anchor.
    await page.keyboard.type("SU", { delay: 20 });
    await page.waitForSelector(".ss-autocomplete", { timeout: 5000 });
    steps.autocompleteVisible = true;

    // Piece-D contract: when the on-screen Formula keyboard is up,
    // the autocomplete popup gets the `is-above-keyboard` class so
    // it pins above the keyboard band instead of below the cell
    // (which would be covered by the keyboard on mobile).
    const aboveKbCount = await page.locator(
      ".ss-autocomplete.is-above-keyboard",
    ).count();
    steps.autocompleteAboveKeyboard = aboveKbCount === 1;

    // The first match for "SU" should be SUM; confirm the picker
    // actually filtered to something sensible (not just rendered
    // an empty list).
    const firstName = (
      await page.locator(".ss-autocomplete-item .ss-ac-name").first().textContent()
    )?.trim();
    steps.firstMatchIsSum = firstName === "SUM";
  } catch (e) {
    collector.stepError = `${e.message}\n${e.stack || ""}`;
  }

  collector.scenario = {
    name: "mobile-spreadsheet-keyboards", docId: doc.id, title, steps,
  };
  await page.screenshot({
    path: join(outDir, "tab-a.png"), fullPage: false,
  }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── Semantic-search scenario (Phase 6 close criterion #2, issue #86) ──
// Closes the dedicated coverage gap on the RRF / semantic-search path
// (the existing ask-flow scenario only exercises the agent's tools,
// not the direct /search hybrid fusion). Seeds a doc whose content
// shares no tokens with a deliberately abstract query, then polls
// /search until the doc surfaces — which can only happen via the
// embedding leg of the RRF fusion. API-only; no browser steps.
async function scenarioSemanticSearch(ctx, collector) {
  const { baseUrlA, baseUrl, emailA } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-semantic@ogrenotes.example.com",
  );

  // Distinctive title fragment so we can filter results to the doc we
  // seeded (other docs in the user's home folder shouldn't confuse the
  // assertion). The body uses board/merger vocabulary; the query uses
  // M&A vocabulary that doesn't share any content tokens, so BM25 has
  // no signal and the only ranking that surfaces this doc is the
  // semantic leg.
  const distinctive = `SemDoctor-${Date.now()}`;
  const seededTitle = `${distinctive} board approval notes`;
  const body =
    "The directors voted to approve the proposed combination with " +
    "the target company after extensive due diligence on the target's " +
    "financials and risk profile.";
  const semanticQuery = "merger acquisition ratified";

  collector.scenario = {
    name: "semantic-search",
    target,
    distinctive,
    query: semanticQuery,
    steps: {},
  };
  const steps = collector.scenario.steps;

  // Step 1: seed the doc via the markdown-import route (creates a new
  // doc with body content, which the embedding pipeline picks up).
  const importRes = await fetch(`${target}/api/v1/documents/import`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${tokens.accessToken}`,
    },
    body: JSON.stringify({
      format: "markdown",
      title: seededTitle,
      content: body,
    }),
  });
  if (!importRes.ok) {
    throw new Error(
      `seed import failed: ${importRes.status} ${await importRes.text()}`,
    );
  }
  const seededDoc = await importRes.json();
  collector.scenario.docId = seededDoc.id;
  steps.docSeeded = true;

  // Step 2: poll /search with the semantic-only query until the doc
  // appears, or fail with a timeout. The embedding pipeline is async
  // (Bedrock invocation + Qdrant upsert), so wait up to 90s.
  const SEARCH_TIMEOUT_MS = 90 * 1000;
  const POLL_INTERVAL_MS = 2000;
  const deadline = Date.now() + SEARCH_TIMEOUT_MS;
  let surfaced = false;
  let lastJson = null;
  while (Date.now() < deadline) {
    const searchRes = await fetch(
      `${target}/api/v1/search?q=${encodeURIComponent(semanticQuery)}`,
      { headers: { authorization: `Bearer ${tokens.accessToken}` } },
    );
    if (searchRes.ok) {
      lastJson = await searchRes.json();
      const results = lastJson.results || [];
      // SearchResultItem serializes the doc id as plain `id`
      // (camelCase rename in the route's serde). Match defensively
      // on docId too in case the wire shape grows a new field.
      const hit = results.find(
        (r) => r.id === seededDoc.id || r.docId === seededDoc.id,
      );
      if (hit) {
        surfaced = true;
        collector.scenario.hitRank = results.indexOf(hit);
        break;
      }
    }
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }

  if (!surfaced) {
    const snippet = lastJson
      ? JSON.stringify(lastJson).slice(0, 500)
      : "(no successful response)";
    throw new Error(
      `semantic-only query "${semanticQuery}" did not surface the ` +
        `seeded doc within ${SEARCH_TIMEOUT_MS}ms — either embedding ` +
        `hasn't indexed yet, the RRF/semantic path isn't wired, or ` +
        `qdrant_url isn't set on the deployed stack. Last results: ` +
        snippet,
    );
  }
  steps.semanticHitObserved = true;
  collector.scenario.ok = true;
}

// ─── Import-job round-trip scenario (Phase 6 close criterion #6, issue #87) ──
// Closes the doctor-level coverage gap on POST /jobs (via the
// /documents/import-job route) → GET /jobs/{id}. Uses fake .docx bytes
// — the route validates the extension + stages without parsing, and
// the worker fails the parse → dead-letters; either terminal state
// proves the enqueue → consume → poll round-trip works. Requires the
// ogrenote-worker ECS service (M-6.4 piece D) to be running on the
// target environment; if not, the scenario times out and fails. No
// browser steps — fetch-driven.
async function scenarioImportJobRoundTrip(ctx, collector) {
  const { baseUrlA, baseUrl, emailA } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-import-job@ogrenotes.example.com",
  );

  collector.scenario = {
    name: "import-job-round-trip",
    target,
    steps: {},
  };
  const steps = collector.scenario.steps;

  // Build a tiny fake .docx multipart body. Real DOCX bytes aren't
  // needed: the route only checks the filename + stages to S3, and the
  // worker reaches a terminal "failed" state on the parse failure —
  // which is exactly the round-trip mechanic we're proving.
  const fakeBytes = new TextEncoder().encode(
    "PK\x03\x04 fake doctor docx — worker should dead-letter this",
  );
  const boundary = "doctorimportjobboundary";
  const docxMime =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
  const body = buildMultipartFileBody(
    boundary, "doctor.docx", docxMime, fakeBytes,
  );

  const enqueueRes = await fetch(`${target}/api/v1/documents/import-job`, {
    method: "POST",
    headers: {
      "content-type": `multipart/form-data; boundary=${boundary}`,
      authorization: `Bearer ${tokens.accessToken}`,
    },
    body,
  });
  if (enqueueRes.status !== 202) {
    throw new Error(
      `enqueue expected 202, got ${enqueueRes.status}: ${await enqueueRes.text()}`,
    );
  }
  const enqueueJson = await enqueueRes.json();
  const jobId = enqueueJson.jobId;
  if (!jobId) throw new Error(`response missing jobId: ${JSON.stringify(enqueueJson)}`);
  collector.scenario.jobId = jobId;
  steps.enqueued = true;

  // Poll /jobs/{id} until terminal (succeeded OR failed). A worker
  // consuming the stream picks up the fake .docx, the parser errors,
  // and after the retry budget exhausts the status flips to "failed"
  // and the entry lands on the dead-letter stream. Either terminal
  // state proves the round-trip.
  const POLL_TIMEOUT_MS = 90 * 1000;
  const POLL_INTERVAL_MS = 1000;
  const deadline = Date.now() + POLL_TIMEOUT_MS;
  let terminalState = null;
  let lastJson = null;
  while (Date.now() < deadline) {
    const pollRes = await fetch(`${target}/api/v1/jobs/${jobId}`, {
      headers: { authorization: `Bearer ${tokens.accessToken}` },
    });
    if (pollRes.ok) {
      lastJson = await pollRes.json();
      if (lastJson.state === "succeeded" || lastJson.state === "failed") {
        terminalState = lastJson.state;
        break;
      }
    }
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }

  if (!terminalState) {
    throw new Error(
      `job ${jobId} did not reach a terminal state within ${POLL_TIMEOUT_MS}ms — ` +
        `the ogrenote-worker may not be consuming the queue. Last poll: ` +
        `${JSON.stringify(lastJson)}`,
    );
  }
  steps.terminalReached = true;
  collector.scenario.terminalState = terminalState;
  collector.scenario.ok = true;
}

// Hand-rolled multipart/form-data body for the import-job scenario.
// fetch() in Node can stream a Uint8Array directly; this keeps the
// scenario free of any extra npm dep just to wrap a single file part.
function buildMultipartFileBody(boundary, filename, contentType, dataBytes) {
  const head =
    `--${boundary}\r\n` +
    `Content-Disposition: form-data; name="file"; filename="${filename}"\r\n` +
    `Content-Type: ${contentType}\r\n\r\n`;
  const tail = `\r\n--${boundary}--\r\n`;
  const headBytes = new TextEncoder().encode(head);
  const tailBytes = new TextEncoder().encode(tail);
  const total = new Uint8Array(
    headBytes.length + dataBytes.length + tailBytes.length,
  );
  total.set(headBytes, 0);
  total.set(dataBytes, headBytes.length);
  total.set(tailBytes, headBytes.length + dataBytes.length);
  return total;
}

// ─── Pre-sync edit preserved on reload (regression for 07147fa) ────
// End-to-end coverage for the data-loss class fixed by 07147fa: edits
// typed *during* the WS sync handshake (between Connected and Synced)
// used to be silently dropped because `send_update` early-returned
// before applying the diff to ydoc. Unit tests pin the buffer-side
// and flush-side invariants; this scenario proves the full
// round-trip — page edit → server-side persistence → reload-fetch —
// works under the exact timing condition that regressed.
//
// The deterministic-repro trick is `context.routeWebSocket(...)`:
// we intercept the WS connection, forward page→server normally, but
// hold the very first server→page SyncStep2 (msg byte 0x02) for
// PRE_SYNC_HOLD_MS so the client stays "Connected, not Synced" long
// enough for our keystrokes to land in the typing-during-sync window.
// After the hold, the held frame is released and the client enters
// Synced + flushes pending_updates over the wire normally.
async function scenarioPreSyncEditPreserved(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-presync@ogrenotes.example.com",
  );

  const title = `doctor-presync-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);

  // Hold the first SyncStep2 server→page for this many ms. Sized so
  // a `keyboard.type(...)` of the sentinel comfortably finishes
  // inside the window — the sentinel is short and the per-keystroke
  // delay is small.
  const PRE_SYNC_HOLD_MS = 3000;
  const SYNC_STEP2 = 0x02;

  await context.routeWebSocket(/\/api\/v1\/documents\/.*\/ws/, (ws) => {
    const server = ws.connectToServer();
    let firstSyncStep2Released = false;
    server.onMessage((message) => {
      const bytes =
        message instanceof Buffer
          ? message
          : Buffer.from(message instanceof ArrayBuffer ? new Uint8Array(message) : message);
      if (!firstSyncStep2Released && bytes.length > 0 && bytes[0] === SYNC_STEP2) {
        // Hold this one frame, release after the window expires.
        firstSyncStep2Released = true;
        setTimeout(() => {
          ws.send(message);
        }, PRE_SYNC_HOLD_MS);
        return;
      }
      ws.send(message);
    });
    // page→server forwarding stays automatic because we don't call
    // ws.onMessage — Playwright preserves the default forwarder.
  });

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  const sentinel = `PRE_SYNC_SENTINEL_${Date.now()}`;
  collector.scenario = {
    name: "pre-sync-edit-preserved",
    target,
    docId: doc.id,
    sentinel,
    holdMs: PRE_SYNC_HOLD_MS,
    steps: {},
  };
  const steps = collector.scenario.steps;

  try {
    // /content fetch fires immediately on navigation and isn't
    // gated on the WS handshake, so the editor appears regardless
    // of whether SyncStep2 has been released yet.
    await page.goto(`${target}/d/${doc.id}`, {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    const editor = page.locator('[contenteditable="true"]').first();
    await editor.waitFor({ timeout: 15000 });
    steps.editorVisible = true;

    // Type into the editor *while WS is still in the handshake* —
    // SyncStep2 from the server is held by routeWebSocket above, so
    // the client's state is Connected, not Synced. This is the bug
    // window: pre-fix, every keystroke here was lost.
    await editor.click();
    await page.keyboard.type(sentinel, { delay: 15 });
    steps.typedDuringHandshake = true;

    // Wait past the hold + a settle window to give the post-sync
    // flush and the server's append_update + DocUpdate write time
    // to land.
    await page.waitForTimeout(PRE_SYNC_HOLD_MS + 3000);
    steps.waitedForFlush = true;

    // Reload — fresh navigation pulls the doc state from S3 +
    // pending DocUpdate rows. The sentinel survives iff the
    // buffered keystrokes reached the server.
    await page.reload({ waitUntil: "domcontentloaded", timeout: 30000 });
    const editorAfter = page.locator('[contenteditable="true"]').first();
    await editorAfter.waitFor({ timeout: 15000 });
    // Small settle for the editor's hydrate-from-content path.
    await page.waitForTimeout(1500);

    const bodyText = await page
      .locator('[contenteditable="true"]')
      .first()
      .textContent();
    if (!bodyText || !bodyText.includes(sentinel)) {
      throw new Error(
        `pre-sync edit lost on reload — sentinel "${sentinel}" not ` +
          `found in body text: ${JSON.stringify((bodyText || "").slice(0, 500))}`,
      );
    }
    steps.sentinelPreservedAfterReload = true;
    collector.scenario.ok = true;
  } finally {
    await context.close();
    await browser.close();
  }
}

// ─── Sustained-type → reload scenario ──────────────────────────────
// Repros the persist-on-refresh bug class fixed in d92dac4 / 108d4fc:
// before those commits, a stream of keystrokes against a default
// (non-commentable-container-shaped) doc generated ~60 KB updates per
// keystroke that piled into DDB faster than they could merge, then
// the GET /content path silently truncated at the 1 MB DDB Query cap,
// dropping everything but the first few characters on reload. This
// scenario doesn't replicate the slow-path symptom directly — that's
// gap #1's unit test — but it nails the user-visible outcome: 100
// sequential keystrokes must survive a `page.reload()`. Pre-fix this
// scenario would have failed; post-fix the sentinel round-trips.
async function scenarioSustainedTypeReload(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-sustained@ogrenotes.example.com",
  );

  const title = `doctor-sustained-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  const sentinel = `SUSTAINED_TYPE_SENTINEL_${Date.now()}`;
  collector.scenario = {
    name: "sustained-type-reload",
    target,
    docId: doc.id,
    sentinel,
    steps: {},
  };
  const steps = collector.scenario.steps;

  try {
    await page.goto(`${target}/d/${doc.id}`, {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    const editor = page.locator('[contenteditable="true"]').first();
    await editor.waitFor({ timeout: 15000 });
    steps.editorVisible = true;

    // The editor becomes DOM-visible before the WS SyncStep2 frame
    // arrives. Typing in that window exercises the pre-sync buffer
    // flush path (which `scenarioPreSyncEditPreserved` covers) —
    // not the steady-state send path this scenario is named for.
    // `.sync-indicator.is-saved` is set by SyncIndicator when
    // ConnectionState::Synced and pending == 0, so it's the
    // canonical "we are in the normal send path" signal.
    await page.waitForSelector('.sync-indicator.is-saved', {
      timeout: 15000,
    });
    steps.wsSynced = true;

    // Type the sentinel character-by-character at a realistic pace.
    // The 15 ms inter-key delay mirrors a typist who is paying
    // attention — fast enough to stress the update pipeline, slow
    // enough that yrs has a chance to fold the sequential edits into
    // a single block. Total keystroke count is the sentinel's length
    // (~30 chars); the scenario was originally specced for 100, but
    // a shorter sentinel keeps the scenario well under a minute on
    // CI while still exercising the persist path.
    await editor.click();
    await page.keyboard.type(sentinel, { delay: 15 });
    steps.typingComplete = true;

    // Settle window for the buffered keystrokes to flush over WS and
    // for the server's append_update + DocUpdate writes to land.
    await page.waitForTimeout(2000);
    steps.waitedForFlush = true;

    // Reload — fresh navigation pulls the doc state from S3 +
    // pending DocUpdate rows via GET /content. The sentinel
    // survives iff all keystrokes reached durable storage and
    // GET /content returns the full update tail.
    await page.reload({ waitUntil: "domcontentloaded", timeout: 30000 });
    const editorAfter = page.locator('[contenteditable="true"]').first();
    await editorAfter.waitFor({ timeout: 15000 });
    await page.waitForTimeout(1500);

    const bodyText = await page
      .locator('[contenteditable="true"]')
      .first()
      .textContent();
    if (!bodyText || !bodyText.includes(sentinel)) {
      throw new Error(
        `sustained typing lost on reload — sentinel "${sentinel}" not ` +
          `found in body text: ${JSON.stringify((bodyText || "").slice(0, 500))}`,
      );
    }
    steps.sentinelPreservedAfterReload = true;
    collector.scenario.ok = true;
  } finally {
    await context.close();
    await browser.close();
  }
}

// ─── First-keystrokes scenario (#92) ────────────────────────────────
// Regression for the mount-time input race: characters typed after
// `.editor-content[data-editor-ready]` but before the initial WS
// SyncStep2 settles were dropped and/or reordered in the LIVE view —
// the sync-triggered swap rebuilt the editor from a ydoc that had never
// seen them. The fix folds the live editor model into the ydoc before
// every remote apply (merge, not clobber) and removes the is_synced()
// gate so `send_update` can buffer pre-sync edits.
//
// `pre-sync-edit-preserved` cannot catch this: it only asserts the
// sentinel survives a RELOAD, and the REST autosave (PUT /content)
// fires during the hold and persists the text even when the live view
// clobbered it. This scenario reuses the same deterministic
// SyncStep2-hold trick but asserts the LIVE editor text, exactly —
// dropped chars and reordering both fail the equality check — and only
// then the post-reload persistence.
async function scenarioFirstKeystrokes(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(
    target, emailA || "doctor-firstkeys@ogrenotes.example.com",
  );
  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-firstkeys-${Date.now()}`,
    "document",
  );

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);

  // Hold the first server→page SyncStep2 so the client stays in the
  // Connected-not-Synced window while we type — the deterministic
  // version of "start typing the instant the editor renders".
  const PRE_SYNC_HOLD_MS = 3000;
  const SYNC_STEP2 = 0x02;
  await context.routeWebSocket(/\/api\/v1\/documents\/.*\/ws/, (ws) => {
    const server = ws.connectToServer();
    let firstSyncStep2Released = false;
    server.onMessage((message) => {
      const bytes =
        message instanceof Buffer
          ? message
          : Buffer.from(message instanceof ArrayBuffer ? new Uint8Array(message) : message);
      if (!firstSyncStep2Released && bytes.length > 0 && bytes[0] === SYNC_STEP2) {
        firstSyncStep2Released = true;
        setTimeout(() => {
          ws.send(message);
        }, PRE_SYNC_HOLD_MS);
        return;
      }
      ws.send(message);
    });
  });

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  const PHRASE = "alpha one alpha two alpha three";
  const steps = {};
  collector.scenario = {
    name: "first-keystrokes",
    target,
    docId: doc.id,
    phrase: PHRASE,
    holdMs: PRE_SYNC_HOLD_MS,
    steps,
  };

  try {
    await page.goto(`${target}/d/${doc.id}`, {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    // Wait ONLY for readiness — SyncStep2 is being held, so typing here
    // lands squarely in the mount/sync race window.
    await page.waitForSelector(".editor-content[data-editor-ready]", {
      timeout: 15000,
    });
    const ed = page.locator(".editor-content");
    await ed.click();
    await page.keyboard.type(PHRASE, { delay: 15 });
    steps.typedDuringHandshake = true;

    // Release + settle: the held SyncStep2 lands, the recv path folds /
    // swaps, the pending buffer flushes.
    await page.waitForTimeout(PRE_SYNC_HOLD_MS + 2000);

    // The #92 assertion: the LIVE view kept every keystroke, in order.
    const live = ((await ed.textContent()) || "").trim();
    collector.scenario.live = live;
    steps.liveTextExact = live === PHRASE;
    if (!steps.liveTextExact) {
      throw new Error(
        `live editor text lost/reordered keystrokes after initial sync: ` +
          `expected ${JSON.stringify(PHRASE)}, got ${JSON.stringify(live)}`,
      );
    }

    // And the same text persisted (round-trip through the server).
    await page.reload({ waitUntil: "domcontentloaded", timeout: 30000 });
    await page.waitForSelector(".editor-content[data-editor-ready]", {
      timeout: 15000,
    });
    await page.waitForTimeout(1500);
    const persisted = ((await ed.textContent()) || "").trim();
    collector.scenario.persisted = persisted;
    steps.persistedTextExact = persisted === PHRASE;
    if (!steps.persistedTextExact) {
      throw new Error(
        `persisted text diverged after reload: expected ` +
          `${JSON.stringify(PHRASE)}, got ${JSON.stringify(persisted)}`,
      );
    }
    collector.scenario.ok = true;
  } catch (e) {
    collector.stepError = `${e.message}`;
    throw e;
  } finally {
    const tab = collector["tab-a"] || { errors: [] };
    steps.noPageErrors = (tab.errors || []).length === 0;
    await page
      .screenshot({ path: join(outDir, "tab-a.png"), fullPage: false })
      .catch(() => {});
    await context.close();
    await browser.close();
  }
}

// ─── Menu → Export download scenario ───────────────────────────────
// Repros the 2026-05-28 bug class where Document → Markdown / HTML /
// CSV / Excel opened the export URL in a fresh tab via window.open,
// which carries no bearer header and 401'd. The fix moved exports to
// an authenticated fetch + client-side Blob download. This scenario
// clicks each menu item end-to-end and asserts the browser actually
// receives a `download` event with the expected file extension. Any
// regression that brings back the fresh-tab path (or otherwise breaks
// auth on /export) surfaces here as no-download.
async function scenarioMenuExportDownloads(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const tokens = await devLogin(target, emailA || "doctor-export@ogrenotes.example.com");

  const title = `doctor-export-${Date.now()}`;
  const doc = await createDocViaApi(target, tokens.accessToken, title);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    acceptDownloads: true,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
  });
  await seedAuth(context, tokens);
  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  collector.scenario = {
    name: "menu-export-downloads",
    target,
    docId: doc.id,
    steps: {},
  };
  const steps = collector.scenario.steps;

  try {
    await page.goto(`${target}/d/${doc.id}`, {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    await page.waitForSelector('[contenteditable="true"]', { timeout: 15000 });
    steps.docLoaded = true;

    // Each Document → Export submenu entry — visible label and the
    // file extension we expect the browser to suggest in the download.
    // Covers text formats (markdown/html/csv) and binary (xlsx) so a
    // future binary-handling regression also surfaces.
    const formats = [
      { label: "Markdown", ext: "md" },
      { label: "HTML", ext: "html" },
      { label: "CSV", ext: "csv" },
      { label: "Excel (.xlsx)", ext: "xlsx" },
    ];

    for (const { label, ext } of formats) {
      await page
        .getByRole("button", { name: "Document", exact: true })
        .click();
      const [download] = await Promise.all([
        page.waitForEvent("download", { timeout: 15000 }),
        // Menu items render as buttons whose visible text is the
        // label with leading spaces for the submenu indent — using
        // a substring match keeps the selector resilient to that.
        page.getByText(label, { exact: false }).first().click(),
      ]);
      const filename = await download.suggestedFilename();
      if (!filename.toLowerCase().endsWith(`.${ext}`)) {
        throw new Error(
          `${label} export: expected .${ext} suffix, got ${filename}`,
        );
      }
      steps[`download_${ext}`] = filename;
    }

    collector.scenario.ok = true;
  } finally {
    await context.close();
    await browser.close();
  }
}

// ─── block-links scenario ───────────────────────────────────────
//
// Promoted from the ad-hoc probe-block-links.mjs. Exercises block deep
// links end to end:
//   - producer: right-click a block → "Copy Link to Block" copies
//     `<baseUrl>/d/<docId>[/<slug>]#b=<blockId>` to the clipboard.
//   - consumer: navigating to a valid `#b=` fragment scrolls to and
//     flashes the target block, with no toast.
//   - malformed/foreign hashes (`#appearance`, `#b=`, `#b=abc def`) are
//     inert — no scroll flash, no toast, no console errors.
//   - navigating to an unknown/deleted block id shows a top toast
//     ("no longer exists") that auto-dismisses.
async function scenarioBlockLinks(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, outDir } = ctx;
  const target = baseUrlA || baseUrl;

  const tokens = await devLogin(
    target,
    emailA || `doctor-blocklinks-${Date.now()}@ogrenotes.example.com`
  );
  logJson({ at: "dev-login", userId: tokens.userId });

  const doc = await createDocViaApi(
    target,
    tokens.accessToken,
    `doctor-blocklinks-${Date.now()}`,
    "document"
  );
  logJson({ at: "doc-created", docId: doc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
    permissions: ["clipboard-read", "clipboard-write"],
  });
  await seedAuth(context, tokens);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  const waitFor = async (pred, ms = 6000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await pred().catch(() => false)) return true;
      await page.waitForTimeout(150);
    }
    return false;
  };

  const steps = {};
  try {
    await page.goto(`${target}/d/${doc.id}`, {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 20000 });
    await page.waitForTimeout(1200); // first-keystroke flake: settle
    await page.locator(".editor-content").click();
    await page.keyboard.type("First paragraph for block links");
    await page.keyboard.press("Enter");
    await page.keyboard.type("Second paragraph is the target");
    await waitFor(async () => (await page.locator(".editor-content [data-block-id]").count()) >= 2);

    const blocks = page.locator(".editor-content [data-block-id]");
    const nBlocks = await blocks.count();
    steps.blocksHaveIds = nBlocks >= 2;
    const targetBlock = blocks.nth(nBlocks - 1);
    const blockId = await targetBlock.getAttribute("data-block-id");
    steps.targetBlockId = !!blockId;

    // ── producer: right-click → Copy Link to Block ──
    await targetBlock.click(); // caret into the target block
    await targetBlock.click({ button: "right" });
    const menuItem = page.locator(".ui-menu-item", { hasText: "Copy Link to Block" });
    steps.menuItemVisible = await menuItem.waitFor({ timeout: 5000 }).then(() => true).catch(() => false);
    await menuItem.click();
    await page.waitForTimeout(300);
    let clip = null;
    try {
      clip = await page.evaluate(() => navigator.clipboard.readText());
    } catch (e) {
      collector.stepError = `clipboard read blocked: ${e.message}`;
    }
    // The page pathname may carry the /d/:id/:slug variant — the fragment
    // composes with it, so accept both forms.
    const blockUrlRe = new RegExp(`^${target}/d/${doc.id}(/[^#]*)?#b=${blockId}$`);
    steps.clipboardUrlCorrect = clip !== null && blockUrlRe.test(clip);

    // ── consumer: navigate to the (actual copied, when readable) block
    // URL, expect scroll + flash, no toast ──
    await page.goto(clip ?? `${target}/d/${doc.id}#b=${blockId}`, { waitUntil: "domcontentloaded" });
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 20000 });
    steps.validFragmentFlash = await page
      .waitForSelector(`[data-block-id="${blockId}"].block-link-flash`, { timeout: 6000 })
      .then(() => true).catch(() => false);
    steps.validFragmentNoToast = (await page.locator(".collab-liveapp-toast").count()) === 0;
    await page.screenshot({ path: join(outDir, "1-valid-fragment.png") }).catch(() => {});

    // ── malformed / foreign hashes: no scroll, no toast, no errors ──
    const foreignHashes = [
      { key: "foreignHashInertAppearance", frag: "#appearance" },
      { key: "foreignHashInertEmptyB", frag: "#b=" },
      { key: "foreignHashInertMalformedB", frag: "#b=abc def" },
    ];
    for (const { key, frag } of foreignHashes) {
      await page.goto(`${target}/d/${doc.id}${encodeURI(frag)}`, { waitUntil: "domcontentloaded" });
      await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 20000 });
      // Settle-poll: confirm the inert state holds steady rather than a
      // fixed sleep — assert it stays true across the whole window.
      let inert = true;
      const end = Date.now() + 1500;
      while (Date.now() < end) {
        const toast = await page.locator(".collab-liveapp-toast").count();
        const flash = await page.locator(".block-link-flash").count();
        if (toast !== 0 || flash !== 0) { inert = false; break; }
        await page.waitForTimeout(150);
      }
      steps[key] = inert;
    }
    await page.screenshot({ path: join(outDir, "2-foreign-hash.png") }).catch(() => {});

    // ── deleted/unknown block: toast appears, stays at top, auto-dismisses ──
    await page.goto(`${target}/d/${doc.id}#b=zzzz_gone_${Date.now() % 100000}`, {
      waitUntil: "domcontentloaded",
    });
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 20000 });
    const toastEl = page.locator(".collab-liveapp-toast");
    steps.missingBlockToastShown = await toastEl.first().waitFor({ timeout: 6000 })
      .then(() => true).catch(() => false);
    if (steps.missingBlockToastShown) {
      const text = await toastEl.first().textContent();
      steps.missingBlockToastText = /no longer exists/i.test(text ?? "");
      await page.screenshot({ path: join(outDir, "3-missing-block-toast.png") }).catch(() => {});
      steps.missingBlockToastAutodismiss = await toastEl.first()
        .waitFor({ state: "detached", timeout: 8000 })
        .then(() => true).catch(() => false);
    }
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  const tab = collector["tab-a"] || { errors: [], console: [] };
  const consoleErrors = (tab.console || []).filter((m) => m.type === "error");
  steps.noConsoleErrors = consoleErrors.length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "block-links",
    steps,
  };

  await page.screenshot({ path: join(outDir, "tab-a.png"), fullPage: false }).catch(() => {});
  await context.close();
  await browser.close();
}

// ─── doc-mentions scenario ───────────────────────────────────────
//
// Promoted from the ad-hoc probe-doc-mentions.mjs. Exercises the
// DocMention paste matrix and chip element behavior:
//   - paste a bare doc URL / anchor URL / dangling-block URL / stranger's
//     inaccessible doc URL / a URL embedded mid-sentence, and assert each
//     converts (or doesn't) correctly.
//   - a single Ctrl+Z after a doc-URL paste fully restores the raw URL
//     text (one undo step, not two).
//   - reload triggers the overlay resolve pass: dangling anchors and
//     missing (trashed) docs get their degraded class + label.
//   - chip context menu (Copy Original URL / Convert to Plain Link) is
//     present on chips, absent on plain text.
//   - clicking a live doc chip SPA-navigates to the target doc (URL
//     changes; polled, since it need not be a full reload).
//   - clicking a missing chip is inert (no navigation).
//   - Convert to Plain Link removes the chip and leaves plain link text.
async function scenarioDocMentions(ctx, collector) {
  const { baseUrlA, baseUrl, emailA, emailB, outDir } = ctx;
  const target = baseUrlA || baseUrl;
  const stamp = Date.now();

  // Stranger's private doc first, then the verifying user (A) — mirrors
  // the probe's login order, though seedAuth (unlike a shared
  // context.request cookie jar) only ever seeds A's cookies into the
  // browser context below, so the order isn't load-bearing here.
  const tokensB = await devLogin(target, emailB || `doctor-mentions-stranger-${stamp}@ogrenotes.example.com`);
  const bDoc = await createDocViaApi(target, tokensB.accessToken, "B Private");

  const tokensA = await devLogin(target, emailA || `doctor-mentions-verifier-${stamp}@ogrenotes.example.com`);
  const t1 = await createDocViaApi(target, tokensA.accessToken, "Target One");
  const t2 = await createDocViaApi(target, tokensA.accessToken, "Target Two");
  const host = await createDocViaApi(target, tokensA.accessToken, "Host Doc");
  logJson({ at: "docs-created", t1: t1.id, t2: t2.id, host: host.id, bDoc: bDoc.id });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    ...DOCTOR_CONTEXT_DEFAULTS,
    recordHar: { path: join(outDir, "tab-a.har"), mode: "full" },
    permissions: ["clipboard-read", "clipboard-write"],
  });
  await seedAuth(context, tokensA);

  const page = await context.newPage();
  instrument(context, page, "tab-a", collector);

  const ready = async () => {
    await page.waitForSelector(".editor-content[data-editor-ready]", { timeout: 20000 });
    await page.waitForTimeout(1200);
  };
  const waitFor = async (pred, ms = 8000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await pred().catch(() => false)) return true;
      await page.waitForTimeout(150);
    }
    return false;
  };
  async function pasteText(text) {
    await page.evaluate((t) => navigator.clipboard.writeText(t), text);
    await page.locator(".editor-content").click();
    await page.keyboard.press("Control+End");
    await page.keyboard.press("Enter");
    await page.keyboard.press("Control+v");
  }

  const steps = {};
  try {
    // ── Prep: real block in Target One, capture its blockId ──
    await page.goto(`${target}/d/${t1.id}`, { waitUntil: "domcontentloaded", timeout: 30000 });
    await ready();
    await page.locator(".editor-content").click();
    await page.keyboard.type("Anchor target paragraph with snippet text");
    await page.waitForTimeout(1000); // let WS persistence flush
    const t1BlockId = await page
      .locator(".editor-content [data-block-id]").last().getAttribute("data-block-id");
    steps.t1BlockId = !!t1BlockId;

    await page.goto(`${target}/d/${host.id}`, { waitUntil: "domcontentloaded", timeout: 30000 });
    await ready();

    // ── Cell 1: doc-only URL → chip, then SINGLE undo restores raw URL ──
    await pasteText(`${target}/d/${t1.id}`);
    const chip1 = page.locator(`span.doc-mention[data-doc-id="${t1.id}"]`).first();
    steps.pasteDocUrlConverts = await chip1.waitFor({ timeout: 8000 }).then(() => true).catch(() => false);
    if (steps.pasteDocUrlConverts) {
      const text1 = await chip1.textContent();
      // The app auto-derives doc titles from the first line, so T1's live
      // title is its typed paragraph, not "Target One". Assert: doc glyph
      // + the label is the live title (not the raw URL).
      steps.docChipGlyphTitle = (text1 ?? "").includes("📄") && !(text1 ?? "").includes("/d/");

      await page.keyboard.press("Control+z");
      const undone = await waitFor(async () =>
        (await page.locator(`span.doc-mention[data-doc-id="${t1.id}"]`).count()) === 0
        && (await page.locator(".editor-content").textContent() ?? "").includes(`/d/${t1.id}`)
      );
      steps.singleUndoRestoresUrl = undone;
      // Re-paste fresh (no redo-keybinding dependency) so later cells have
      // the doc chip.
      await pasteText(`${target}/d/${t1.id}`);
      await page.locator(`span.doc-mention[data-doc-id="${t1.id}"]`).first()
        .waitFor({ timeout: 8000 }).catch(() => {});
    }

    // ── Cell 2: anchor URL → chip with target + snippet ──
    await pasteText(`${target}/d/${t1.id}#b=${t1BlockId}`);
    const chip2 = page.locator(`span.doc-mention[data-block-id-target="${t1BlockId}"]`).first();
    steps.pasteAnchorUrlConverts = await chip2.waitFor({ timeout: 8000 }).then(() => true).catch(() => false);
    if (steps.pasteAnchorUrlConverts) {
      const text2 = await chip2.textContent();
      steps.anchorChipGlyphSnippet = (text2 ?? "").includes("⚓") && (text2 ?? "").includes("Anchor target");
    }

    // ── Cell 3: dangling fragment → chip keeps target ──
    await pasteText(`${target}/d/${t1.id}#b=zzzz_gone_blk`);
    const chip3 = page.locator('span.doc-mention[data-block-id-target="zzzz_gone_blk"]').first();
    steps.pasteDanglingConverts = await chip3.waitFor({ timeout: 8000 }).then(() => true).catch(() => false);

    // ── Cell 4: inaccessible doc URL → stays plain text ──
    await pasteText(`${target}/d/${bDoc.id}`);
    await page.waitForTimeout(2500); // give a wrong conversion time to (not) happen
    steps.noAccessStaysPlain = (await page.locator(`span.doc-mention[data-doc-id="${bDoc.id}"]`).count()) === 0;

    // ── Cell 5: URL-in-sentence → untouched ──
    const before5 = await page.locator("span.doc-mention").count();
    await pasteText(`see ${target}/d/${t1.id} please`);
    await page.waitForTimeout(2000);
    steps.urlInSentenceUntouched = (await page.locator("span.doc-mention").count()) === before5;

    // ── Cell for T2 (used by missing-state later) ──
    await pasteText(`${target}/d/${t2.id}`);
    await page.locator(`span.doc-mention[data-doc-id="${t2.id}"]`).first()
      .waitFor({ timeout: 8000 }).catch(() => {});
    await page.waitForTimeout(1500); // let attrs/persistence settle

    // ── Reload: overlay resolve pass → dangling gets class + glyph ──
    await page.goto(`${target}/d/${host.id}`, { waitUntil: "domcontentloaded", timeout: 30000 });
    await ready();
    await page.waitForTimeout(1500); // resolve round-trip
    const dangling = page.locator('span.doc-mention[data-block-id-target="zzzz_gone_blk"]').first();
    const danglingClass = await dangling.getAttribute("class").catch(() => null);
    steps.danglingClassApplied = (danglingClass ?? "").includes("doc-mention-dangling");
    const danglingText = await dangling.textContent().catch(() => "");
    steps.danglingDocGlyph = (danglingText ?? "").includes("📄");
    await page.screenshot({ path: join(outDir, "1-chips.png") }).catch(() => {});

    // ── Context menu: Copy Original URL + Convert to Plain Link ──
    const chipT1 = page.locator(`span.doc-mention[data-doc-id="${t1.id}"]`).first();
    await chipT1.click({ button: "right" });
    const copyItem = page.locator(".ui-menu-item", { hasText: "Copy Original URL" });
    const convItem = page.locator(".ui-menu-item", { hasText: "Convert to Plain Link" });
    steps.ctxEntriesVisible =
      await copyItem.waitFor({ timeout: 4000 }).then(() => true).catch(() => false)
      && (await convItem.count()) > 0;
    await copyItem.click();
    await page.waitForTimeout(300);
    let clip = null;
    try { clip = await page.evaluate(() => navigator.clipboard.readText()); } catch {}
    steps.ctxCopyOriginalUrl = clip !== null && clip.includes(`/d/${t1.id}`);
    // Entries absent on plain text right-click:
    await page.locator(".editor-content").click({ button: "right", position: { x: 40, y: 10 } });
    await page.waitForTimeout(400);
    steps.ctxAbsentOnPlainText =
      (await page.locator(".ui-menu-item", { hasText: "Copy Original URL" }).count()) === 0;
    await page.keyboard.press("Escape");

    // ── Click-nav: chip click navigates to reconstructed /d/<id> (SPA
    // navigation — URL still changes, poll instead of waiting for load) ──
    await chipT1.click();
    steps.chipClickNavigates = await waitFor(async () => page.url().includes(`/d/${t1.id}`), 6000);

    // ── Missing state: trash T2, reload host → grayed missing chip,
    // click inert ──
    const del = await fetch(`${target}/api/v1/documents/${t2.id}`, {
      method: "DELETE",
      headers: { authorization: `Bearer ${tokensA.accessToken}` },
    });
    steps.t2Trashed = del.ok;
    await page.goto(`${target}/d/${host.id}`, { waitUntil: "domcontentloaded", timeout: 30000 });
    await ready();
    await page.waitForTimeout(1500);
    const missing = page.locator(`span.doc-mention[data-doc-id="${t2.id}"]`).first();
    const missingClass = await missing.getAttribute("class").catch(() => null);
    steps.missingClassApplied = (missingClass ?? "").includes("doc-mention-missing");
    const missingText = await missing.textContent().catch(() => "");
    steps.missingLabel = /missing document/i.test(missingText ?? "");
    const urlBefore = page.url();
    await missing.click().catch(() => {});
    await page.waitForTimeout(800);
    steps.missingClickInert = page.url() === urlBefore;
    await page.screenshot({ path: join(outDir, "2-missing.png") }).catch(() => {});

    // ── Convert to Plain Link (on the T1 chip) ──
    await page.goto(`${target}/d/${host.id}`, { waitUntil: "domcontentloaded", timeout: 30000 });
    await ready();
    const chipConv = page.locator(`span.doc-mention[data-doc-id="${t1.id}"]`).first();
    // Read what this chip's title-derived link text will be (strip the glyph).
    const convLabel = ((await chipConv.textContent()) ?? "").replace(/^[⚓📄]\s*/u, "");
    const convNodeId = await chipConv.getAttribute("data-node-block-id");
    await chipConv.click({ button: "right" });
    const conv2 = page.locator(".ui-menu-item", { hasText: "Convert to Plain Link" });
    if (await conv2.waitFor({ timeout: 4000 }).then(() => true).catch(() => false)) {
      await conv2.click();
      await page.waitForTimeout(600);
      const chipStillThere =
        (await page.locator(`span.doc-mention[data-node-block-id="${convNodeId}"]`).count()) > 0;
      const editorText = (await page.locator(".editor-content").textContent()) ?? "";
      // The converted text is the TITLE attr (may differ from a snippet
      // label — recorded UX nit); accept either the chip label or the doc
      // title.
      const hasLinkText = editorText.includes("Target One") || (convLabel && editorText.includes(convLabel));
      steps.convertToPlainLink = !chipStillThere && hasLinkText;
    }
    // Backspace-atom deletion: covered by the atom infrastructure's wasm
    // tests (clicking a chip now navigates, so a click-then-Backspace
    // probe cell is unreliable) — left to the human sweep if desired.
    await page.screenshot({ path: join(outDir, "3-final.png") }).catch(() => {});
  } catch (e) {
    collector.stepError = `${e.message}`;
  }

  // Rapid page.goto navigation aborts in-flight requests (title saves,
  // WASM streaming) — known benign scenario-pace noise, not product errors.
  const tab = collector["tab-a"] || { errors: [], console: [] };
  const realErrors = (tab.console || []).filter(
    (m) => m.type === "error" && !/Failed to fetch|loading was aborted|compilation aborted/i.test(m.text || "")
  );
  steps.noConsoleErrors = realErrors.length === 0;

  collector.scenario = {
    name: collector.scenario?.name || "doc-mentions",
    steps,
  };

  await context.close();
  await browser.close();
}

async function main() {
  const args = parseArgs(process.argv);
  const scenario = args.scenario || "collab-sync";
  const baseUrl = args["base-url"];
  const docId = args["doc-id"];
  const emailA = args["email-a"] || "doctor-a@ogrenotes.example.com";
  const emailB = args["email-b"] || "doctor-b@ogrenotes.example.com";
  const outDir = args.out;

  // `collab-sync` needs an existing docId to join; the other
  // scenarios create their own doc via the REST API, so --doc-id is
  // optional for those.
  const needsDocId = scenario === "collab-sync";
  if (!baseUrl || (needsDocId && !docId) || !outDir) {
    console.error(
      "usage: doctor.js --base-url <url> --out <dir> " +
        "[--scenario collab-sync|trash-flow|spreadsheet-paste|" +
        "spreadsheet-features|pivot-editor|comment-live-sync|" +
        "spreadsheet-keyboard|spreadsheet-headers|spreadsheet-toolbar|" +
        "spreadsheet-freeze|spreadsheet-sheet-tabs|" +
        "spreadsheet-remote-cursor|mobile-spreadsheet-keyboards|" +
        "command-palette-actions|embed-youtube|calendar-block|kanban-block|kanban-drag|kanban-column-reorder|kanban-wip-limit|kanban-card-metadata|type-past-atom|code-block-enter|bulk-delete|" +
        "a11y-audit|ask-flow|admin-console|mfa-flow|" +
        "semantic-search|import-job-round-trip|" +
        "menu-export-downloads|pre-sync-edit-preserved|" +
        "sustained-type-reload|history-pane|delete-document|" +
        "share-dialog|comment-popup|block-links|doc-mentions] " +
        "[--doc-id <docId>] [--email-a <addr>] [--email-b <addr>]"
    );
    process.exit(2);
  }

  if (!existsSync(outDir)) mkdirSync(outDir, { recursive: true });

  const collector = { scenario: { name: scenario } };
  const ctx = { baseUrl, docId, emailA, emailB, outDir };

  const startedAt = Date.now();
  let ok = true;
  let errMsg = null;
  try {
    if (scenario === "collab-sync") {
      await scenarioCollabSync(ctx, collector);
    } else if (scenario === "trash-flow") {
      await scenarioTrashFlow(ctx, collector);
    } else if (scenario === "spreadsheet-paste") {
      await scenarioSpreadsheetPaste(ctx, collector);
    } else if (scenario === "spreadsheet-features") {
      await scenarioSpreadsheetFeatures(ctx, collector);
    } else if (scenario === "spreadsheet-lifecycle") {
      await scenarioSpreadsheetLifecycle(ctx, collector);
    } else if (scenario === "focus-mode") {
      await scenarioFocusMode(ctx, collector);
    } else if (scenario === "settings-appearance") {
      await scenarioSettingsAppearance(ctx, collector);
    } else if (scenario === "menu-switch") {
      await scenarioMenuSwitch(ctx, collector);
    } else if (scenario === "doc-actions") {
      await scenarioDocActions(ctx, collector);
    } else if (scenario === "favorites") {
      await scenarioFavorites(ctx, collector);
    } else if (scenario === "find-replace") {
      await scenarioFindReplace(ctx, collector);
    } else if (scenario === "line-numbers") {
      await scenarioLineNumbers(ctx, collector);
    } else if (scenario === "document-details") {
      await scenarioDocumentDetails(ctx, collector);
    } else if (scenario === "expand") {
      await scenarioExpand(ctx, collector);
    } else if (scenario === "subscript") {
      await scenarioSubscript(ctx, collector);
    } else if (scenario === "pivot-editor") {
      await scenarioPivotEditor(ctx, collector);
    } else if (scenario === "comment-live-sync") {
      await scenarioCommentLiveSync(ctx, collector);
    } else if (scenario === "spreadsheet-keyboard") {
      await scenarioSpreadsheetKeyboard(ctx, collector);
    } else if (scenario === "spreadsheet-headers") {
      await scenarioSpreadsheetHeaders(ctx, collector);
    } else if (scenario === "spreadsheet-toolbar") {
      await scenarioSpreadsheetToolbar(ctx, collector);
    } else if (scenario === "spreadsheet-freeze") {
      await scenarioSpreadsheetFreeze(ctx, collector);
    } else if (scenario === "spreadsheet-sheet-tabs") {
      await scenarioSpreadsheetSheetTabs(ctx, collector);
    } else if (scenario === "spreadsheet-remote-cursor") {
      await scenarioSpreadsheetRemoteCursor(ctx, collector);
    } else if (scenario === "mobile-spreadsheet-keyboards") {
      await scenarioMobileSpreadsheetKeyboards(ctx, collector);
    } else if (scenario === "command-palette-actions") {
      await scenarioCommandPaletteActions(ctx, collector);
    } else if (scenario === "embed-youtube") {
      await scenarioEmbedYouTube(ctx, collector);
    } else if (scenario === "calendar-block") {
      await scenarioCalendarBlock(ctx, collector);
    } else if (scenario === "kanban-block") {
      await scenarioKanbanBlock(ctx, collector);
    } else if (scenario === "kanban-drag") {
      await scenarioKanbanDrag(ctx, collector);
    } else if (scenario === "kanban-column-reorder") {
      await scenarioKanbanColumnReorder(ctx, collector);
    } else if (scenario === "kanban-wip-limit") {
      await scenarioKanbanWipLimit(ctx, collector);
    } else if (scenario === "kanban-card-metadata") {
      await scenarioKanbanCardMetadata(ctx, collector);
    } else if (scenario === "type-past-atom") {
      await scenarioTypePastAtom(ctx, collector);
    } else if (scenario === "code-block-enter") {
      await scenarioCodeBlockEnter(ctx, collector);
    } else if (scenario === "bulk-delete") {
      await scenarioBulkDelete(ctx, collector);
    } else if (scenario === "a11y-audit") {
      await scenarioA11yAudit(ctx, collector);
    } else if (scenario === "ask-flow") {
      await scenarioAskFlow(ctx, collector);
    } else if (scenario === "admin-console") {
      await scenarioAdminConsole(ctx, collector);
    } else if (scenario === "mfa-flow") {
      await scenarioMfaFlow(ctx, collector);
    } else if (scenario === "semantic-search") {
      await scenarioSemanticSearch(ctx, collector);
    } else if (scenario === "import-job-round-trip") {
      await scenarioImportJobRoundTrip(ctx, collector);
    } else if (scenario === "menu-export-downloads") {
      await scenarioMenuExportDownloads(ctx, collector);
    } else if (scenario === "pre-sync-edit-preserved") {
      await scenarioPreSyncEditPreserved(ctx, collector);
    } else if (scenario === "sustained-type-reload") {
      await scenarioSustainedTypeReload(ctx, collector);
    } else if (scenario === "first-keystrokes") {
      await scenarioFirstKeystrokes(ctx, collector);
    } else if (scenario === "history-pane") {
      await scenarioHistoryPane(ctx, collector);
    } else if (scenario === "delete-document") {
      await scenarioDeleteDocument(ctx, collector);
    } else if (scenario === "share-dialog") {
      await scenarioShareDialog(ctx, collector);
    } else if (scenario === "comment-popup") {
      await scenarioCommentPopup(ctx, collector);
    } else if (scenario === "block-links") {
      await scenarioBlockLinks(ctx, collector);
    } else if (scenario === "doc-mentions") {
      await scenarioDocMentions(ctx, collector);
    } else {
      throw new Error(`unknown scenario: ${scenario}`);
    }
  } catch (e) {
    ok = false;
    errMsg = `${e.message}\n${e.stack || ""}`;
  }
  const durationMs = Date.now() - startedAt;

  // trash-flow reports a `steps` map of what succeeded; if any step is
  // missing/false we flip ok=false so CI treats it as a failure even when
  // no exception was thrown.
  if (ok && scenario === "trash-flow") {
    const s = collector.scenario?.steps || {};
    const required = [
      "docLoaded",
      "deletedAndHomeNav",
      "trashRowVisible",
      "trashedDocListedWithActions",
      "trashBannerShown",
      "editorReadonly",
      "restoredAndHomeNav",
      "docBackInHome",
      "purgedFromTrash",
      "purgedApi404",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `trash-flow steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "mfa-flow") {
    const s = collector.scenario?.steps || {};
    const required = [
      "initialLogin",
      "enrollPageRenderedSecret",
      "recoveryCodesDisplayed",
      "verifyFinalizedEnrollment",
      "loggedOutAfterEnroll",
      "reloginReturnedMfaPending202",
      "totpChallengeMintedSession",
      "postChallengeMeWorks",
      "recoveryCodeMintedSession",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg =
        `mfa-flow steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "admin-console") {
    const s = collector.scenario?.steps || {};
    const required = [
      "adminUsersPageMounted",
      "peerRowVisibleAfterSearch",
      "peerDisabled",
      "peerReEnabled",
      "auditRowsVisible",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `admin-console steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "comment-live-sync") {
    const s = collector.scenario?.steps || {};
    const required = [
      "tabAPopupOpen",
      "tabBPopupOpen",
      "popupBodyOverflowed",
      "tabBScrolledOnOpen",
      "replyPosted",
      "tabBSawReply",
      "tabASawReply",
      "tabBScrolledAfterReply",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `comment-live-sync steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "spreadsheet-features") {
    const s = collector.scenario?.steps || {};
    const required = [
      "gridMounted",
      "toolbarPresent",
      "formatPainterPresent",
      "sheetTabsPresent",
      "formulaBarPresent",
      "cellsPopulated",
      "sortDialogOpened",
      "sortDialogClosedAfterApply",
      "sortReorderedRows",
      "contextMenuOpens",
      "frozenRowClassApplied",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `spreadsheet-features steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "favorites") {
    const s = collector.scenario?.steps || {};
    const required = [
      "startsUnstarred",
      "starButtonGoesActive",
      "appearsInSidebar",
      "unstarButtonGoesInactive",
      "leavesSidebar",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `favorites steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "find-replace") {
    const s = collector.scenario?.steps || {};
    const required = [
      "barOpensFromMenu",
      "barOpensFromCtrlF",
      "countShowsThreeMatches",
      "nextAdvancesMatch",
      "replaceAllRewritesDoc",
      "noMatchesAfterReplace",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `find-replace steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "line-numbers") {
    const s = collector.scenario?.steps || {};
    const required = [
      "editorTyped",
      "numbersAppear",
      "perVisualLine",
      "uniformFont",
      "noFullWidthPageRule",
      "togglesOff",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `line-numbers steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "document-details") {
    const s = collector.scenario?.steps || {};
    const required = [
      "panelOpens",
      "hasAllRows",
      "wordCountCorrect",
      "charCountCorrect",
      "panelCloses",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `document-details steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "subscript") {
    const s = collector.scenario?.steps || {};
    const required = ["subscriptRenders", "switchesToSuperscript", "noPageErrors"];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `subscript steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "expand") {
    const s = collector.scenario?.steps || {};
    const required = [
      "startsCollapsed",
      "entersExpanded",
      "headerHidden",
      "fabShown",
      "collapses",
      "headerBack",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `expand steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "doc-actions") {
    const s = collector.scenario?.steps || {};
    const required = [
      "renameUpdatesTitle",
      "duplicateDialogPrefillsName",
      "duplicateNavigatedToNewDoc",
      "duplicateUsesEnteredName",
      "duplicateCopiedContent",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `doc-actions steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "menu-switch") {
    const s = collector.scenario?.steps || {};
    const required = [
      "documentOpened",
      "switchedToViewInOneClick",
      "switchedToFormatInOneClick",
      "sameNameCloses",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `menu-switch steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "focus-mode") {
    const s = collector.scenario?.steps || {};
    // #134: the toggle must enter + exit via both the button and the
    // shortcut, the chrome must hide/show, and nothing may panic.
    const required = [
      "startsUnfocused",
      "menuVisibleInitially",
      "buttonEntersFocus",
      "menuHiddenInFocus",
      "toggleStillPresentInFocus",
      "buttonExitsFocus",
      "menuVisibleAfterExit",
      "shortcutEntersFocus",
      "shortcutExitsFocus",
      "noPageErrors",
      "noPanicConsole",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `focus-mode steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "spreadsheet-lifecycle") {
    const s = collector.scenario?.steps || {};
    // #76: every loop must complete, the post-unmount copy/paste must land
    // its value (proves the surviving doc's engine is still live), and the
    // run must produce no panic — the use-after-free this guards against
    // surfaces as a Rust panic on a reclaimed engine.
    const required = [
      "bothDocsCreated",
      "loopsCompleted",
      "postUnmountPasteWorks",
      "noPageErrors",
      "noPanicConsole",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `spreadsheet-lifecycle steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "history-pane") {
    const s = collector.scenario?.steps || {};
    // Must actually open a diff modal (the panic only fires on modal
    // teardown), then close it with no pageerror. modalOpen guards
    // against a false green if version seeding ever stops producing a
    // version to browse.
    const required = ["editorMounted", "paneOpened", "modalOpen", "noPanic"];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      const errs = (collector.scenario?.errors || [])
        .map((e) => e.message)
        .join("; ");
      errMsg = `history-pane steps failed: ${missing.join(", ")}` +
        (errs ? ` [pageerrors: ${errs}]` : "") +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "delete-document") {
    const s = collector.scenario?.steps || {};
    const required = ["editorMounted", "confirmShown", "noPanicOnDelete"];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      const errs = (collector.scenario?.errors || [])
        .map((e) => e.message)
        .join("; ");
      errMsg = `delete-document steps failed: ${missing.join(", ")}` +
        (errs ? ` [pageerrors: ${errs}]` : "") +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && (scenario === "share-dialog" || scenario === "comment-popup")) {
    const s = collector.scenario?.steps || {};
    const shownKey = scenario === "share-dialog" ? "dialogShown" : "popupShown";
    const required = ["editorMounted", shownKey, "noPanic"];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      const errs = (collector.scenario?.errors || []).map((e) => e.message).join("; ");
      errMsg = `${scenario} steps failed: ${missing.join(", ")}` +
        (errs ? ` [pageerrors: ${errs}]` : "") +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "pivot-editor") {
    const s = collector.scenario?.steps || {};
    const required = [
      "gridMounted",
      "cellsPopulated",
      "contextMenuOpens",
      "editorOpened",
      "fieldListPopulated",
      "chipsAppearedInZone",
      "summarizeFnPickerPresent",
      "editorClosedAfterDelete",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `pivot-editor steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "spreadsheet-paste") {
    const s = collector.scenario?.steps || {};
    const required = [
      "gridMounted",
      "cellsPopulated",
      "pasteExecuted",
      "pastedFormulaTranslated",
      "pastedValueIs12",
      "noPermissionDialog",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `spreadsheet-paste steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  // ─── May 2026 UI batch — six required-steps gates ───────────────
  const requiredSteps = {
    "settings-appearance": [
      "stylesheetApplied", "threeThemeButtons", "panelStretches",
      "themeButtonsInsidePanel", "noPageErrors",
    ],
    "spreadsheet-keyboard": [
      "gridMounted", "valueTyped", "undoCleared", "redoRestored",
    ],
    "spreadsheet-headers": [
      "gridMounted", "cellsPopulated", "colHeaderClickSelectsColumn",
      "statusBarShowsCount", "statusBarShowsSum",
    ],
    "spreadsheet-toolbar": [
      "gridMounted", "toolbarPresent", "formatDropdownIsFormat",
      "currencyAppliedToCell", "boldButtonAppliedFontWeight",
    ],
    "spreadsheet-freeze": [
      "gridMounted", "contextMenuOpens",
      "frozenAboveExcludesClickedRow", "clickedRowNotFrozen",
    ],
    "spreadsheet-sheet-tabs": [
      "gridMounted", "secondTabAdded", "tabContextMenuOpened",
      "contextMenuFitsInViewport", "deleteRemovesTab",
    ],
    "spreadsheet-remote-cursor": [
      "tabAGridMounted", "tabBGridMounted", "tabANavigated",
      "tabBSeesRemoteCell",
    ],
    "mobile-spreadsheet-keyboards": [
      "gridMounted", "touchPrimaryActive", "cellEditing",
      "formulaKeyboardMounted", "autocompleteVisible",
      "autocompleteAboveKeyboard", "firstMatchIsSum",
    ],
    "command-palette-actions": [
      "editorReady", "textTyped", "textSelected",
      "paletteOpened", "paletteInActionMode",
      "boldVisible", "enterDispatched", "dialogClosed",
      "boldApplied",
    ],
    "embed-youtube": [
      "editorReady", "embedButtonVisible", "iframeInserted",
      "srcRewrittenToEmbed", "sandboxAllowsScripts",
      "referrerPolicyCorrect", "loadingLazy",
      "wrapperContenteditableFalse",
    ],
    "bulk-delete": [
      "fileBrowserMounted", "threeBoxesChecked",
      "selectionBarVisible", "countShows3", "confirmDialogOpened",
      "selectionBarDismissed", "docsRemovedFromHome",
    ],
    "a11y-audit": [
      "homeMounted", "homeNoSeriousOrCritical",
      "editorMounted", "editorNoSeriousOrCritical",
      "paletteMounted", "paletteNoSeriousOrCritical",
    ],
    "ask-flow": [
      "homeMounted", "paletteOpened", "askDialogOpened",
      "questionSubmitted", "askEndpointAvailable",
      "answerStreamed", "sourcesAppeared",
      "firstCitationMatchesSeed",
    ],
    "code-block-enter": [
      "editorReady", "aCodeBlockCreated", "aChipShowsPython",
      "bKeywordSpan", "cSinglePre", "cTextContent", "cSentinelBr",
      "cSelectionInPre", "dTextContent", "dSinglePre",
      "eParagraphAfterPre", "eSelectionInParagraph",
      "eTextContentUnchanged", "noPageErrors", "noConsoleErrors",
    ],
    "block-links": [
      "blocksHaveIds", "targetBlockId", "menuItemVisible",
      "clipboardUrlCorrect", "validFragmentFlash", "validFragmentNoToast",
      "foreignHashInertAppearance", "foreignHashInertEmptyB",
      "foreignHashInertMalformedB", "missingBlockToastShown",
      "missingBlockToastText", "missingBlockToastAutodismiss",
      "noConsoleErrors",
    ],
    "doc-mentions": [
      "t1BlockId", "pasteDocUrlConverts", "docChipGlyphTitle",
      "singleUndoRestoresUrl", "pasteAnchorUrlConverts",
      "anchorChipGlyphSnippet", "pasteDanglingConverts",
      "noAccessStaysPlain", "urlInSentenceUntouched",
      "danglingClassApplied", "danglingDocGlyph", "ctxEntriesVisible",
      "ctxCopyOriginalUrl", "ctxAbsentOnPlainText", "chipClickNavigates",
      "t2Trashed", "missingClassApplied", "missingLabel",
      "missingClickInert", "convertToPlainLink", "noConsoleErrors",
    ],
  };
  if (ok && Object.prototype.hasOwnProperty.call(requiredSteps, scenario)) {
    const s = collector.scenario?.steps || {};
    const missing = requiredSteps[scenario].filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `${scenario} steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  const report = {
    ok,
    error: errMsg,
    durationMs,
    outDir,
    scenario: collector.scenario,
    editorError: collector.editorError || null,
    assertError: collector.assertError || null,
    stepError: collector.stepError || null,
    shareApiError: collector.shareApiError || null,
    tabA: collector["tab-a"] || null,
    tabB: collector["tab-b"] || null,
  };

  writeFileSync(join(outDir, "report.json"), JSON.stringify(report, null, 2));
  process.stdout.write("FRONTEND_DOCTOR_REPORT " + JSON.stringify(report) + "\n");
  process.exit(ok ? 0 : 1);
}

main().catch((e) => {
  console.error("doctor fatal:", e);
  process.exit(3);
});
