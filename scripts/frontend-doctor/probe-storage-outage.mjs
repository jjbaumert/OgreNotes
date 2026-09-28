// Ad-hoc probe: the server loses its storage mid-edit (self-hosting PR 1).
// Pauses the DynamoDB Local container to simulate a dropped connection,
// types, checks the banner + "Not saved" badge + fail-fast 503, unpauses,
// and checks the edit is saved and survives a reload.
import { chromium } from "playwright";
import { execSync } from "node:child_process";
import fs from "node:fs";

const BASE = process.env.BASE || "http://127.0.0.1:3100";
const OUT = process.env.OUT || "./probe-out-outage";
const DDB = process.env.DDB_CONTAINER || "ogrenotes-dynamodb-local-1";
fs.mkdirSync(OUT, { recursive: true });

const results = [];
const check = (n, ok, d = "") => { results.push({ n, ok }); console.log(`${ok ? "PASS" : "FAIL"} ${n}${d ? ` — ${d}` : ""}`); };
const docker = (cmd) => execSync(`docker ${cmd} ${DDB}`, { stdio: "ignore" });

const browser = await chromium.launch();
const context = await browser.newContext();
const login = await context.request.post(`${BASE}/api/v1/auth/dev-login`, { data: { email: "outage@test.com" } });
const { accessToken } = await login.json();
const created = await context.request.post(`${BASE}/api/v1/documents`, {
  headers: { Authorization: `Bearer ${accessToken}` },
  data: { title: "Outage probe", doc_type: "document" },
});
const docId = (await created.json()).id;
console.log("doc", docId);

const page = await context.newPage();
page.on("console", (m) => { if (/error|warn|collab/i.test(m.type() + m.text())) console.log("console:", m.type(), m.text().slice(0, 200)); });
await page.goto(`${BASE}/d/${docId}/probe`);
await page.waitForSelector('[contenteditable="true"]', { timeout: 30000 });
await page.waitForTimeout(1500);
await page.click('[contenteditable="true"]');
await page.keyboard.type("before ");
await page.waitForSelector(".sync-indicator.is-saved", { timeout: 15000 });
check("saved before the outage", true);

try {
  docker("pause");
  const t0 = Date.now();
  await page.keyboard.type("during-outage");
  await page.waitForSelector(".sync-indicator.is-not-saved", { timeout: 60000 });
  check("badge says Not saved", true, `${Math.round((Date.now() - t0) / 1000)}s after typing`);
  await page.waitForSelector(".storage-banner", { timeout: 10000 });
  check("storage banner shown", true, (await page.textContent(".storage-banner")).trim());
  await page.screenshot({ path: `${OUT}/outage.png` });

  // Wait for the server's own probe to notice, then the API must fail fast.
  let status = "";
  for (let i = 0; i < 30 && status !== "unreachable"; i++) {
    status = (await (await context.request.get(`${BASE}/api/v1/status`)).json()).storage;
    if (status !== "unreachable") await page.waitForTimeout(1000);
  }
  check("status endpoint reports unreachable", status === "unreachable");
  const t1 = Date.now();
  const r = await context.request.get(`${BASE}/api/v1/folders`, { headers: { Authorization: `Bearer ${accessToken}` } });
  check("API fails fast with tagged 503", r.status() === 503 && r.headers()["x-ogrenotes-storage"] === "unavailable",
    `${r.status()} in ${Date.now() - t1}ms`);
  // A tab opened during the outage must wait, not bounce to /login.
  var during = await context.newPage();
  await during.goto(`${BASE}/d/${docId}/probe`);
  await during.waitForSelector(".storage-banner", { timeout: 30000 });
  check("page opened during the outage explains and waits", !during.url().includes("/login"), during.url());
} finally {
  docker("unpause");
}
await during.waitForSelector('[contenteditable="true"]', { timeout: 60000 }).then(
  () => check("that page continues to the document once storage is back", true),
  () => check("that page continues to the document once storage is back", false, during.url()),
);

await page.waitForSelector(".storage-banner", { state: "detached", timeout: 60000 });
check("banner clears once storage is back", true);
try {
  await page.waitForSelector(".sync-indicator.is-saved", { timeout: 60000 });
  check("badge returns to Saved", true);
} catch {
  check("badge returns to Saved", false, await page.getAttribute(".sync-indicator", "class"));
}
await page.screenshot({ path: `${OUT}/recovered.png` });

// A fresh page must see the text typed during the outage.
await page.waitForTimeout(2000);
const fresh = await context.newPage();
await fresh.goto(`${BASE}/d/${docId}/probe`);
try {
  await fresh.waitForSelector('[contenteditable="true"]', { timeout: 30000 });
} catch (e) {
  await fresh.screenshot({ path: `${OUT}/fresh-fail.png` });
  console.log("fresh page stuck at", fresh.url(), "body:", (await fresh.textContent("body")).slice(0, 300));
  throw e;
}
await fresh.waitForTimeout(1500);
const text = await fresh.textContent('[contenteditable="true"]');
check("edit typed during the outage survived", text.includes("during-outage"), JSON.stringify(text));

await browser.close();
const failed = results.filter((r) => !r.ok).length;
console.log(failed ? `${failed} FAILED` : "ALL PASS");
process.exit(failed ? 1 : 0);
