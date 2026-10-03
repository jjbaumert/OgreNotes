// Actual editors, distinct principals, real WebSockets and persisted reloads.
// Run against an isolated dev stack: node collab-preservation.cjs http://127.0.0.1:3000
const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const base = process.argv[2];
assert(base && ['localhost', '127.0.0.1'].includes(new URL(base).hostname), 'Pass an isolated local dev server URL');
const selector = '.editor-content[data-editor-ready][contenteditable="true"]';
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, message) {
  const end = Date.now() + 15000;
  let last;
  do { try { last = await check(); if (last) return; } catch (e) { last = e.message; } await delay(50); } while (Date.now() < end);
  throw new Error(`${message}: ${last}`);
}
async function api(context, method, path, data, token) {
  const response = await context.request.fetch(`${base}/api/v1/${path}`, {
    method, data, headers: token ? { authorization: `Bearer ${token}` } : {},
  });
  assert(response.ok(), `${method} ${path}: ${response.status()} ${await response.text()}`);
  return response;
}
async function select(page, paragraph, start, end = start) {
  await page.locator(selector).evaluate((editor, args) => {
    editor.focus();
    const p = editor.querySelectorAll('p')[args.paragraph];
    const walker = document.createTreeWalker(p, NodeFilter.SHOW_TEXT);
    const nodes = []; while (walker.nextNode()) nodes.push(walker.currentNode);
    function boundary(offset) {
      for (const node of nodes) { if (offset <= node.length) return [node, offset]; offset -= node.length; }
      if (!nodes.length && offset === 0) return [p, 0];
      throw new Error('Selection beyond paragraph');
    }
    const [a, x] = boundary(args.start), [b, y] = boundary(args.end);
    getSelection().setBaseAndExtent(a, x, b, y);
    document.dispatchEvent(new Event('selectionchange'));
  }, { paragraph, start, end });
}
async function text(page) { return page.locator(selector).evaluate(el => [...el.querySelectorAll(':scope > p')].map(p => p.textContent)); }
async function exact(pages, expected) {
  let observed;
  try {
    await until(async () => {
      observed = await Promise.all(pages.map(text));
      return observed.every(actual => JSON.stringify(actual) === JSON.stringify(expected));
    }, `Both editors must contain ${JSON.stringify(expected)}`);
  } catch (error) { throw new Error(`${error.message}; observed ${JSON.stringify(observed)}`); }
  for (const page of pages) {
    const ids = await page.locator(selector).evaluate(el => [...el.querySelectorAll('[data-block-id]')].map(p => p.dataset.blockId));
    assert.equal(new Set(ids).size, ids.length, 'Block identities must be unique');
  }
}
// Hold the 16 ms Yrs receive debounce while leaving the browser's other
// timers on their real clock. Playwright's global clock fast-forward can call
// a wasm-bindgen one-shot closure twice, which tests the clock shim rather
// than this editor's post-apply/pre-render window.
async function holdReceiveDebounce(page) {
  await page.evaluate(() => {
    const set = window.setTimeout.bind(window);
    const clear = window.clearTimeout.bind(window);
    const pending = new Map();
    window.__receiveGate = {
      pending: () => pending.size,
      release: () => {
        window.setTimeout = set;
        window.clearTimeout = clear;
        for (const [id, { callback, args }] of pending) {
          clear(id);
          set(callback, 0, ...args);
        }
        pending.clear();
      },
    };
    window.setTimeout = (callback, ms, ...args) => {
      if (ms !== 16) return set(callback, ms, ...args);
      const id = set(() => {}, 2147483647);
      pending.set(id, { callback, args });
      return id;
    };
    window.clearTimeout = id => { pending.delete(id); clear(id); };
  });
}
async function socketGate(page) {
  const gate = { held: false, pending: [], updates: 0, synced: false };
  await page.routeWebSocket('**/api/v1/documents/*/ws', ws => {
    const server = ws.connectToServer();
    server.onMessage(message => {
      if (Buffer.isBuffer(message) && message[0] === 2) gate.synced = true;
      if (Buffer.isBuffer(message) && message[0] === 3) {
        gate.updates++;
        if (gate.held) { gate.pending.push(() => ws.send(message)); return; }
      }
      ws.send(message);
    });
  });
  gate.release = () => { gate.held = false; for (const send of gate.pending.splice(0)) send(); };
  return gate;
}
(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_BIN || undefined });
  const errors = [];
  try {
    for (const scenario of ['disjoint-text', 'formatting', 'inline-formatting', 'prepend', 'structural-window', 'unicode-history']) {
      if (process.env.COLLAB_CASE && process.env.COLLAB_CASE !== scenario) continue;
      const contexts = await Promise.all([0, 1].map(() => browser.newContext({ bypassCSP: true, serviceWorkers: 'block' })));
      const tokens = await Promise.all(contexts.map(async (c, i) => (await api(c, 'POST', 'auth/dev-login', { email: `preserve-${scenario}-${Date.now()}-${i}@example.test` })).json()));
      const doc = await (await api(contexts[0], 'POST', 'documents', { title: `Preservation ${scenario}` }, tokens[0].accessToken)).json();
      await api(contexts[0], 'POST', `documents/${doc.id}/members`, { userId: tokens[1].userId, accessLevel: 'EDIT' }, tokens[0].accessToken);
      const pages = await Promise.all(contexts.map(c => c.newPage()));
      pages.forEach((p, i) => p.on('pageerror', e => errors.push(`${scenario} tab ${i}: ${e.stack || e.message}`)));
      const gates = await Promise.all(pages.map(socketGate));
      const [a, b] = pages;
      const open = async p => { await p.goto(`${base}/d/${doc.id}`); await p.locator(selector).waitFor(); };
      await open(a);
      await until(() => gates[0].synced, 'A must complete WebSocket sync');
      let initial = scenario === 'disjoint-text' ? 'abcdef' : scenario === 'unicode-history' ? 'A😀B' : 'Hello';
      await select(a, 0, 0);
      await a.keyboard.insertText(initial);
      if (scenario === 'inline-formatting') {
        await a.keyboard.press('Shift+Enter');
        assert.equal(await a.locator(`${selector} p br:not([data-sentinel])`).count(), 1,
          `Shift+Enter must insert a hard break: ${await a.locator(selector).innerHTML()}`);
        await a.keyboard.insertText('world');
        initial = 'Helloworld';
      }
      await open(b);
      await until(() => gates[1].synced, 'B must complete WebSocket sync');
      await exact(pages, [initial]);
      if (scenario === 'inline-formatting') {
        for (const page of pages) assert.equal(await page.locator(`${selector} p br:not([data-sentinel])`).count(), 1,
          `Initial hard break must exist: ${await page.locator(selector).innerHTML()}`);
      }
      let expected;
      if (scenario === 'disjoint-text') {
        gates.forEach(g => { g.held = true; });
        await select(b, 0, 1, 2); await b.keyboard.insertText('B');
        await select(b, 0, 5, 6); await b.keyboard.insertText('F');
        await select(a, 0, 3); await a.keyboard.insertText('X');
        await until(() => gates.every(g => g.pending.length), 'Concurrent edits must cross the real server');
        gates.forEach(g => g.release());
        expected = ['aBcXdeF'];
      } else if (scenario.endsWith('formatting')) {
        gates.forEach(g => { g.held = true; });
        await select(a, 0, 0, initial.length); await a.keyboard.press('Control+b');
        await select(b, 0, 0, initial.length); await b.keyboard.press('Control+i');
        await until(() => gates.every(g => g.pending.length), 'Both formatting changes must reach peers');
        gates.forEach(g => g.release());
        expected = [initial];
        await until(async () => (await Promise.all(pages.map(p => p.locator(`${selector} strong em, ${selector} em strong`).count()))).every(Boolean), 'Both marks must survive');
      } else if (scenario === 'structural-window') {
        // Hold remote frames while B appends a block, then pause A's timers.
        // The local keystroke lands after remote Yrs apply but before the
        // debounced UI swap and local sync callbacks can run.
        gates[0].held = true;
        await select(b, 0, 5);
        await b.keyboard.press('Enter');
        await b.keyboard.insertText('Peer');
        await exact([b], ['Hello', 'Peer']);
        await until(() => gates[0].pending.length > 0, 'Remote structural update must be queued');
        await holdReceiveDebounce(a);
        gates[0].release();
        await until(() => a.evaluate(() => window.__receiveGate.pending() > 0),
          'Remote apply must schedule its debounced UI update');
        await select(a, 0, 5);
        await a.keyboard.insertText('!');
        await exact([a], ['Hello!']);
        await a.evaluate(() => window.__receiveGate.release());
        expected = ['Hello!', 'Peer'];
      } else if (scenario === 'prepend') {
        // Exercise the actual prepend UI. Deterministic stale-baseline timing
        // is covered separately by collab_semantics using the same bridge.
        await select(b, 0, 0);
        await b.keyboard.press('Enter');
        await select(b, 0, 0);
        await b.keyboard.insertText('Peer');
        await exact(pages, ['Peer', 'Hello']);
        await select(a, 1, 5);
        await a.keyboard.insertText('!');
        expected = ['Peer', 'Hello!'];
      } else {
        await select(a, 0, 3); await a.keyboard.insertText('X');
        await exact(pages, ['A😀XB']);
        // Start another history group so the test distinguishes one undo from two.
        await delay(550);
        await select(a, 0, 5); await a.keyboard.insertText('Y');
        await exact(pages, ['A😀XBY']);
        await a.locator(selector).evaluate(el => el.dispatchEvent(new InputEvent('beforeinput', { bubbles: true, cancelable: true, inputType: 'historyUndo' })));
        await exact(pages, ['A😀XB']);
        await a.locator(selector).evaluate(el => el.dispatchEvent(new InputEvent('beforeinput', { bubbles: true, cancelable: true, inputType: 'historyRedo' })));
        await exact(pages, ['A😀XBY']);
        await a.keyboard.press('Control+z');
        await exact(pages, ['A😀XB']);
        await a.keyboard.press('Control+Shift+z');
        expected = ['A😀XBY'];
      }
      await exact(pages, expected);
      // Export reads the persisted snapshot plus pending durable updates,
      // independently of either live WebSocket room. A reload alone could
      // reconnect to that room and conceal a stale stored snapshot.
      await until(async () => {
        const response = await api(contexts[0], 'GET', `documents/${doc.id}/export/html`, undefined, tokens[0].accessToken);
        const html = await response.text();
        const stored = await a.evaluate(value => {
          const body = new DOMParser().parseFromString(value, 'text/html').body;
          return {
            paragraphs: [...body.querySelectorAll(':scope > p')].map(p => p.textContent),
            jointlyMarked: [...body.querySelectorAll('strong em, em strong')].map(n => n.textContent).join(''),
            hardBreaks: body.querySelectorAll('p br').length,
          };
        }, html);
        return JSON.stringify(stored.paragraphs) === JSON.stringify(expected)
          && (!scenario.endsWith('formatting') || stored.jointlyMarked === initial)
          && (scenario !== 'inline-formatting' || stored.hardBreaks === 1);
      }, `Stored export must contain ${JSON.stringify(expected)} and its formatting`);
      // Fresh editor instances must fetch and render the saved document again.
      await Promise.all(pages.map(p => p.reload()));
      await exact(pages, expected);
      if (scenario.endsWith('formatting')) {
        for (const p of pages) {
          assert.equal((await p.locator(`${selector} strong em, ${selector} em strong`).allTextContents()).join(''), initial);
          if (scenario === 'inline-formatting') assert.equal(await p.locator(`${selector} p br:not([data-sentinel])`).count(), 1);
        }
      }
      console.log(`PASS ${scenario}: two principals, exact content, unique IDs, persisted reload`);
      await Promise.all(contexts.map(c => c.close()));
    }
    assert.deepEqual(errors, [], 'No browser page errors');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
