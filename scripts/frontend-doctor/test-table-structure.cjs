// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
// Browser regression for inserting rows and columns in a document table.
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const args = process.argv.slice(2);
const base = args.includes('--base-url') ? args[args.indexOf('--base-url') + 1] : 'http://127.0.0.1:3000';

async function moveCaretAwayFromRightClick(page) {
  const selectedCell = await page.evaluate(() => {
    const cell = document.querySelector('.editor-content table tr:first-child td:first-child');
    const range = document.createRange();
    range.setStart(cell, 0);
    range.collapse(true);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    return selection.anchorNode.closest('td') === cell;
  });
  assert(selectedCell, 'browser caret is in the first cell, away from the right-click target');
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext();
  let viewerContext;
  let id;
  let token;
  try {
    const login = await context.request.post(`${base}/api/v1/auth/dev-login`, {
      data: { email: `table-structure-${Date.now()}@ogrenotes.example.com`, name: 'Table regression' },
    });
    assert(login.ok(), `dev-login: ${login.status()}`);
    token = (await login.json()).accessToken;
    const created = await context.request.post(`${base}/api/v1/documents`, {
      data: { title: 'Table structure regression' },
      headers: { Authorization: `Bearer ${token}` },
    });
    assert(created.ok(), `create document: ${created.status()}`);
    id = (await created.json()).id;

    const page = await context.newPage();
    await page.goto(`${base}/d/${id}/table-regression`);
    await page.locator('.editor-content[data-editor-ready=true]').waitFor();
    await page.locator('.editor-content').click();
    await page.keyboard.press('Control+Shift+P');
    const palette = page.locator('.search-dialog input');
    await palette.waitFor();
    await palette.fill('>table');
    await page.locator('.command-item-label', { hasText: 'Insert table' }).waitFor();
    await palette.press('Enter');
    const table = page.locator('.editor-content table').first();
    await table.waitFor();
    assert.equal(await table.locator('tr').count(), 3);
    assert.equal(await table.locator('tr').first().locator('td,th').count(), 3);

    await table.locator('tr').first().locator('td,th').nth(2).click();
    await page.keyboard.type('far');
    await table.locator('tr').first().locator('td,th').first().click();
    await table.locator('tr').nth(1).locator('td,th').nth(1).click({ button: 'right' });
    const tableMenu = page.getByRole('menuitem', { name: /^Table/ });
    await tableMenu.waitFor({ timeout: 3000 });
    await tableMenu.hover();
    await moveCaretAwayFromRightClick(page);
    await page.getByRole('menuitem', { name: 'Insert row below' }).click();
    assert.equal(await table.locator('tr').count(), 4);
    const caretRow = await page.evaluate(() => {
      const row = window.getSelection()?.anchorNode?.parentElement?.closest('tr');
      return row ? [...row.parentElement.children].indexOf(row) : -1;
    });
    assert.equal(caretRow, 2, 'new row follows the right-clicked second row');

    await table.locator('tr').first().locator('td,th').nth(2).click({ button: 'right' });
    await tableMenu.waitFor();
    await tableMenu.hover();
    await moveCaretAwayFromRightClick(page);
    await page.getByRole('menuitem', { name: 'Insert column right' }).click();
    for (const row of await table.locator('tr').all()) {
      assert.equal(await row.locator('td,th').count(), 4);
    }
    assert.equal(await table.locator('tr').first().locator('td,th').nth(2).innerText(), 'far');
    assert.equal((await table.locator('tr').first().locator('td,th').nth(3).innerText()).trim(), '');
    await page.waitForFunction(async ({ base, id, token }) => {
      const response = await fetch(`${base}/api/v1/documents/${id}/export/html`, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!response.ok) return false;
      const html = await response.text();
      return (html.match(/<tr\b/g) || []).length === 4
        && (html.match(/<t[dh]\b/g) || []).length === 16
        && html.includes('far');
    }, { base, id, token }, { timeout: 15000, polling: 100 });
    await page.reload();
    await page.locator('.editor-content[data-editor-ready=true]').waitFor();
    assert.equal(await table.locator('tr').count(), 4);
    for (const row of await table.locator('tr').all()) {
      assert.equal(await row.locator('td,th').count(), 4);
    }

    const peer = await context.newPage();
    await peer.goto(`${base}/d/${id}/table-regression`);
    await peer.locator('.editor-content[data-editor-ready=true]').waitFor();
    await table.locator('tr').last().locator('td,th').last().click({ button: 'right' });
    await tableMenu.waitFor();
    const peerText = `peer-${Date.now()}`;
    await peer.locator('.editor-content > p').first().click();
    await peer.keyboard.type(peerText);
    await page.locator('.editor-content > p', { hasText: peerText }).first().waitFor();
    await page.locator('.ui-menu').first().waitFor({ state: 'detached' });
    await peer.close();

    await page.locator('.editor-content > p').first().click({ button: 'right' });
    await page.locator('.ui-menu').first().waitFor();
    assert.equal(await page.getByRole('menuitem', { name: /^Table/ }).count(), 0);
    await page.keyboard.press('Escape');

    viewerContext = await browser.newContext();
    const viewerLogin = await viewerContext.request.post(`${base}/api/v1/auth/dev-login`, {
      data: { email: `table-viewer-${Date.now()}@ogrenotes.example.com`, name: 'Table viewer' },
    });
    assert(viewerLogin.ok(), `viewer dev-login: ${viewerLogin.status()}`);
    const viewer = await viewerLogin.json();
    const share = await context.request.post(`${base}/api/v1/documents/${id}/members`, {
      data: { userId: viewer.userId, accessLevel: 'VIEW' },
      headers: { Authorization: `Bearer ${token}` },
    });
    assert(share.ok(), `share document: ${share.status()}`);
    const viewerPage = await viewerContext.newPage();
    await viewerPage.goto(`${base}/d/${id}/table-regression`);
    const readonlyCell = viewerPage.locator('.editor-content-readonly table td').first();
    await readonlyCell.waitFor();
    await readonlyCell.click({ button: 'right' });
    await viewerPage.evaluate(() => new Promise(resolve =>
      requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert.equal(await viewerPage.getByRole('menuitem', { name: /^Table/ }).count(), 0);
    console.log('PASS document table row and column insertion');
  } finally {
    if (id && token) {
      const headers = { Authorization: `Bearer ${token}` };
      await context.request.delete(`${base}/api/v1/documents/${id}`, { headers });
      await context.request.delete(`${base}/api/v1/documents/${id}/purge`, { headers });
    }
    if (viewerContext) await viewerContext.close();
    await context.close();
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
