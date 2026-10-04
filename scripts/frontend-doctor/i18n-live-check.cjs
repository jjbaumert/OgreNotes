// Real backend acceptance. No request/WebSocket mocks, route interception, or CRDT seeding.
// Prerequisites: updated frontend served by DEV_MODE=true API at 127.0.0.1:3000,
// initialized DynamoDB/Redis/S3, and Chromium installed for Playwright.
// From scripts/frontend-doctor (after npm ci and Playwright browser installation):
// python3 ../with-test-cleanup.py -- node i18n-live-check.cjs http://127.0.0.1:3000
// Creates two unique dev users and one owned document, then trashes/purges that document.
// Dev users remain; no administrative user-deletion API is assumed.
const assert = require('node:assert/strict');
const { randomUUID } = require('node:crypto');
const { chromium } = require('playwright');
const target = process.argv[2];
assert(target, 'Pass an isolated local dev server URL');
const targetUrl = new URL(target);
assert(['localhost', '127.0.0.1'].includes(targetUrl.hostname)
  && ['http:', 'https:'].includes(targetUrl.protocol)
  && !targetUrl.username && !targetUrl.password
  && targetUrl.pathname === '/' && !targetUrl.search && !targetUrl.hash,
  'Pass an isolated localhost/127.0.0.1 HTTP(S) origin');
const base = targetUrl.origin;
const editor = '.editor-content[data-editor-ready="true"][contenteditable="true"]';
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, label) {
  const deadline = Date.now() + 30000;
  let last;
  do {
    last = await check();
    if (last) return;
    await pause(100);
  } while (Date.now() < deadline);
  throw new Error(`Timed out: ${label}; last result ${JSON.stringify(last)}`);
}
async function api(context, token, method, route, data) {
  const response = await context.request.fetch(`${base}/api/v1${route}`, {
    method, data, headers: token ? { Authorization: `Bearer ${token}` } : {}, timeout: 15000,
  });
  assert(response.ok(), `${method} ${route}: ${response.status()} ${await response.text()}`);
  return response;
}
async function login(context, email) {
  return (await api(context, null, 'POST', '/auth/dev-login', { email, name: 'I18n live acceptance' })).json();
}
async function selection(page, start, end = start) {
  await page.locator(editor).evaluate((el, { start, end }) => {
    el.focus();
    const block = el.querySelector(':scope > p');
    if (!block) throw new Error('Expected first paragraph');
    const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT);
    const nodes = []; while (walker.nextNode()) nodes.push(walker.currentNode);
    function boundary(offset) {
      for (const node of nodes) {
        if (offset <= node.length) return [node, offset];
        offset -= node.length;
      }
      if (!nodes.length && offset === 0) return [block, 0];
      throw new Error('Selection exceeds UTF-16 paragraph text');
    }
    getSelection().setBaseAndExtent(...boundary(start), ...boundary(end));
    document.dispatchEvent(new Event('selectionchange'));
  }, { start, end });
}
async function snapshot(page) {
  return page.locator(editor).evaluate(el => ({
    text: el.textContent,
    children: [...el.children].map(child => child.tagName),
    blocks: [...el.querySelectorAll(':scope > p')].map(p => ({
      text: p.textContent, id: p.dataset.blockId, dir: p.getAttribute('dir'),
      alignment: getComputedStyle(p).textAlign,
    })),
  }));
}
async function expectDoc(pages, expected, id, direction, alignment) {
  if (id !== undefined) assert(typeof id === 'string' && id.length > 0, 'Expected block identity must be nonempty');
  let observed;
  await until(async () => {
    observed = await Promise.all(pages.map(snapshot));
    return observed.every(state => state.children.length === 1 && state.children[0] === 'P'
      && state.text === expected && state.blocks.length === 1 && state.blocks[0].text === expected
      && (id === undefined || state.blocks[0].id === id)
      && (direction === undefined || state.blocks[0].dir === direction)
      && (alignment === undefined || state.blocks[0].alignment === alignment));
  }, `editors preserve exact structure/text/identity/direction/alignment: ${JSON.stringify({ expected, id, direction, alignment })}`)
    .catch(error => { throw new Error(`${error.message}; observed ${JSON.stringify(observed)}`, { cause: error }); });
}
function observeSocket(page) {
  const observation = { syncs: 0, updates: 0 };
  page.on('websocket', ws => {
    if (!new URL(ws.url()).pathname.endsWith('/ws')) return;
    ws.on('framereceived', ({ payload }) => {
      if (!Buffer.isBuffer(payload)) return;
      if (payload[0] === 2) observation.syncs++;
      if (payload[0] === 3) observation.updates++;
    });
  });
  return observation;
}
(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_BIN || undefined });
  const contexts = [], pages = [], errors = [];
  let ownerContext, ownerToken, documentId, failure;
  async function context() {
    const result = await browser.newContext({ locale: 'en-US', serviceWorkers: 'block', permissions: ['clipboard-read', 'clipboard-write'] });
    result.setDefaultTimeout(20000);
    contexts.push(result);
    return result;
  }
  async function page(context, name) {
    const result = await context.newPage();
    pages.push(result);
    result.on('pageerror', error => errors.push(`${name}: ${error.stack || error.message}`));
    return result;
  }
  try {
    const nonce = randomUUID();
    const ownerEmail = `i18n-live-owner-${nonce}@example.test`;
    ownerContext = await context();
    ownerToken = (await login(ownerContext, ownerEmail)).accessToken;
    const settings = await page(ownerContext, 'settings');
    await settings.goto(`${base}/settings?locale=en-US&keep=acceptance#appearance`);
    await settings.locator('#locale-select').waitFor();
    assert.equal(await settings.locator('#locale-select').inputValue(), 'en-US');
    const save = settings.waitForResponse(response =>
      new URL(response.url()).pathname === '/api/v1/users/me/prefs' && response.request().method() === 'PUT');
    const load = settings.waitForEvent('load');
    await settings.locator('#locale-select').selectOption('ar');
    assert((await save).ok(), 'actual locale preference PUT succeeds');
    await load;
    await settings.locator('#locale-select').waitFor();
    assert.equal(await settings.locator('#locale-select').inputValue(), 'ar');
    assert.deepEqual(await settings.evaluate(() => ({ lang: document.documentElement.lang, dir: document.documentElement.dir })),
      { lang: 'ar', dir: 'rtl' });
    const finalUrl = new URL(settings.url());
    assert.equal(finalUrl.searchParams.get('locale'), 'ar');
    assert.equal(finalUrl.searchParams.get('keep'), 'acceptance');
    assert.equal(finalUrl.hash, '#appearance');
    const profile = await (await api(ownerContext, ownerToken, 'GET', '/users/me')).json();
    assert.equal(profile.uiPrefs.locale, 'ar', 'locale stored by actual backend');
    await settings.reload();
    await settings.locator('#locale-select').waitFor();
    assert.equal(await settings.locator('#locale-select').inputValue(), 'ar');
    console.log('PASS actual locale selector save and reload');

    // Same account, fresh cookie jar/localStorage, no URL override.
    const fresh = await context();
    const freshAuth = await login(fresh, ownerEmail);
    const a = await page(fresh, 'owner editor');
    await a.goto(`${base}/settings#appearance`);
    await a.locator('#locale-select').waitFor();
    assert.equal(await a.evaluate(() => localStorage.getItem('ogrenotes.locale')), null,
      'fresh context has no cached locale to supply Arabic');
    assert.equal(await a.locator('#locale-select').inputValue(), 'ar');
    assert.equal(await a.locator('html').getAttribute('lang'), 'ar');
    assert.equal(await a.locator('html').getAttribute('dir'), 'rtl');
    console.log('PASS fresh context derives Arabic from real server preference');
    await settings.close();

    const created = await (await api(ownerContext, ownerToken, 'POST', '/documents', { title: 'I18n live acceptance' })).json();
    documentId = created.id;
    const peer = await context();
    const peerAuth = await login(peer, `i18n-live-peer-${nonce}@example.test`);
    await api(ownerContext, ownerToken, 'POST', `/documents/${documentId}/members`, {
      userId: peerAuth.userId, accessLevel: 'EDIT',
    });
    const b = await page(peer, 'peer editor');
    const aSocket = observeSocket(a), bSocket = observeSocket(b);
    const open = async p => {
      const response = await p.goto(`${base}/d/${documentId}`);
      assert(response.ok(), `document navigation: ${response.status()}`);
      await p.locator(editor).waitFor();
    };
    await open(a);
    await until(() => aSocket.syncs > 0, 'owner real WS sync');
    await expectDoc([a], '');
    const empty = (await snapshot(a)).blocks[0];
    const emptyId = empty.id;
    assert(emptyId, 'empty document paragraph has a stable block identity');
    const emptyAlignment = empty.alignment;
    await selection(a, 0);
    await a.evaluate(async () => navigator.clipboard.write([new ClipboardItem({
      'text/plain': new Blob(['مرحبا😀'], { type: 'text/plain' }),
      'text/html': new Blob(['<p dir="rtl" style="text-align: left"><strong>مرحبا😀</strong></p>'], { type: 'text/html' }),
    })]));
    await a.keyboard.press('Control+v');
    await expectDoc([a], 'مرحبا😀', emptyId, 'rtl', 'left');
    await a.keyboard.press('Control+z');
    await expectDoc([a], '', emptyId, 'auto', emptyAlignment);
    await a.keyboard.press('Control+Shift+z');
    await expectDoc([a], 'مرحبا😀', emptyId, 'rtl', 'left');
    await a.keyboard.press('Control+z');
    await expectDoc([a], '', emptyId, 'auto', emptyAlignment);
    console.log('PASS actual directional clipboard paste, undo and redo preserve block identity');
    const initial = 'مرحبا 😀Hello עולם 123';
    await selection(a, 0);
    await a.keyboard.insertText(initial);
    await expectDoc([a], initial);
    const blockId = (await snapshot(a)).blocks[0].id;
    assert(blockId, 'new block receives a stable identity');
    await selection(a, 0);
    await a.locator('.menu-bar-item').filter({ hasText: /^تنسيق$/ }).click();
    await a.getByRole('menuitem', { name: /^اتجاه النص/ }).click();
    await a.getByRole('menuitem', { name: 'من اليمين إلى اليسار', exact: true }).click();
    await expectDoc([a], initial, blockId, 'rtl');
    await open(b);
    await until(() => bSocket.syncs > 0, 'peer real WS sync');
    await expectDoc([a, b], initial, blockId, 'rtl');
    assert.equal(await b.locator('html').getAttribute('dir'), 'ltr',
      'explicit RTL block persists for an English-locale peer');

    const phrase = '😀Hello עולם';
    const start = initial.indexOf(phrase), end = start + phrase.length;
    await selection(a, end, start);
    assert.equal(await a.evaluate(() => getSelection().toString()), phrase);
    await a.keyboard.press('Control+b');
    await until(async () => (await Promise.all([a, b].map(p =>
      p.locator(`${editor} strong`).allTextContents()))).every(parts => parts.join('') === phrase),
      'backward mixed RTL/non-BMP selection formats and syncs');
    const suffix = ' peer🛰️';
    await selection(b, initial.length);
    await b.keyboard.insertText(suffix);
    await expectDoc([a, b], initial + suffix, blockId, 'rtl');
    const replacement = '🧭Latin שלום';
    await selection(a, start, end);
    await a.keyboard.insertText(replacement);
    const expected = initial.slice(0, start) + replacement + initial.slice(end) + suffix;
    await expectDoc([a, b], expected, blockId, 'rtl');
    assert(aSocket.updates > 0 && bSocket.updates > 0, 'both real sockets received peer updates');
    console.log('PASS mixed Arabic/Hebrew/Latin/emoji editing, RTL Format command, peer sync and block identity');

    await until(async () => {
      const html = await (await api(fresh, freshAuth.accessToken, 'GET', `/documents/${documentId}/export/html`)).text();
      return a.evaluate(({ html, expected }) => [...new DOMParser().parseFromString(html, 'text/html').querySelectorAll('p')]
        .some(p => p.textContent === expected), { html, expected });
    }, 'actual HTML export has exact persisted Unicode text');
    // HTML currently omits ordinary block IDs and direction attributes. Their
    // persistence is asserted against real editor reloads, not claimed for export.
    for (const p of [a, b]) {
      const response = await p.reload();
      assert(response.ok(), `saved document reload: ${response.status()}`);
      await p.locator(editor).waitFor();
      await expectDoc([p], expected, blockId, 'rtl');
    }
    assert.deepEqual(errors, [], 'no browser exceptions');
    console.log('PASS real HTML export text and owner/peer reloads preserve text, block identity and RTL');
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    // Attempt every cleanup even after an assertion or an earlier cleanup fails.
    // Keep the original test failure as the primary error if both fail.
    const cleanupErrors = [];
    const clean = async (label, action) => {
      try { await action(); }
      catch (error) { cleanupErrors.push(new Error(`${label}: ${error.message}`, { cause: error })); }
    };
    for (const p of pages) {
      if (!p.isClosed()) await clean('close page', () => p.close());
    }
    if (documentId) {
      await clean(`trash own document ${documentId}`, () => api(ownerContext, ownerToken, 'DELETE', `/documents/${documentId}`));
      await clean(`purge own document ${documentId}`, () => api(ownerContext, ownerToken, 'DELETE', `/documents/${documentId}/purge`));
      if (!cleanupErrors.length) console.log('CLEANED own probe document');
    }
    for (const c of contexts) await clean('close context', () => c.close());
    await clean('close browser', () => browser.close());
    if (cleanupErrors.length) {
      if (failure) console.error('Additional cleanup failures:', ...cleanupErrors);
      else throw new AggregateError(cleanupErrors, 'Live i18n probe cleanup failed');
    }
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
