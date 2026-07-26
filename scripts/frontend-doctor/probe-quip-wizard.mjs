// Ad-hoc probe: Quip import wizard token-entry step (Phase 0), plus an
// optional Phase 1 scope → Continue → progress-view scenario.
// Opens the wizard, confirms the token field renders, and drives a garbage
// token through the full UI→API→real-Quip→error path.
//
// The Phase 1 scenario (Continue → progress) needs a token that actually
// authenticates against a real (or wiremocked) Quip — the garbage-token
// path above can't get past `connect` by design. Pass one via
// `--quip-token <token>` or the `QUIP_TEST_TOKEN` env var; without either,
// the scenario is skipped (not failed) so this probe stays runnable
// against a plain local stack.
import { chromium } from "playwright";
import fs from "node:fs";

const args = Object.fromEntries(
  process.argv.slice(2).join(" ").split("--").filter(Boolean)
    .map((s) => s.trim().split(/\s+/)).map(([k, ...v]) => [k, v.join(" ")])
);
const BASE = args["base-url"] || "http://127.0.0.1:3100";
const OUT = args["out"] || "./probe-out";
fs.mkdirSync(OUT, { recursive: true });

const results = [];
const check = (n, ok, d = "") => { results.push({ n, ok, d }); console.log(`${ok ? "PASS" : "FAIL"} ${n}${d ? ` — ${d}` : ""}`); };

const browser = await chromium.launch();
const context = await browser.newContext();
const login = await context.request.post(`${BASE}/api/v1/auth/dev-login`, {
  data: { email: "verifier@test.com", name: "Verifier" },
});
if (!login.ok()) throw new Error(`dev-login ${login.status()}`);

const page = await context.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });

await page.goto(`${BASE}/`);
await page.waitForSelector(".sidebar", { timeout: 20000 });
await page.waitForTimeout(800);

// Open the sidebar "+ New" menu, then the "Import from Quip" entry.
await page.locator(".sidebar-header .toolbar-btn").first().click(); // the "+" new button
const quipItem = page.locator(".ui-menu-item", { hasText: /Quip/i });
const menuOk = await quipItem.first().waitFor({ timeout: 5000 }).then(() => true).catch(() => false);
check("import-from-quip-menu-entry", menuOk);
if (!menuOk) { fs.writeFileSync(`${OUT}/results.json`, JSON.stringify(results)); await browser.close(); process.exit(1); }
await quipItem.first().click();

// The wizard modal + token field render.
const tokenInput = page.locator('input[type="password"]').first();
check("token-input-renders", await tokenInput.waitFor({ timeout: 5000 }).then(() => true).catch(() => false));
await page.screenshot({ path: `${OUT}/1-wizard-token-step.png` });

// Garbage token → Connect → an error banner surfaces (full round trip).
// The banner is deliberately OPAQUE (status + x-request-id, never the
// response body) — so we assert the banner element appears, not any
// specific copy, and separately assert the profile step did NOT render.
await tokenInput.fill("quip-garbage-UI-SEEKRET-42");
const connectBtn = page.locator("button", { hasText: /Connect/i }).first();
await connectBtn.click();
const errBanner = page.locator('[role="alert"].template-picker-error');
const errShown = await errBanner.first().waitFor({ timeout: 15000 }).then(() => true).catch(() => false);
check("garbage-token-shows-error-banner", errShown,
  errShown ? (await errBanner.first().innerText()).slice(0, 80) : "");
// The connect must NOT advance to a connected/profile state on a bad token.
const advanced = await page.locator("text=/root folder/i").first()
  .isVisible().catch(() => false);
check("bad-token-does-not-advance", !advanced);
await page.screenshot({ path: `${OUT}/2-invalid-token-error.png` });

// The token string must not appear in the page's console errors.
check("no-token-in-console", !errors.some((e) => e.includes("SEEKRET")),
  errors.slice(0, 2).join(" | "));

// ─── Optional Phase 1 scenario: scope → Continue → progress ───
// Reuses the same wizard instance (re-opened fresh) with a real/mocked
// token so `connect` actually succeeds and the scope step renders.
const quipToken = args["quip-token"] || process.env.QUIP_TEST_TOKEN;
if (!quipToken) {
  console.log("SKIP quip-continue-to-progress — no --quip-token / QUIP_TEST_TOKEN set");
} else {
  // Close and reopen the wizard (per-open reset clears the prior garbage
  // token + error state) via the same "+ New" → "Import from Quip" path.
  await page.keyboard.press("Escape");
  await page.waitForTimeout(300);
  await page.locator(".sidebar-header .toolbar-btn").first().click();
  await page.locator(".ui-menu-item", { hasText: /Quip/i }).first().click();

  const retryTokenInput = page.locator('input[type="password"]').first();
  await retryTokenInput.waitFor({ timeout: 5000 });
  await retryTokenInput.fill(quipToken);
  await page.locator("button", { hasText: /Connect/i }).first().click();

  const scopeStep = page.locator(".quip-import-step-scope");
  const scopeOk = await scopeStep.waitFor({ timeout: 15000 }).then(() => true).catch(() => false);
  check("quip-token-connects-to-scope-step", scopeOk);

  if (scopeOk) {
    // Root folders default all-checked (see `do_connect`); just click
    // Continue once its disabled-gate (home folder loaded) clears.
    const continueBtn = page.locator(".quip-import-step-scope button", { hasText: /Continue/i });
    await continueBtn.waitFor({ timeout: 5000 }).catch(() => {});
    const enabled = await continueBtn.first().isEnabled({ timeout: 10000 }).catch(() => false);
    check("continue-button-enabled", enabled);
    if (enabled) {
      await continueBtn.first().click();
      const progressStep = page.locator(".quip-import-step-progress");
      const progressOk = await progressStep.waitFor({ timeout: 10000 }).then(() => true).catch(() => false);
      check("continue-advances-to-progress-step", progressOk);
      if (progressOk) {
        await page.screenshot({ path: `${OUT}/3-import-progress.png` });
        const totalLine = page.locator("[data-quip-import-total]");
        const totalOk = await totalLine.first().waitFor({ timeout: 60000 }).then(() => true).catch(() => false);
        check("progress-view-shows-item-count", totalOk,
          totalOk ? (await totalLine.first().innerText()).slice(0, 80) : "");
      }
    }
  }
}

fs.writeFileSync(`${OUT}/results.json`, JSON.stringify(results, null, 2));
await browser.close();
const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
process.exit(failed.length ? 1 : 0);
