// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
// Real document editor + two principals + persisted Yrs reload + view-only user.
// node scripts/frontend-doctor/test-embedded-spreadsheet.cjs --base-url http://127.0.0.1:3000
const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
// The nightly browser stack intentionally omits PDF to bound cold build time.
const skipPdf = args.includes('--skip-pdf');
const base = args.includes('--base-url') ? args[args.indexOf('--base-url') + 1] : 'http://127.0.0.1:3000';
assert(['127.0.0.1', 'localhost'].includes(new URL(base).hostname), 'Use an isolated local dev stack');
const out = args.includes('--out') ? args[args.indexOf('--out') + 1] : '/tmp/ogrenotes-embedded-spreadsheet';
const phase = message => console.log(`RUN ${message}`);
async function screenshotFailure(page, label) {
  fs.mkdirSync(out, { recursive: true });
  try {
    const filename = path.join(out, `${label}.png`);
    await page.screenshot({ path: filename, fullPage: true, timeout: 5000 });
    console.error(`FAIL screenshot ${filename} (${page.url()})`);
  } catch (error) { console.error(`FAIL screenshot unavailable: ${error.message}`); }
}
const edgeCase = args.includes('--edge-case') ? args[args.indexOf('--edge-case') + 1] : null;
const editorSelector = '.editor-content[data-editor-ready]';
const gridSelector = '.embedded-spreadsheet';
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, message) {
  const end = Date.now() + 15000;
  let last;
  do {
    try { last = await check(); if (last) return; } catch (error) { last = error.message; }
    await pause(50);
  } while (Date.now() < end);
  throw new Error(`${message}: ${JSON.stringify(last)}`);
}
async function api(context, method, route, data, token) {
  const response = await context.request.fetch(`${base}/api/v1${route}`, {
    method, data, headers: token ? { Authorization: `Bearer ${token}` } : {},
  });
  assert(response.ok(), `${method} ${route}: ${response.status()} ${await response.text()}`);
  return response.headers()['content-type']?.includes('application/json') ? response.json() : null;
}
function assertExport(text, format) {
  const normalized = text.replace(/\s+/g, ' ');
  for (const value of ['Before spreadsheet', 'After spreadsheet', 'Sheet1', 'Budget', '$3.00', 'second sheet', 'independent workbook', 'owner cell', 'peer cell']) {
    assert(normalized.includes(value), `${format} export contains ${JSON.stringify(value)}`);
  }
  assert(!normalized.includes('=A1+B1'), `${format} export shows computed formula output`);
}
async function verifyPersistedExports(owner, id, identities) {
  const headers = { Authorization: `Bearer ${owner.auth.accessToken}` };
  // These reads load storage snapshots plus durable UPDATE rows. All browser
  // pages have closed, so the old live editor room cannot mask missing saves.
  await until(async () => {
    const response = await owner.context.request.get(`${base}/api/v1/documents/${id}/export/html`, { headers });
    if (!response.ok()) return false;
    const html = await response.text();
    return ['owner cell', 'peer cell', 'second sheet', 'independent workbook', '$3.00'].every(value => html.includes(value));
  }, 'All edits become visible through persisted public exports after pages close');
  const content = await owner.context.request.get(`${base}/api/v1/documents/${id}/content`, { headers });
  assert(content.ok(), `GET native content: ${content.status()}`);
  const nativeBytes = await content.body();
  assert(nativeBytes.length > 0, 'Native content is a full binary state');
  const formats = skipPdf ? ['html', 'markdown', 'docx'] : ['html', 'markdown', 'docx', 'pdf'];
  if (skipPdf) console.log('SKIP PDF export: explicitly disabled for the browser-only CI build; backend PDF regressions run in regular CI');
  for (const format of formats) {
    phase(`persisted public ${format} export`);
    const response = await owner.context.request.get(`${base}/api/v1/documents/${id}/export/${format}`, { headers });
    assert(response.ok(), `${format} export: ${response.status()} ${response.ok() ? '' : await response.text()}`);
    const bytes = await response.body();
    if (format === 'docx') {
      assert.equal(bytes.subarray(0, 2).toString(), 'PK', 'DOCX is an actual ZIP package');
      const extracted = JSON.parse(execFileSync('python3', ['-c', [
        'import io,json,sys,zipfile,xml.etree.ElementTree as E',
        'root=E.fromstring(zipfile.ZipFile(io.BytesIO(sys.stdin.buffer.read())).read("word/document.xml"))',
        'ns={"w":"http://schemas.openxmlformats.org/wordprocessingml/2006/main"}',
        'print(json.dumps({"text":" ".join(t.text or "" for t in root.findall(".//w:t",ns)),"tables":len(root.findall(".//w:tbl",ns))}))',
      ].join('\n')], { input: bytes, maxBuffer: 8 * 1024 * 1024 }).toString());
      assert(extracted.tables >= 3, 'DOCX contains tables for every embedded sheet');
      assertExport(extracted.text, format);
    } else if (format === 'pdf') {
      assert.equal(bytes.subarray(0, 4).toString(), '%PDF', 'PDF is an actual PDF document');
      const text = execFileSync('pdftotext', ['-layout', '-', '-'], { input: bytes, maxBuffer: 8 * 1024 * 1024 }).toString();
      assertExport(text, format);
    } else {
      const text = bytes.toString();
      assertExport(text, format);
      if (format === 'html') assert((text.match(/<table\b/g) || []).length >= 3, 'HTML contains every sheet table');
    }
  }
  phase('native binary download/upload roundtrip');
  await verifyNativeRoundtrip(owner, nativeBytes, identities);
  console.log(`PASS persisted native Yrs roundtrip and actual ${formats.join('/')} public API exports after all clients close`);
}
async function verifyNativeRoundtrip(owner, bytes, identities) {
  let clone;
  let page;
  try {
    clone = (await api(owner.context, 'POST', '/documents', { title: 'Embedded spreadsheet native roundtrip' }, owner.auth.accessToken)).id;
    const uploaded = await owner.context.request.put(`${base}/api/v1/documents/${clone}/content`, {
      data: bytes,
      headers: { Authorization: `Bearer ${owner.auth.accessToken}`, 'Content-Type': 'application/octet-stream' },
    });
    assert(uploaded.ok(), `PUT native content clone: ${uploaded.status()} ${uploaded.ok() ? '' : await uploaded.text()}`);
    page = await owner.context.newPage();
    await page.goto(`${base}/d/${clone}/native-roundtrip`);
    await page.locator(editorSelector).waitFor();
    await page.locator('.sync-indicator.is-saved').waitFor();
    assert.equal(await page.locator(gridSelector).count(), 2, 'Native upload restores both workbooks');
    for (const [index, identity] of identities.entries()) {
      const book = grid(page, index);
      assert.equal(await book.getAttribute('data-block-id'), identity.id, 'Workbook identity survives binary download/upload');
      const sheets = await book.locator('[data-sheet-id]').evaluateAll(tabs => tabs.map(tab => ({ id: tab.dataset.sheetId, name: tab.textContent })));
      assert.deepEqual(sheets, identity.sheets, 'All native sheet names and stable identities survive download/upload');
    }
    const first = grid(page);
    await cell(first, 2, 0).click();
    assert.equal(await first.locator('[data-sheet-action=raw]').inputValue(), '=A1+B1', 'Formula source retained in native binary');
    assert.equal(await first.locator('[data-sheet-action=format]').inputValue(), 'currency', 'Number format retained in native binary');
    await expectCell(first, 2, 0, '$3.00');
    for (const [col, row, expected] of [[0, 0, '1'], [1, 0, '2'], [0, 3, 'owner cell'], [1, 3, 'peer cell']]) {
      await cell(first, col, row).click();
      assert.equal(await first.locator('[data-sheet-action=raw]').inputValue(), expected, 'Independent cell source retained in native binary');
    }
    await switchSheet(first, 'Budget');
    await expectCell(first, 0, 0, 'second sheet');
    await cell(first, 0, 0).click();
    assert.equal(await first.locator('[data-sheet-action=raw]').inputValue(), 'second sheet');
    await expectCell(grid(page, 1), 0, 0, 'independent workbook');
    const paragraphs = await page.locator(`${editorSelector} > p`).allTextContents();
    assert(paragraphs.some(text => text.startsWith('Before spreadsheet')) && paragraphs.includes('After spreadsheet'), 'Native upload restores surrounding prose');
  } catch (error) {
    if (page) await screenshotFailure(page, 'native-roundtrip-failure');
    throw error;
  } finally {
    if (page) await page.close();
    if (clone) {
      await api(owner.context, 'DELETE', `/documents/${clone}`, undefined, owner.auth.accessToken);
      await api(owner.context, 'DELETE', `/documents/${clone}/purge`, undefined, owner.auth.accessToken);
    }
  }
}
async function selectParagraph(page, index, offset = 0) {
  await page.locator(editorSelector).evaluate((editor, { index, offset }) => {
    editor.focus();
    const p = editor.querySelectorAll(':scope > p')[index];
    if (!p) throw new Error(`Missing paragraph ${index}`);
    const node = p.firstChild?.nodeType === Node.TEXT_NODE ? p.firstChild : p;
    const selection = getSelection();
    selection.setBaseAndExtent(node, offset, node, offset);
    document.dispatchEvent(new Event('selectionchange'));
  }, { index, offset });
}
async function insertGrid(page, expectedCount) {
  await page.keyboard.press('Control+Shift+P');
  const search = page.locator('.search-dialog input');
  await search.waitFor();
  await search.fill('>Insert spreadsheet');
  await until(async () => (await page.locator('.search-dialog').innerText()).includes('Insert spreadsheet'), 'Insert command appears');
  await search.press('Enter');
  await until(async () => await page.locator(gridSelector).count() === expectedCount, 'Spreadsheet inserted');
}
const grid = (page, index = 0) => page.locator(gridSelector).nth(index);
const cell = (book, col, row) => book.locator(`[data-sheet-action=cell][data-col="${col}"][data-row="${row}"]`);
async function edit(book, col, row, raw) {
  await cell(book, col, row).click();
  const input = book.locator('[data-sheet-action=raw]');
  await input.fill(raw);
  await input.press('Enter');
}
async function expectCell(book, col, row, text) {
  await until(async () => await cell(book, col, row).textContent() === text, `Cell ${col},${row} must display ${text}`);
}
async function switchSheet(book, name) {
  await book.locator('.embedded-sheet-tabs').getByRole('button', { name, exact: true }).click();
}
// Focused user-level regressions can run against the pre-fix distribution:
// --edge-case undo-removal | rejected-navigation
async function runEdgeCase(page, scenario) {
  assert(['undo-removal', 'rejected-navigation'].includes(scenario), `Unknown edge case ${scenario}`);
  phase(`edge case ${scenario}`);
  await selectParagraph(page, 0);
  await insertGrid(page, 1);
  const book = grid(page);
  await cell(book, 0, 0).click();
  if (scenario === 'undo-removal') {
    await page.keyboard.press('Control+z');
    await until(async () => await page.locator(gridSelector).count() === 0, 'Undo removes focused workbook');
    const context = await page.locator(editorSelector).evaluate(editor => ({
      activeMarker: editor.getAttribute('data-spreadsheet-active'),
      focusedTag: document.activeElement?.tagName,
      focusedInside: editor.contains(document.activeElement),
      selectionInside: editor.contains(getSelection()?.focusNode),
    }));
    console.log(`Undo-removal focus context ${JSON.stringify(context)}`);
    // Use physical key events before any protocol insertText can implicitly
    // focus the editing host. Capture the immediate post-undo state separately.
    await page.keyboard.press('Control+End');
    await page.keyboard.type('Resumed prose after undo');
    const prose = await page.locator(`${editorSelector} > p`).allTextContents();
    console.log(`Undo-removal physical typing ${JSON.stringify(prose)}`);
    assert(prose.some(text => text.includes('Resumed prose after undo')), 'Physical typing resumes in prose after undo removes workbook');
    assert(context.selectionInside, 'Undo restores a DOM selection inside prose');
    assert(context.focusedInside, 'Undo immediately restores editor focus');
    assert.equal(context.activeMarker, null, 'Undo immediately releases removed workbook identity without requiring later prose focus');
    assert.equal(await page.locator(editorSelector).getAttribute('data-spreadsheet-active'), null, 'Removed workbook leaves no stale active marker');
    console.log('PASS undo of focused workbook restores normal prose typing');
  } else {
    await edit(book, 0, 0, 'original A1');
    await cell(book, 0, 0).click();
    const raw = book.locator('[data-sheet-action=raw]');
    await raw.fill('😀'.repeat(1100));
    await cell(book, 1, 0).click();
    await book.locator('.embedded-sheet-error[role=alert]').waitFor();
    const rejectedFocusRestored = await raw.evaluate(input => document.activeElement === input);
    await page.keyboard.press('ArrowRight');
    await raw.fill('corrected original A1');
    await raw.press('Enter');
    console.log(`Rejected-navigation focus restored=${rejectedFocusRestored}, A1=${JSON.stringify(await cell(book, 0, 0).textContent())}, B1=${JSON.stringify(await cell(book, 1, 0).textContent())}`);
    await expectCell(book, 0, 0, 'corrected original A1');
    await expectCell(book, 1, 0, '');
    assert(rejectedFocusRestored, 'Rejected cell click restores raw draft focus');
    console.log('PASS rejected draft remains associated with original cell after attempted keyboard navigation');
  }
}
// Hold only peer update frames. Both writes still traverse the actual server,
// while neither client sees the other's value before making its own edit.
async function socketGate(page) {
  const gate = { held: false, pending: [], synced: false };
  await page.routeWebSocket('**/api/v1/documents/*/ws', socket => {
    const server = socket.connectToServer();
    server.onMessage(message => {
      if (Buffer.isBuffer(message) && message[0] === 2) gate.synced = true;
      if (Buffer.isBuffer(message) && message[0] === 3 && gate.held) {
        gate.pending.push(() => socket.send(message)); return;
      }
      socket.send(message);
    });
  });
  gate.release = () => { gate.held = false; for (const send of gate.pending.splice(0)) send(); };
  return gate;
}
(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_BIN || undefined });
  const users = [];
  const errors = [];
  let id;
  try {
    phase('login and owner/editor/viewer permissions');
    for (const role of ['owner', 'editor', 'viewer']) {
      const context = await browser.newContext({ bypassCSP: true, serviceWorkers: 'block', viewport: { width: 1500, height: 1000 } });
      const auth = await api(context, 'POST', '/auth/dev-login', { email: `embedded-sheet-${role}-${Date.now()}@example.test`, name: `Embedded sheet ${role}` });
      users.push({ context, auth });
    }
    const owner = users[0];
    id = (await api(owner.context, 'POST', '/documents', { title: 'Embedded spreadsheet acceptance' }, owner.auth.accessToken)).id;
    for (const [index, accessLevel] of [[1, 'EDIT'], [2, 'VIEW']]) {
      await api(owner.context, 'POST', `/documents/${id}/members`, { userId: users[index].auth.userId, accessLevel }, owner.auth.accessToken);
    }
    const pages = await Promise.all(users.map(user => user.context.newPage()));
    pages.forEach(page => page.on('pageerror', error => errors.push(error.message)));
    const [a, b, viewer] = pages;
    const gates = await Promise.all([a, b].map(socketGate));
    const open = async page => {
      await page.goto(`${base}/d/${id}/embedded-spreadsheet`);
      await page.locator(editorSelector).waitFor();
      await page.locator('.sync-indicator.is-saved').waitFor();
    };
    await open(a);
    await until(() => gates[0].synced, 'Owner completes initial Yrs sync');
    if (edgeCase) {
      await runEdgeCase(a, edgeCase);
      assert.deepEqual(errors, []);
      return;
    }
    phase('insert workbook between prose and calculate formula');
    await selectParagraph(a, 0);
    await a.keyboard.insertText('Before spreadsheet');
    await a.keyboard.press('Enter');
    await a.keyboard.press('Enter');
    await a.keyboard.insertText('After spreadsheet');
    await selectParagraph(a, 1);
    await insertGrid(a, 1);
    await until(async () => {
      const paragraphs = await a.locator(`${editorSelector} > p`).allTextContents();
      return paragraphs.includes('Before spreadsheet') && paragraphs.includes('After spreadsheet');
    }, 'Prose remains before and after the grid');
    await edit(grid(a), 0, 0, '1');
    await edit(grid(a), 1, 0, '2');
    await edit(grid(a), 2, 0, '=A1+B1');
    await expectCell(grid(a), 2, 0, '3');
    console.log('PASS formula C1 calculates 3');
    phase('raw draft commits when clicking prose and survives reload');
    // Leaving the raw editor for prose commits through the same Yrs/history
    // path as Enter. A fresh reload must retain that visible value.
    await cell(grid(a), 3, 0).click();
    await grid(a).locator('[data-sheet-action=raw]').fill('committed on prose click');
    await a.locator(`${editorSelector} > p`).filter({ hasText: 'After spreadsheet' }).click();
    await expectCell(grid(a), 3, 0, 'committed on prose click');
    await until(async () => {
      const response = await owner.context.request.get(`${base}/api/v1/documents/${id}/export/html`, { headers: { Authorization: `Bearer ${owner.auth.accessToken}` } });
      return response.ok() && (await response.text()).includes('committed on prose click');
    }, 'Focusout edit reaches durable storage before reload');
    await a.locator('.sync-indicator.is-saved').waitFor();
    await a.reload();
    await a.locator(editorSelector).waitFor();
    await expectCell(grid(a), 3, 0, 'committed on prose click');
    console.log('PASS prose focusout edit survives persisted reload');
    phase('rejected draft remains visible with error');
    // Multibyte input can fit the HTML character limit while exceeding the
    // native byte budget. Rejected drafts remain visible and clearly marked.
    await cell(grid(a), 4, 0).click();
    const invalidDraft = grid(a).locator('[data-sheet-action=raw]');
    const oversized = '😀'.repeat(1100);
    await invalidDraft.fill(oversized);
    await a.locator(`${editorSelector} > p`).filter({ hasText: 'After spreadsheet' }).click();
    await grid(a).locator('.embedded-sheet-error[role=alert]').waitFor();
    assert.equal(await invalidDraft.inputValue(), oversized, 'Rejected draft remains visible');
    assert.equal(await invalidDraft.getAttribute('aria-invalid'), 'true');
    await expectCell(grid(a), 4, 0, '');
    await invalidDraft.press('Escape');
    await cell(grid(a), 2, 0).click();
    const format = grid(a).locator('[data-sheet-action=format]');
    phase('numeric pattern and named number format');
    await format.fill('0.00');
    await format.press('Tab');
    await expectCell(grid(a), 2, 0, '3.00');
    console.log('PASS advertised numeric pattern 0.00 displays 3.00');
    await cell(grid(a), 2, 0).click();
    await format.fill('currency');
    await format.press('Tab');
    await expectCell(grid(a), 2, 0, '$3.00');
    await grid(a).locator('[data-sheet-action=add]').click();
    await switchSheet(grid(a), 'Sheet2');
    await edit(grid(a), 0, 0, 'second sheet');
    const sheetName = grid(a).locator('[data-sheet-action=name]');
    await sheetName.fill('Budget');
    await grid(a).locator('[data-sheet-action=rename]').click();
    await switchSheet(grid(a), 'Sheet1');
    await expectCell(grid(a), 2, 0, '$3.00');

    console.log('PASS currency format and add/rename/switch sheet');
    phase('independent workbook and parent undo/redo');
    const afterIndex = await a.locator(`${editorSelector} > p`).evaluateAll(nodes => nodes.findIndex(node => node.textContent === 'After spreadsheet'));
    await selectParagraph(a, afterIndex, 'After spreadsheet'.length);
    await a.keyboard.press('Enter');
    await insertGrid(a, 2);
    await edit(grid(a, 1), 0, 0, 'independent workbook');
    await expectCell(grid(a), 0, 0, '1');
    await edit(grid(a), 0, 1, 'undo me');
    await cell(grid(a), 0, 1).press('Control+z');
    await expectCell(grid(a), 0, 1, '');
    await cell(grid(a), 0, 1).press('Control+Shift+z');
    await expectCell(grid(a), 0, 1, 'undo me');

    console.log('PASS independent workbook and parent undo/redo');
    phase('peer sync and focused draft preservation');
    await open(b);
    await until(() => gates[1].synced, 'Peer completes initial Yrs sync');
    await expectCell(grid(b), 2, 0, '$3.00');
    // Focused uncommitted draft survives a collaborator's unrelated prose write.
    await cell(grid(a), 0, 2).click();
    const draft = grid(a).locator('[data-sheet-action=raw]');
    await draft.fill('=A1+B1+10');
    await draft.evaluate(input => { window.__embeddedDraftInput = input; });
    await selectParagraph(b, 0, 'Before spreadsheet'.length);
    await b.keyboard.insertText(' updated remotely');
    await until(async () => (await a.locator(`${editorSelector} > p`).first().textContent()).endsWith('updated remotely'), 'Peer prose reaches owner');
    assert(await draft.evaluate(input => input === window.__embeddedDraftInput && document.activeElement === input), 'Input identity and focus survive unrelated remote prose');
    assert.equal(await draft.inputValue(), '=A1+B1+10');
    await expectCell(grid(a), 0, 2, '');
    await expectCell(grid(b), 0, 2, '');
    await draft.press('Escape');

    console.log('PASS remote prose preserves draft input/focus without committing it');
    phase('simultaneous disjoint-cell collaboration');
    gates.forEach(gate => { gate.held = true; });
    await edit(grid(a), 0, 3, 'owner cell');
    await edit(grid(b), 1, 3, 'peer cell');
    await until(() => gates.every(gate => gate.pending.length > 0), 'Both disjoint edits cross the real WebSocket server');
    gates.forEach(gate => gate.release());
    for (const page of [a, b]) {
      await expectCell(grid(page), 0, 3, 'owner cell');
      await expectCell(grid(page), 1, 3, 'peer cell');
    }
    console.log('PASS concurrent cell registers converge in both editors');
    phase('viewer read-only grid');
    await open(viewer);
    await viewer.locator('.editor-content-readonly').waitFor();
    await expectCell(grid(viewer), 2, 0, '$3.00');
    assert(await grid(viewer).locator('[data-sheet-action=raw]').evaluate(input => input.readOnly));
    assert(await grid(viewer).locator('[data-sheet-action=add]').isDisabled());
    assert(await grid(viewer).locator('[data-sheet-action=rename]').isDisabled());
    await cell(grid(viewer), 0, 0).press('x');
    await expectCell(grid(viewer), 0, 0, '1');

    console.log('PASS viewer cannot edit workbook');
    phase('original document persisted reload');
    // A newly opened third context also proves server persistence before reload.
    await a.reload();
    await a.locator(editorSelector).waitFor();
    await expectCell(grid(a), 2, 0, '$3.00');
    await cell(grid(a), 2, 0).click();
    assert.equal(await grid(a).locator('[data-sheet-action=raw]').inputValue(), '=A1+B1');
    assert.equal(await grid(a).locator('[data-sheet-action=format]').inputValue(), 'currency');
    await switchSheet(grid(a), 'Budget');
    await expectCell(grid(a), 0, 0, 'second sheet');
    await switchSheet(grid(a), 'Sheet1');
    await expectCell(grid(a), 0, 3, 'owner cell');
    await expectCell(grid(a), 1, 3, 'peer cell');
    await expectCell(grid(a, 1), 0, 0, 'independent workbook');
    // Capture identities before closing every client. Download the durable
    // native bytes, then upload them into a new document and verify that clone.
    const identities = [];
    for (const index of [0, 1]) {
      const book = grid(a, index);
      const sheets = await book.locator('[data-sheet-id]').evaluateAll(tabs => tabs.map(tab => ({ id: tab.dataset.sheetId, name: tab.textContent })));
      identities.push({ id: await book.getAttribute('data-block-id'), sheets });
    }
    for (const page of pages) await page.locator('.sync-indicator.is-saved').waitFor();
    console.log('PASS reload retains source, formats, sheets and both workbooks');
    phase('close all clients and validate durable public APIs');
    await Promise.all(pages.map(page => page.close()));
    await verifyPersistedExports(owner, id, identities);
    assert.deepEqual(errors, []);
    console.log('PASS embedded spreadsheet formulas, formats, sheets, independent blocks, history, draft preservation, concurrent Yrs edits, read-only and reload');
  } catch (error) {
    console.error(`FAIL ${error.stack || error.message}`);
    console.error(`FAIL pageerror summary: ${JSON.stringify(errors)}`);
    for (const [index, user] of users.entries()) {
      for (const [pageIndex, page] of user.context.pages().entries()) {
        await screenshotFailure(page, `client-${index}-page-${pageIndex}-failure`);
      }
    }
    throw error;
  } finally {
    try {
      if (id) {
        await api(users[0].context, 'DELETE', `/documents/${id}`, undefined, users[0].auth.accessToken);
        await api(users[0].context, 'DELETE', `/documents/${id}/purge`, undefined, users[0].auth.accessToken);
      }
    } finally {
      for (const user of users) await user.context.close();
      await browser.close();
    }
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
