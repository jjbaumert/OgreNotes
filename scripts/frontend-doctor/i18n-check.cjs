// Run the actual Trunk UI with local API fixtures; no account is needed.
// These checks cover rendering and local editing, not server persistence or collaboration.
// Generate fixtures with: cargo run --manifest-path frontend/Cargo.toml
//   --example i18n_fixtures > /tmp/ogre-i18n-fixtures.json
// Usage: node i18n-check.cjs [dist-directory] [fixtures.json]
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const dist = path.resolve(
  process.argv[2] || path.join(__dirname, "../../frontend/dist"),
);
assert.ok(
  fs.existsSync(path.join(dist, "index.html")),
  `Missing Trunk build: ${dist}`,
);
const fixtures = JSON.parse(
  fs.readFileSync(process.argv[3] || "/tmp/ogre-i18n-fixtures.json", "utf8"),
);
const origin = "https://ogre-i18n.test";
const taglines = {
  ar: "مستندات بأنياب.", de: "Dokumente mit Biss.",
  es: "Documentos con garra.", fr: "Des documents qui ont du mordant.",
  it: "Documenti con grinta.", "en-US": "Documents with teeth.",
};
// Offsets here deliberately use DOM UTF-16 units, including surrogate pairs.
async function selectText(page, blockId, start, end) {
  await page.locator(`[data-block-id="${blockId}"]`).evaluate((block, { start, end }) => {
    block.closest('[contenteditable="true"]').focus();
    const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT);
    const nodes = []; while (walker.nextNode()) nodes.push(walker.currentNode);
    const boundary = (offset) => {
      for (const node of nodes) {
        if (offset <= node.length) return [node, offset];
        offset -= node.length;
      }
      throw new Error("Selection beyond fixture text");
    };
    getSelection().setBaseAndExtent(...boundary(start), ...boundary(end));
    document.dispatchEvent(new Event("selectionchange"));
  }, { start, end });
}
async function expectBlockText(page, blockId, expected) {
  await page.waitForFunction(({ blockId, expected }) =>
    document.querySelector(`[data-block-id="${blockId}"]`)?.textContent === expected,
    { blockId, expected });
}
const mime = {
  ".html": "text/html",
  ".js": "application/javascript",
  ".wasm": "application/wasm",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".json": "application/json",
  ".webmanifest": "application/manifest+json",
};
(async () => {
  const browser = await chromium.launch({
    executablePath: process.env.CHROME_BIN || undefined,
    headless: true,
    args: ["--no-sandbox"],
  });
  try {
    const cases = [
      { name: "Arabic login", url: "/login?locale=ar", expected: "ar" },
      { name: "German login", url: "/login?locale=de", expected: "de" },
      { name: "Spanish login", url: "/login?locale=es", expected: "es" },
      { name: "French login", url: "/login?locale=fr", expected: "fr" },
      { name: "Italian login", url: "/login?locale=it", expected: "it" },
      { name: "English login", url: "/login?locale=en-US", expected: "en-US" },
      {
        name: "Invalid URL with German cache",
        url: "/login?locale=INVALID!",
        stored: "de",
        expected: "de",
      },
      {
        name: "Unsupported URL with German cache",
        url: "/login?locale=ja",
        stored: "de",
        expected: "de",
      },
      {
        name: "Malformed cache falls through to regional browser locale",
        url: "/login", stored: "INVALID!", browserLocale: "de-DE", expected: "de-DE",
      },
      {
        name: "Unsupported cache falls through to Arabic browser locale",
        url: "/login", stored: "ja", browserLocale: "ar-EG", expected: "ar-EG",
      },
      {
        name: "Unsupported browser locale uses English fallback",
        url: "/login", browserLocale: "ja-JP", expected: "en-US",
      },
      {
        name: "URL locale beats server preference and cache",
        url: "/settings?locale=fr#appearance", stored: "de", hint: "ar",
        auth: true, expected: "fr",
      },
      {
        name: "French server pref beats German cache",
        url: "/settings#appearance",
        stored: "de",
        hint: "fr",
        auth: true,
        expected: "fr",
      },
      {
        name: "Unsupported server pref with German cache",
        url: "/settings#appearance",
        stored: "de",
        hint: "ja",
        auth: true,
        expected: "de",
      },
      {
        name: "Regional German selector",
        url: "/settings?locale=de-DE#appearance",
        auth: true,
        expected: "de-DE",
      },
      {
        name: "Arabic settings coverage and labels",
        url: "/settings?locale=ar#appearance",
        auth: true,
        expected: "ar",
      },
      {
        name: "URL override then select German",
        url: "/settings?locale=fr&keep=yes#appearance",
        auth: true,
        expected: "de",
        switchTo: "de",
      },
      {
        name: "Selection survives failed preference save",
        url: "/settings?locale=fr&keep=yes#appearance",
        auth: true,
        hint: "fr",
        expected: "de",
        switchTo: "de",
        saveFailure: true,
      },
      {
        name: "Arabic import error",
        url: "/?locale=ar",
        auth: true,
        expected: "ar",
        home: true,
        drop: true,
      },
      ...["ar", "de"].flatMap((locale) =>
        ["document", "spreadsheet"].map((docType) => ({
          name: `${locale} ${docType} rendering`,
          url: `/d/fixture?locale=${locale}`,
          auth: true,
          expected: locale,
          docType,
        })),
      ),
      ...["en-GB", "es-MX", "fr-CA", "it-IT", "ar-EG"].map((tag) => ({
        name: `Regional ${tag} selector`,
        url: `/settings?locale=${tag}#appearance`,
        auth: true,
        expected: tag,
      })),
    ];
    const selectedCases = cases.filter((spec) =>
      !process.env.I18N_CASE || spec.name.includes(process.env.I18N_CASE));
    assert.ok(selectedCases.length, `No cases matched I18N_CASE=${process.env.I18N_CASE}`);
    for (const spec of selectedCases) {
      let hint = spec.hint;
      const saves = [];
      const errors = [];
      const context = await browser.newContext({
        locale: spec.browserLocale || "en-US",
        serviceWorkers: "block",
        viewport: { width: 1280, height: 900 },
      });
      context.setDefaultTimeout(15000);
      await context.addInitScript(
        ({ stored }) => {
          if (stored) localStorage.setItem("ogrenotes.locale", stored);
        },
        { stored: spec.stored },
      );
      await context.route("**/*", async (route) => {
        const url = new URL(route.request().url());
        if (url.origin !== origin) return route.abort();
        if (url.pathname.startsWith("/api/")) {
          let status = 200,
            body = {};
          if (url.pathname === "/api/v1/documents/fixture/content")
            return route.fulfill({
              status: 200,
              contentType: "application/octet-stream",
              body: Buffer.from(fixtures[spec.docType], "base64"),
            });
          if (url.pathname === "/api/v1/documents/fixture")
            body = {
              id: "fixture",
              title: "Fixture",
              docType: spec.docType,
              createdAt: 1,
              updatedAt: 1,
              canEdit: true,
            };
          else if (url.pathname.endsWith("/ws-token")) status = 403;
          else if (url.pathname.endsWith("/threads")) body = { threads: [] };
          else if (url.pathname === "/api/v1/auth/refresh") {
            if (!spec.auth) {
              status = 401;
              body = { error: "unauthorized" };
            } else
              body = {
                accessToken: "i18n-audit-local-token",
                userId: "audit-user",
                email: "audit@example.test",
                name: "Audit",
                uiPrefs: { locale: hint },
              };
          } else if (
            url.pathname === "/api/v1/users/me/prefs" &&
            route.request().method() === "PUT"
          ) {
            body = route.request().postDataJSON();
            saves.push(body);
            if (spec.saveFailure) status = 500;
            else hint = body.locale;
          } else if (url.pathname === "/api/v1/users/me") {
            body = {
              id: "audit-user",
              userId: "audit-user",
              name: "Audit",
              email: "audit@example.test",
              homeFolderId: "home",
              trashFolderId: "trash",
              privateFolderId: "private",
              uiPrefs: { locale: hint },
              avatarUrl: null,
              status: null,
            };
          } else if (url.pathname.includes("/folders/")) {
            body = {
              id: "home",
              title: "Home",
              color: 0,
              parentId: null,
              folderType: "home",
              createdAt: 1,
              updatedAt: 1,
              children: [],
            };
          } else if (
            url.pathname.includes("favorites") ||
            url.pathname.includes("collections") ||
            url.pathname.includes("workspaces")
          )
            body = [];
          else if (url.pathname.includes("notifications"))
            body = { notifications: [], unreadCount: 0 };
          else if (url.pathname.includes("chats")) body = { chats: [] };
          else if (url.pathname.includes("auth/config"))
            body = { devMode: true };
          return route.fulfill({
            status,
            contentType: "application/json",
            body: JSON.stringify(body),
          });
        }
        let relative = decodeURIComponent(url.pathname).replace(/^\//, "");
        let file = path.join(dist, relative);
        if (!fs.existsSync(file) || !fs.statSync(file).isFile())
          file = path.join(dist, "index.html");
        return route.fulfill({
          status: 200,
          contentType: mime[path.extname(file)] || "application/octet-stream",
          body: fs.readFileSync(file),
        });
      });
      const page = await context.newPage();
      page.on("pageerror", (e) => errors.push(e.message));
      await page.goto(origin + spec.url);
      await page.waitForSelector(
        spec.docType === "document"
          ? ".calendar-event"
          : spec.docType === "spreadsheet"
            ? ".spreadsheet-cell"
            : spec.home
              ? ".file-list"
              : spec.auth
                ? ".settings-page"
                : ".login-card",
        { timeout: 60000 },
      );
      if (spec.switchTo) {
        await Promise.all([
          page.waitForEvent("load", { timeout: 15000 }),
          page.locator("#locale-select").selectOption(spec.switchTo),
        ]);
        await page.waitForSelector(".settings-page");
      }
      if (spec.drop) {
        await page.evaluate(() => {
          const dt = new DataTransfer();
          dt.items.add(
            new File(["example"], "example.txt", { type: "text/plain" }),
          );
          document.dispatchEvent(
            new DragEvent("drop", {
              bubbles: true,
              cancelable: true,
              dataTransfer: dt,
            }),
          );
        });
        await page.waitForFunction(() =>
          [...document.querySelectorAll("[role=alert]")].some((x) =>
            x.textContent.includes("example.txt"),
          ),
        );
      }
      const result = await page.evaluate(() => ({
        lang: document.documentElement.lang,
        dir: document.documentElement.dir,
        tagline: document.querySelector(".login-subtitle")?.textContent,
        selector: document.querySelector("#locale-select")?.value,
        selectorIndex: document.querySelector("#locale-select")?.selectedIndex,
        tabs: [...document.querySelectorAll(".settings-tab")].map((x) =>
          x.textContent.trim(),
        ),
        englishRailLabels: [...document.querySelectorAll(".sidebar-icon-btn")]
          .map((x) => ({ title: x.title, label: x.getAttribute("aria-label") }))
          .filter((x) => ["Chats", "Profile"].includes(x.title)),
        alerts: [...document.querySelectorAll("[role=alert]")].map(
          (x) => x.textContent,
        ),
        localstorage: localStorage.getItem("ogrenotes.locale"),
      }));
      assert.equal(result.lang, spec.expected, spec.name);
      assert.equal(
        result.dir,
        spec.expected.startsWith("ar") ? "rtl" : "ltr",
        spec.name,
      );
      if (!spec.auth)
        assert.equal(result.tagline, taglines[spec.expected] || taglines[spec.expected.split("-")[0]], `${spec.name}: translated login text`);
      if (spec.auth && !spec.home && !spec.docType)
        assert.equal(
          result.selector,
          spec.expected.startsWith("en")
            ? "en-US"
            : spec.expected.split("-")[0],
          spec.name,
        );
      if (spec.expected === "ar") {
        assert.deepEqual(result.englishRailLabels, [], spec.name);
        if (spec.auth) {
          assert.equal(await page.locator('.sidebar-icon-btn[title="المحادثات"]').count(), 1,
            "Arabic chat rail label is present");
          assert.equal(await page.locator('.sidebar-icon-btn[title="المحادثات"]').getAttribute("aria-label"),
            "المحادثات", "Arabic chat rail accessible label");
        }
        assert.ok(!result.tabs.includes("Templates"), spec.name);
      }
      if (spec.drop)
        assert.ok(
          result.alerts.some(
            (x) =>
              x.includes("example.txt") &&
              /[\u0600-\u06ff]/.test(x) &&
              !x.includes("unsupported file type"),
          ),
          spec.name,
        );
      if (spec.switchTo) {
        assert.equal(result.localstorage, spec.switchTo);
        assert.equal(saves.at(-1).locale, spec.switchTo);
        const finalUrl = new URL(page.url());
        assert.equal(finalUrl.searchParams.get("locale"), spec.switchTo);
        assert.equal(finalUrl.searchParams.get("keep"), "yes");
        assert.equal(finalUrl.hash, "#appearance");
      }
      if (spec.docType === "document") {
        const paragraph = page.locator('[data-block-id="rtl-paragraph"]');
        assert.equal(await paragraph.getAttribute("dir"), "rtl");
        assert.equal(await paragraph.textContent(), "مرحبا Hello 123");
        const leftAligned = page.locator('[data-block-id="rtl-left-paragraph"]');
        assert.equal(await leftAligned.getAttribute("dir"), "rtl");
        assert.equal(await leftAligned.evaluate((el) => getComputedStyle(el).textAlign), "left");
        const title = await page.locator(".calendar-toolbar").textContent();
        assert.ok(
          spec.expected === "ar"
            ? /[\u0600-\u06ff]/.test(title)
            : title.includes("Mai"),
          title,
        );
        const event = page.locator(".calendar-event").first();
        assert.ok((await event.textContent()).includes(
          spec.expected === "ar" ? "(بلا عنوان)" : "(ohne Titel)"),
          "untitled calendar event has localized fallback");
        const handle = event.locator("[data-calendar-resize]");
        const box = await handle.boundingBox();
        const before = await event.boundingBox();
        const direction = spec.expected === "ar" ? -1 : 1;
        assert.ok(
          direction === -1
            ? box.x < before.x + before.width / 2
            : box.x > before.x + before.width / 2,
          "handle at logical end",
        );
        await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
        await page.mouse.down();
        await page.mouse.move(
          box.x + box.width / 2 + direction * 25,
          box.y + box.height / 2,
          { steps: 5 },
        );
        assert.ok(
          (await event.boundingBox()).width > before.width + 15,
          "resize preview extends in logical direction",
        );
        await page.mouse.move(
          box.x + box.width / 2 - direction * 20,
          box.y + box.height / 2,
          { steps: 5 },
        );
        assert.ok(
          (await event.boundingBox()).width < before.width - 10,
          "resize preview shortens in logical direction",
        );
        await page.mouse.up();
      }
      if (spec.docType === "document") {
        const codeLabels = spec.expected === "ar"
          ? ["لغة الكتلة البرمجية", "نص عادي"]
          : ["Sprache des Codeblocks", "Nur Text"];
        await selectText(page, "code", 1, 1);
        const chip = page.locator(".code-lang-chip select");
        await chip.waitFor();
        assert.equal(await chip.getAttribute("aria-label"), codeLabels[0]);
        assert.equal(await chip.locator('option[value=""]').textContent(), codeLabels[1]);

        const blockId = "mixed-selection";
        const original = "مرحبا 😀Hello עולם 123";
        const selected = "😀Hello עולם";
        const start = original.indexOf(selected), end = start + selected.length;
        const mixed = page.locator(`[data-block-id="${blockId}"]`);
        assert.equal(await mixed.getAttribute("dir"), "rtl");
        assert.equal(await mixed.textContent(), original);
        // A backward selection crosses an emoji, a bold run, and Hebrew.
        await selectText(page, blockId, end, start);
        assert.equal(await page.evaluate(() => getSelection().toString()), selected);
        await page.keyboard.press("Control+i");
        await page.waitForFunction(({ blockId, selected }) =>
          [...document.querySelectorAll(`[data-block-id="${blockId}"] em`)]
            .map(el => el.textContent).join("") === selected,
          { blockId, selected });
        assert.equal(await mixed.textContent(), original, "formatting preserves mixed Unicode text");
        // Reselect forward through the rebuilt DOM, then replace via actual input.
        await selectText(page, blockId, start, end);
        const replacement = "🧭Latin שלום";
        await page.keyboard.insertText(replacement);
        const edited = original.slice(0, start) + replacement + original.slice(end);
        await expectBlockText(page, blockId, edited);
        await page.keyboard.press("Control+z");
        await expectBlockText(page, blockId, original);
        await page.keyboard.press("Control+Shift+z");
        await expectBlockText(page, blockId, edited);
        assert.equal(await mixed.getAttribute("dir"), "rtl", "editing retains paragraph direction");
      }
      if (spec.docType === "spreadsheet") {
        const cellA = page.locator('.spreadsheet-cell[data-row="0"][data-col="0"]');
        const cellB = page.locator('.spreadsheet-cell[data-row="0"][data-col="1"]');
        const aBox = await cellA.boundingBox();
        const bBox = await cellB.boundingBox();
        assert.ok(
          spec.expected === "ar" ? aBox.x > bBox.x : aBox.x < bBox.x,
          "column A must render at inline-start",
        );
        const rowHeader = await page.locator('.spreadsheet-grid tbody tr[data-row="0"] .spreadsheet-row-header').boundingBox();
        assert.ok(spec.expected === "ar" ? rowHeader.x > aBox.x : rowHeader.x < aBox.x,
          "row header must stay at the grid's inline-start edge");
        const resize = await page.locator(".col-resize-handle").first().boundingBox();
        const x = resize.x + resize.width / 2;
        const y = resize.y + resize.height / 2;
        await page.mouse.move(x, y);
        await page.mouse.down();
        await page.mouse.move(x + (spec.expected === "ar" ? -35 : 35), y, { steps: 5 });
        await page.mouse.up();
        assert.ok((await cellA.boundingBox()).width > aBox.width + 20,
          "column resize must grow toward inline-end");
        await page
          .locator('.spreadsheet-cell[data-row="0"][data-col="0"]')
          .click();
        await page
          .locator('.spreadsheet-cell[data-row="0"][data-col="1"]')
          .click({ modifiers: ["Shift"] });
        await page.waitForSelector(".ss-status-bar");
        const status = await page.locator(".ss-status-bar").textContent();
        const expected = await page.evaluate(
          (locale) =>
            new Intl.NumberFormat(locale, { maximumFractionDigits: 4 }).format(
              1236.5,
            ),
          spec.expected,
        );
        assert.ok(
          status.includes(expected),
          `${status} should contain ${expected}`,
        );
        await cellB.click();
        await page.keyboard.press("ArrowRight");
        const active = page.locator(".spreadsheet-cell.cursor").first();
        assert.equal(await active.getAttribute("data-col"), spec.expected === "ar" ? "0" : "2");
        await cellA.click();
        await page.keyboard.press("Tab");
        assert.equal(await active.getAttribute("data-col"), "1", "Tab advances in column order");
        await page.keyboard.press("Shift+Tab");
        assert.equal(await active.getAttribute("data-col"), "0", "Shift+Tab reverses column order");
        if (spec.expected === "ar") {
          await cellB.click();
          await page.keyboard.press("Control+ArrowRight");
          assert.equal(await active.getAttribute("data-col"), "0", "Ctrl+Right follows RTL visual direction");
        }
      }
      assert.deepEqual(errors, [], `${spec.name}: browser errors including interaction handlers`);
      console.log(`PASS ${spec.name}`);
      await context.close();
    }
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
