// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
// Browser regression for structural insertion in a pasted merged document table.
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const args = process.argv.slice(2);
const base = args.includes('--base-url') ? args[args.indexOf('--base-url') + 1] : 'http://127.0.0.1:3000';

async function insertFromTableMenu(page, cell, action) {
  for (let attempt = 0; attempt < 3; attempt++) {
    await cell.click({ button: 'right' });
    try {
      const menu = page.getByRole('menuitem', { name: /^Table/ });
      await menu.waitFor({ timeout: 1500 });
      await menu.hover({ timeout: 1500 });
      await page.getByRole('menuitem', { name: action }).click({ timeout: 1500 });
      return;
    } catch (error) {
      // An initial collaboration update can close a just-opened menu after
      // reload. Retry against the new DOM; a persistent failure still fails.
      if (attempt === 2) throw error;
    }
  }
}

async function originalCellIds(table) {
  return table.locator('td,th').evaluateAll(cells => Object.fromEntries(cells
    .map(cell => [cell.textContent.trim(), cell.getAttribute('data-block-id')])
    .filter(([text]) => /^[A-F]$/.test(text))));
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1600, height: 900 } });
  let id;
  let emptyRowId;
  let rowGroupId;
  let token;
  try {
    const login = await context.request.post(`${base}/api/v1/auth/dev-login`, {
      data: { email: `merged-table-${Date.now()}@ogrenotes.example.com`, name: 'Merged table regression' },
    });
    assert(login.ok(), `dev-login: ${login.status()}`);
    token = (await login.json()).accessToken;
    const created = await context.request.post(`${base}/api/v1/documents`, {
      data: { title: 'Merged table structure regression' },
      headers: { Authorization: `Bearer ${token}` },
    });
    assert(created.ok(), `create document: ${created.status()}`);
    id = (await created.json()).id;

    const page = await context.newPage();
    await page.goto(`${base}/d/${id}/merged-table-regression`);
    const editor = page.locator('.editor-content[data-editor-ready=true]');
    await editor.waitFor();
    await editor.locator('> p').first().click();
    await page.evaluate(() => {
      const editor = document.querySelector('.editor-content');
      const data = new DataTransfer();
      data.setData('text/html', '<table><tbody><tr><td colspan=" 2 " rowspan=" 2 ">A</td><td>B</td></tr><tr><td>C</td></tr><tr><td>D</td><td>E</td><td>F</td></tr></tbody></table>');
      const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
      Object.defineProperty(event, 'clipboardData', { value: data });
      editor.dispatchEvent(event);
    });
    const table = editor.locator('table').first();
    await table.waitFor();
    const rows = table.locator('tr');
    assert.equal(await rows.count(), 3);
    const merged = rows.first().locator('td,th').first();
    assert.equal(await merged.getAttribute('colspan'), '2');
    assert.equal(await merged.getAttribute('rowspan'), '2');
    const originalIds = await originalCellIds(table);
    assert.deepEqual(Object.keys(originalIds).sort(), ['A', 'B', 'C', 'D', 'E', 'F']);
    assert(Object.values(originalIds).every(Boolean), 'original cell block IDs are present');

    await insertFromTableMenu(page, rows.nth(2).locator('td,th').first(), 'Insert column right');
    assert.equal(await merged.getAttribute('colspan'), '3');
    assert.equal(await rows.nth(2).locator('td,th').count(), 4);
    assert.equal(await rows.nth(2).locator('td,th').nth(2).innerText(), 'E');

    await insertFromTableMenu(page, rows.first().locator('td,th').nth(1), 'Insert row below');
    assert.equal(await rows.count(), 4);
    assert.equal(await merged.getAttribute('rowspan'), '3');
    assert.equal(await rows.nth(1).locator('td,th').count(), 1);
    assert.equal((await rows.nth(1).locator('td,th').first().innerText()).trim(), '');
    assert.equal(await rows.nth(2).locator('td,th').first().innerText(), 'C');

    // Inserting from A itself must use the visual edge of its 3-column,
    // 3-row span, rather than its first physical row or column.
    await insertFromTableMenu(page, merged, 'Insert column right');
    assert.equal(await rows.first().locator('td,th').count(), 3);
    assert.equal((await rows.first().locator('td,th').nth(1).innerText()).trim(), '');
    assert.equal(await rows.first().locator('td,th').nth(2).innerText(), 'B');
    await insertFromTableMenu(page, merged, 'Insert row below');
    assert.equal(await rows.count(), 5);
    assert.equal(await rows.nth(3).locator('td,th').count(), 5);
    assert.equal(await rows.nth(4).locator('td,th').first().innerText(), 'D');

    await page.waitForFunction(async ({ base, id, token }) => {
      const response = await fetch(`${base}/api/v1/documents/${id}/export/html`, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!response.ok) return false;
      const html = await response.text();
      return (html.match(/<tr\b/g) || []).length === 5
        && html.includes('colspan="3"') && html.includes('rowspan="3"')
        && ['A', 'B', 'C', 'D', 'E', 'F'].every(text => html.includes(`>${text}<`));
    }, { base, id, token }, { timeout: 15000, polling: 100 });
    await page.reload();
    await editor.waitFor();
    assert.equal(await rows.count(), 5);
    assert.equal(await merged.getAttribute('colspan'), '3');
    assert.equal(await merged.getAttribute('rowspan'), '3');
    assert.equal(await rows.nth(2).locator('td,th').nth(1).innerText(), 'C');
    assert.equal(await rows.nth(4).locator('td,th').first().innerText(), 'D');
    assert.deepEqual(await originalCellIds(table), originalIds, 'all original cell IDs survive Yrs reload');

    // HTML rowspan=0 reaches the end of its row group. The second row has
    // no own cells, but must survive paste for later structural edits.
    const second = await context.request.post(`${base}/api/v1/documents`, {
      data: { title: 'Rowspan zero regression' },
      headers: { Authorization: `Bearer ${token}` },
    });
    assert(second.ok(), `create second document: ${second.status()}`);
    emptyRowId = (await second.json()).id;
    const emptyPage = await context.newPage();
    await emptyPage.goto(`${base}/d/${emptyRowId}/rowspan-zero-regression`);
    const emptyEditor = emptyPage.locator('.editor-content[data-editor-ready=true]');
    await emptyEditor.waitFor();
    await emptyEditor.locator('> p').first().click();
    await emptyPage.evaluate(() => {
      const editor = document.querySelector('.editor-content');
      const data = new DataTransfer();
      data.setData('text/html', '<table><tbody><tr><td rowspan="0">Only</td></tr><tr></tr></tbody></table>');
      const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
      Object.defineProperty(event, 'clipboardData', { value: data });
      editor.dispatchEvent(event);
    });
    const emptyTable = emptyEditor.locator('table').first();
    await emptyTable.waitFor();
    const emptyRows = emptyTable.locator('tr');
    assert.equal(await emptyRows.count(), 2);
    const only = emptyRows.first().locator('td,th').first();
    assert.equal(await only.getAttribute('rowspan'), '2');
    await emptyPage.waitForFunction(async ({ base, id, token }) => {
      const response = await fetch(`${base}/api/v1/documents/${id}/export/html`, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!response.ok) return false;
      const html = await response.text();
      return (html.match(/<tr\b/g) || []).length === 2 && html.includes('rowspan="2"');
    }, { base, id: emptyRowId, token }, { timeout: 15000, polling: 100 });
    await emptyPage.reload();
    await emptyEditor.waitFor();
    await emptyPage.locator('.sync-indicator.is-saved').waitFor();
    assert.equal(await emptyRows.count(), 2);
    assert.equal(await only.getAttribute('rowspan'), '2');
    await insertFromTableMenu(emptyPage, only, 'Insert column right');
    assert.equal(await emptyRows.first().locator('td,th').count(), 2);
    assert.equal(await emptyRows.nth(1).locator('td,th').count(), 1);
    await emptyPage.waitForFunction(async ({ base, id, token }) => {
      const response = await fetch(`${base}/api/v1/documents/${id}/export/html`, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!response.ok) return false;
      const html = await response.text();
      return (html.match(/<tr\b/g) || []).length === 2
        && (html.match(/<td\b/g) || []).length === 3;
    }, { base, id: emptyRowId, token }, { timeout: 15000, polling: 100 });
    await emptyPage.reload();
    await emptyEditor.waitFor();
    assert.equal(await emptyRows.count(), 2);
    assert.equal(await only.getAttribute('rowspan'), '2');
    assert.equal(await emptyRows.nth(1).locator('td,th').count(), 1);

    // A span in thead must stop at the row-group boundary before tbody.
    const third = await context.request.post(`${base}/api/v1/documents`, {
      data: { title: 'Row group regression' },
      headers: { Authorization: `Bearer ${token}` },
    });
    assert(third.ok(), `create third document: ${third.status()}`);
    rowGroupId = (await third.json()).id;
    const groupPage = await context.newPage();
    await groupPage.goto(`${base}/d/${rowGroupId}/row-group-regression`);
    const groupEditor = groupPage.locator('.editor-content[data-editor-ready=true]');
    await groupEditor.waitFor();
    await groupEditor.locator('> p').first().click();
    await groupPage.evaluate(() => {
      const editor = document.querySelector('.editor-content');
      const data = new DataTransfer();
      data.setData('text/html', '<table><thead><tr><th rowspan="2">H</th><th>H2</th></tr></thead><tbody><tr><td>B1</td><td>B2</td></tr></tbody></table>');
      const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
      Object.defineProperty(event, 'clipboardData', { value: data });
      editor.dispatchEvent(event);
    });
    const groupTable = groupEditor.locator('table').first();
    await groupTable.waitFor();
    const groupRows = groupTable.locator('tr');
    assert.equal(await groupRows.count(), 2);
    assert.equal(await groupRows.first().locator('th').first().evaluate(cell => cell.rowSpan), 1);
    await insertFromTableMenu(groupPage, groupRows.nth(1).locator('td').first(), 'Insert column right');
    assert.equal(await groupRows.first().locator('th').count(), 3);
    assert.equal(await groupRows.nth(1).locator('td').count(), 3);
    console.log('PASS merged document table row and column insertion');
  } finally {
    if (id && token) {
      const headers = { Authorization: `Bearer ${token}` };
      await context.request.delete(`${base}/api/v1/documents/${id}`, { headers });
      await context.request.delete(`${base}/api/v1/documents/${id}/purge`, { headers });
      if (emptyRowId) {
        await context.request.delete(`${base}/api/v1/documents/${emptyRowId}`, { headers });
        await context.request.delete(`${base}/api/v1/documents/${emptyRowId}/purge`, { headers });
      }
      if (rowGroupId) {
        await context.request.delete(`${base}/api/v1/documents/${rowGroupId}`, { headers });
        await context.request.delete(`${base}/api/v1/documents/${rowGroupId}/purge`, { headers });
      }
    }
    await context.close();
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
