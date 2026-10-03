import assert from "node:assert/strict";
import { readFileSync, mkdtempSync, mkdirSync, writeFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import vm from "node:vm";
import { evaluateScenarioResult } from "./scenario-result.js";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const doctorSource = readFileSync(new URL("./doctor.js", import.meta.url), "utf8");
const mainSource = doctorSource.slice(doctorSource.indexOf("async function main()"));
const regressedScenarios = [
  ["collab-sync", "scenarioCollabSync"],
  ["calendar-block", "scenarioCalendarBlock"],
  ["kanban-block", "scenarioKanbanBlock"],
  ["kanban-drag", "scenarioKanbanDrag"],
  ["kanban-column-reorder", "scenarioKanbanColumnReorder"],
  ["kanban-wip-limit", "scenarioKanbanWipLimit"],
  ["kanban-card-metadata", "scenarioKanbanCardMetadata"],
  ["type-past-atom", "scenarioTypePastAtom"],
];

// Execute the production main entry point through JSON output and exit status.
// Only the browser scenario is replaced with deterministic observations; the
// result gate and CLI report wiring remain production code.
async function runDoctor(scenario, functionName, runScenario) {
  const out = mkdtempSync(join(tmpdir(), "ogre-doctor-gate-"));
  const exits = [];
  let output = "";
  try {
    await vm.runInNewContext(mainSource, {
      parseArgs: () => ({ scenario, "base-url": "http://fixture.invalid", "doc-id": "d1", out }),
      [functionName]: runScenario,
      existsSync, mkdirSync, writeFileSync, join, evaluateScenarioResult,
      console: { error() {} },
      process: { argv: [], stdout: { write(text) { output += text; } }, exit(code) { exits.push(code); } },
    });
    assert.equal(exits.length, 1, "CLI must finish with one exit status");
    const report = JSON.parse(readFileSync(join(out, "report.json"), "utf8"));
    assert.ok(output.includes("FRONTEND_DOCTOR_REPORT "));
    return { report, exitCode: exits[0] };
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
}

for (const [scenario, functionName] of regressedScenarios) {
  test(`${scenario}: a caught browser failure writes a failed report and exits nonzero`, async () => {
    const result = await runDoctor(scenario, functionName, async (_, collector) => {
      collector.scenario = { name: scenario, steps: {}, syncObservedInB: false, remoteCursorObservedInB: false };
      collector.stepError = "injected browser selector timeout";
    });
    assert.equal(result.exitCode, 1);
    assert.equal(result.report.ok, false);
    assert.match(result.report.error, /injected browser selector timeout/);
  });
}

for (const [scenario, functionName] of regressedScenarios.slice(1)) {
  const start = doctorSource.indexOf(`async function ${functionName}(`);
  const end = doctorSource.indexOf("\nasync function ", start + 1);
  const body = doctorSource.slice(start, end < 0 ? undefined : end);
  // Discover observations from the scenario producer, independently of the
  // verdict's requiredSteps table. Adding a producer observation without
  // gating it, or removing its gate, must fail this contract test.
  const keys = [...new Set([...body.matchAll(/\bsteps\.([A-Za-z0-9_]+)\s*=/g)].map((m) => m[1]))];
  assert.ok(keys.length > 0, `${scenario} must emit observations`);
  const passingSteps = Object.fromEntries(keys.map((key) => [key, true]));
  test(`${scenario}: successful observations exit zero`, async () => {
    const result = await runDoctor(scenario, functionName, async (_, collector) => {
      collector.scenario = { name: scenario, steps: passingSteps };
    });
    assert.equal(result.exitCode, 0);
    assert.equal(result.report.ok, true);
  });
  for (const key of keys) {
    for (const value of [false, undefined]) {
      test(`${scenario}: ${key}=${value} fails without a thrown error`, async () => {
        const result = await runDoctor(scenario, functionName, async (_, collector) => {
          collector.scenario = { name: scenario, steps: { ...passingSteps, [key]: value } };
        });
        assert.equal(result.exitCode, 1);
        assert.equal(result.report.ok, false);
        assert.ok(result.report.error.includes(key), result.report.error);
      });
    }
  }
}

for (const key of ["syncObservedInB", "remoteCursorObservedInB"]) {
  test(`collab-sync: ${key}=false fails even without an exception`, async () => {
    const result = await runDoctor("collab-sync", "scenarioCollabSync", async (_, collector) => {
      collector.scenario = { name: "collab-sync", syncObservedInB: true, remoteCursorObservedInB: true, [key]: false };
    });
    assert.equal(result.exitCode, 1);
    assert.ok(result.report.error.includes(key));
  });
}

test("collab-sync: synchronized text and visible peer cursor pass", async () => {
  const result = await runDoctor("collab-sync", "scenarioCollabSync", async (_, collector) => {
    collector.scenario = { name: "collab-sync", syncObservedInB: true, remoteCursorObservedInB: true };
  });
  assert.equal(result.exitCode, 0);
});

const selfAssertingScenarios = [
  ["semantic-search", "scenarioSemanticSearch"],
  ["import-job-round-trip", "scenarioImportJobRoundTrip"],
  ["menu-export-downloads", "scenarioMenuExportDownloads"],
  ["pre-sync-edit-preserved", "scenarioPreSyncEditPreserved"],
  ["sustained-type-reload", "scenarioSustainedTypeReload"],
  ["first-keystrokes", "scenarioFirstKeystrokes"],
];
for (const [scenario, functionName] of selfAssertingScenarios) {
  test(`${scenario}: successful self-asserting scenarios do not need a steps map`, async () => {
    const result = await runDoctor(scenario, functionName, async () => {});
    assert.equal(result.exitCode, 0);
  });
  test(`${scenario}: thrown assertions retain a failed CLI result`, async () => {
    const result = await runDoctor(scenario, functionName, async () => {
      throw new Error("injected scenario assertion");
    });
    assert.equal(result.exitCode, 1);
    assert.match(result.report.error, /injected scenario assertion/);
  });
}

for (const field of ["stepError", "editorError", "assertError", "shareApiError"]) {
  test(`caught ${field} fails even when all observations passed`, () => {
    const result = evaluateScenarioResult("collab-sync", {
      scenario: { syncObservedInB: true, remoteCursorObservedInB: true },
      [field]: "injected caught error",
    });
    assert.equal(result.ok, false);
    assert.match(result.error, /injected caught error/);
  });
}

test("existing required-step contracts remain effective", () => {
  const goodSteps = new Proxy({}, { get: () => true });
  assert.equal(evaluateScenarioResult("trash-flow", { scenario: { steps: goodSteps } }).ok, true);
  assert.equal(evaluateScenarioResult("trash-flow", { scenario: { steps: {} } }).ok, false);
});

test("an uncaught browser panic still fails a self-asserting scenario", async () => {
  const result = await runDoctor("semantic-search", "scenarioSemanticSearch", async (_, collector) => {
    collector["tab-a"] = { errors: [{ message: "WASM panic" }] };
  });
  assert.equal(result.exitCode, 1);
  assert.match(result.report.error, /WASM panic/);
});

test("a previously thrown error is not overwritten by caught errors", () => {
  const result = evaluateScenarioResult("calendar-block", { stepError: "secondary" }, false, "original failure");
  assert.deepEqual(result, { ok: false, error: "original failure" });
});

// Run the unmodified CLI with real Chromium against a deliberately small HTTP
// fixture. This validates scenario catch paths, artifacts, and OS exit status,
// independently of the VM contract tests above. Opt in after installing the
// package dependencies and Playwright Chromium: npm run test:browser.
test("browser failures reach the CLI report and process exit status", {
  skip: process.env.DOCTOR_BROWSER_TESTS !== "1",
  timeout: 120000,
}, async (t) => {
  let marker = "";
  const server = createServer(async (req, res) => {
    const url = new URL(req.url, "http://localhost");
    if (url.pathname.endsWith("/auth/dev-login")) {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ userId: "fixture", accessToken: "fixture" }));
    } else if (url.pathname.endsWith("/documents")) {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ id: "fixture" }));
    } else if (url.pathname.endsWith("/marker")) {
      if (req.method === "POST") {
        marker = "";
        for await (const chunk of req) marker += chunk;
      }
      res.end(marker);
    } else {
      const mode = url.pathname.split("/")[1];
      res.setHeader("content-type", "text/html");
      res.end(`<!doctype html><body>
        ${mode === "missing-editor" ? "" : '<div contenteditable="true">Fixture editor</div>'}
        ${mode === "missing-cursor" ? "" : '<div class="remote-cursor-caret">Peer</div>'}
        <div id="peer-text"></div>
        <script>
          const editor = document.querySelector('[contenteditable="true"]');
          if (editor) editor.addEventListener('input', () => {
            fetch('/${mode}/marker', {method: 'POST', body: editor.innerText});
          });
          ${mode === "missing-sync" ? "" : `setInterval(async () => {
            document.getElementById('peer-text').textContent = await (await fetch('/${mode}/marker')).text();
          }, 100);`}
        </script></body>`);
    }
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const cases = [
    ["collab-sync", "success", null],
    ["collab-sync", "missing-editor", /could not find \/ type/],
    ["collab-sync", "missing-sync", /syncObservedInB/],
    ["collab-sync", "missing-cursor", /remoteCursorObservedInB/],
    ["calendar-block", "missing-editor", /waitForSelector/],
  ];
  for (const [scenario, mode, expectedError] of cases) {
    await t.test(`${scenario}: ${mode}`, async () => {
      marker = "";
      const out = mkdtempSync(join(tmpdir(), "ogre-doctor-browser-"));
      try {
        const cli = fileURLToPath(new URL("./doctor.js", import.meta.url));
        const child = spawn(process.execPath, [cli, "--scenario", scenario,
          "--base-url", `http://127.0.0.1:${server.address().port}/${mode}`,
          "--doc-id", "fixture", "--out", out], { stdio: ["ignore", "pipe", "pipe"] });
        let stdout = "";
        let stderr = "";
        child.stdout.on("data", (data) => { stdout += data; });
        child.stderr.on("data", (data) => { stderr += data; });
        const timer = setTimeout(() => child.kill("SIGKILL"), 30000);
        const exitCode = await new Promise((resolve, reject) => {
          child.once("error", reject);
          child.once("close", resolve);
        }).finally(() => clearTimeout(timer));
        assert.ok(existsSync(join(out, "report.json")), stderr);
        const report = JSON.parse(readFileSync(join(out, "report.json"), "utf8"));
        assert.equal(exitCode, expectedError ? 1 : 0, JSON.stringify(report));
        assert.equal(report.ok, !expectedError);
        if (expectedError) assert.match(report.error, expectedError);
        assert.ok(stdout.includes("FRONTEND_DOCTOR_REPORT "));
        assert.ok(existsSync(join(out, "tab-a.png")), "screenshot survives failure");
        assert.ok(existsSync(join(out, "tab-a.har")), "network diagnostics survive failure");
        if (scenario === "calendar-block") assert.match(report.stepError, /waitForSelector/);
      } finally {
        rmSync(out, { recursive: true, force: true });
      }
    });
  }
});
