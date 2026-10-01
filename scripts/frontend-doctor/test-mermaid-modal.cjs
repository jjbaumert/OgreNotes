// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
// Production-editor regression for #288. Requires a local DEV_MODE stack.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');
const args = process.argv.slice(2);
const option = (key, fallback) => args.includes(key) ? args[args.indexOf(key) + 1] : fallback;
const base = option('--base-url', 'http://127.0.0.1:3000');
const out = option('--out', '/tmp/ogrenotes-mermaid-modal');
fs.mkdirSync(out, { recursive: true });
const original = 'graph TD\nA --> Original\n';
const remote = 'graph TD\nA --> Remote';
const draft = 'graph TD\nA --> Draft';
const source = page => page.locator('.editor-content > .mermaid-block').getAttribute('data-source');
const waitSource = (page, expected) => page.waitForFunction(
  value => document.querySelector('.editor-content > .mermaid-block')?.dataset.source === value, expected,
);
const save = page => page.locator('.mermaid-modal button.btn-primary').click();
const cancel = page => page.locator('.mermaid-modal button.btn-secondary').click();

async function api(context, method, route, data, token) {
  const response = await context.request.fetch(base + '/api/v1' + route, {
    method, data, headers: token ? { Authorization: 'Bearer ' + token } : {},
  });
  assert(response.ok(), `${method} ${route}: ${response.status()} ${await response.text()}`);
  return response.headers()['content-type']?.includes('application/json') ? response.json() : null;
}
async function checkLayout(page, label) {
  const results = await page.locator('.mermaid-modal').evaluate((modal, label) => {
    const elements = [modal, ...modal.querySelectorAll(
      '.mermaid-modal-body, textarea, .calendar-modal-actions, button, [role=alert]',
    ), ...(label.startsWith('parser-') ? modal.querySelectorAll('.mermaid-preview, .mermaid-error') : [])];
    return elements.map(el => {
      const rect = el.getBoundingClientRect();
      return { element: el.className, left: rect.left, right: rect.right,
        client: el.clientWidth, scroll: el.scrollWidth, viewport: innerWidth };
    });
  }, label);
  for (const result of results) {
    assert(result.left >= -1 && result.right <= result.viewport + 1, `${label}: ${JSON.stringify(result)}`);
    assert(result.scroll <= result.client + 1, `${label}: horizontal overflow: ${JSON.stringify(result)}`);
  }
  await page.screenshot({ path: path.join(out, label + '.png') });
  console.log('PASS ' + label);
}

(async () => {
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROME_BIN ? { executablePath: process.env.CHROME_BIN } : {}) });
  const contexts = [];
  let owner, ownerAuth, documentId;
  let trashed = false;
  const errors = [];
  try {
    const users = [];
    for (const name of ['owner', 'editor', 'viewer']) {
      const context = await browser.newContext({ bypassCSP: true });
      contexts.push(context);
      const auth = await api(context, 'POST', '/auth/dev-login', {
        email: `mermaid-${name}-${Date.now()}@ogrenotes.example.com`, name: 'Mermaid test ' + name,
      });
      users.push({ context, auth });
    }
    owner = users[0].context;
    ownerAuth = users[0].auth;
    const created = await api(owner, 'POST', '/documents', { title: 'Mermaid modal regression' }, ownerAuth.accessToken);
    documentId = created.id;
    for (const [index, accessLevel] of [[1, 'EDIT'], [2, 'VIEW']]) {
      await api(owner, 'POST', `/documents/${documentId}/members`, {
        userId: users[index].auth.userId, accessLevel,
      }, ownerAuth.accessToken);
    }
    const url = `${base}/d/${documentId}/probe`;
    const pages = [];
    for (const [index, user] of users.entries()) {
      const page = await user.context.newPage();
      page.on('pageerror', e => errors.push(e.message));
      await page.goto(url);
      await page.locator(index === 2 ? '.editor-content-readonly' : '.editor-content[contenteditable=true]').waitFor();
      await page.locator('.sync-indicator.is-saved').waitFor();
      pages.push(page);
    }
    const [a] = pages;
    await a.locator('[contenteditable=true]').focus();
    await a.evaluate(text => {
      const dt = new DataTransfer();
      dt.setData('text/plain', '```mermaid\n' + text + '```');
      document.querySelector('[contenteditable=true]').dispatchEvent(
        new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }),
      );
    }, original);
    for (const page of pages) await waitSource(page, original);
    const [, b, viewer] = pages;
    await viewer.locator('.editor-content > .mermaid-block').click();
    assert.equal(await viewer.locator('.mermaid-modal').count(), 0);
    assert.equal(await viewer.locator('.mermaid-modal button.btn-primary').count(), 0);
    assert.equal(await source(viewer), original);
    console.log('PASS view-only diagram cannot open or save');

    await a.locator('.editor-content > .mermaid-block').click();
    await a.locator('.mermaid-source').fill(draft);
    await b.locator('.editor-content > .mermaid-block').click();
    await b.locator('.mermaid-source').fill(remote);
    await save(b);
    await waitSource(a, remote);
    await save(a);
    await a.locator('.mermaid-save-conflict').waitFor();
    assert.equal(await source(a), remote);
    assert.equal(await a.locator('.mermaid-source').inputValue(), draft);
    for (const width of [320, 375]) {
      await a.setViewportSize({ width, height: 800 });
      await checkLayout(a, `conflict-${width}`);
    }
    await cancel(a);
    await a.locator('.mermaid-modal').waitFor({ state: 'detached' });
    assert.equal(await source(a), remote);
    console.log('PASS received collaborator edit and local draft preserved');

    for (const width of [320, 375]) {
      await a.setViewportSize({ width, height: 800 });
      await a.locator('.editor-content > .mermaid-block').click();
      await a.locator('.mermaid-source').waitFor();
      await checkLayout(a, `normal-${width}`);
      await a.locator('.mermaid-source').fill('');
      await a.locator('.mermaid-modal [role=alert]').waitFor();
      assert(await a.locator('.mermaid-modal button.btn-primary').isDisabled());
      await checkLayout(a, `validation-${width}`);
      await a.locator('.mermaid-source').fill('pie\n' + 'x'.repeat(200));
      await a.waitForFunction(() => document.querySelector('.mermaid-modal .mermaid-error')?.textContent.includes('x'.repeat(200)));
      await checkLayout(a, `parser-${width}`);
      await cancel(a);
      await a.locator('.mermaid-modal').waitFor({ state: 'detached' });
    }
    await a.locator('.editor-content > .mermaid-block').click();
    await a.locator('.mermaid-source').fill(draft);
    await save(a);
    await a.locator('.mermaid-modal').waitFor({ state: 'detached' });
    await waitSource(b, draft);
    await a.locator('[contenteditable=true]').press('Control+z');
    await waitSource(a, remote);
    await waitSource(b, remote);
    console.log('PASS successful Save and Undo both reach collaborator');

    await api(owner, 'DELETE', `/documents/${documentId}`, undefined, ownerAuth.accessToken);
    trashed = true;
    await a.reload();
    await a.locator('.trash-banner').waitFor();
    await a.locator('.editor-content > .mermaid-block').click();
    assert.equal(await a.locator('.mermaid-modal').count(), 0);
    assert.equal(await a.locator('[contenteditable=true]').count(), 0);
    assert.equal(await a.locator('.mermaid-modal button.btn-primary').count(), 0);
    assert.equal(await source(a), remote);
    assert.deepEqual(errors, []);
    console.log('PASS trashed diagram cannot open or save; no browser errors');
  } finally {
    try {
      if (documentId) {
        if (!trashed) await api(owner, 'DELETE', `/documents/${documentId}`, undefined, ownerAuth.accessToken);
        await api(owner, 'DELETE', `/documents/${documentId}/purge`, undefined, ownerAuth.accessToken);
      }
    } finally {
      for (const context of contexts) await context.close();
      await browser.close();
    }
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
