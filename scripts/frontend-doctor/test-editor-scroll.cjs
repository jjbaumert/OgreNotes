// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
// Reproduce viewport drift using a private copy of a document on a DEV_MODE stack.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const https = require('node:https');
const os = require('node:os');
const { spawn, spawnSync } = require('node:child_process');
const { chromium } = require('playwright');
const args = process.argv.slice(2);
const option = (name, fallback) => args.includes(name) ? args[args.indexOf(name) + 1] : fallback;
const base = option('--base-url', 'http://localhost:3000');
const engine = option('--browser', 'chromium');
const viewportWidth = Number(option('--viewport-width', '1360'));
const snapshot = option('--snapshot');
const generatedOnly = args.includes('--generated-only');
const expectCaretVisible = args.includes('--expect-caret-visible');
const baselinePath = option('--baseline-report');
const baseline = baselinePath ? JSON.parse(fs.readFileSync(baselinePath, 'utf8')) : null;
if (baseline) assert.equal(baseline.harnessVersion, 2, 'Recapture the baseline with this harness version');
const out = option('--out', '/tmp/ogrenotes-editor-scroll');
assert(snapshot || generatedOnly, '--snapshot must name a document content snapshot; it stays local');
assert(['chromium', 'firefox'].includes(engine), '--browser must be chromium or firefox');
fs.mkdirSync(out, { recursive: true });
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));

async function firefoxPage(context) {
  const port = await new Promise(resolve => {
    const server = net.createServer();
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port;
      server.close(() => resolve(port));
    });
  });
  const options = ['--port', String(port), '--host', '127.0.0.1', '--log', 'error'];
  if (process.env.GECKODRIVER_PROFILE_ROOT) options.push('--profile-root', process.env.GECKODRIVER_PROFILE_ROOT);
  const driver = spawn(process.env.GECKODRIVER || 'geckodriver', options, { stdio: 'ignore' });
  let launchError;
  driver.on('error', error => { launchError = error; });
  let session;
  async function command(method, route, data) {
    const response = await fetch(`http://127.0.0.1:${port}${route}`, {
      method, headers: { 'Content-Type': 'application/json' },
      body: data === undefined ? undefined : JSON.stringify(data),
    });
    const json = await response.json();
    assert(response.ok, JSON.stringify(json.value));
    return json.value;
  }
  const close = async () => {
    try { if (session) await command('DELETE', `/session/${session}`); }
    finally { driver.kill(); }
  };
  try {
    let ready = false;
    for (let attempt = 0; attempt < 100; attempt++) {
      if (launchError) throw launchError;
      assert(driver.exitCode === null, 'geckodriver exited during startup');
      try { ready = (await command('GET', '/status')).ready; } catch {}
      if (ready) break;
      await pause(100);
    }
    assert(ready, 'geckodriver did not become ready');
    const firefoxOptions = { args: ['-headless'] };
    if (process.env.FIREFOX_BIN) firefoxOptions.binary = process.env.FIREFOX_BIN;
    session = (await command('POST', '/session', {
      capabilities: { alwaysMatch: { browserName: 'firefox', acceptInsecureCerts: true, 'moz:firefoxOptions': firefoxOptions } },
    })).sessionId;
    await command('POST', `/session/${session}/window/rect`, { width: viewportWidth, height: 900 });
    const goto = url => command('POST', `/session/${session}/url`, { url });
    await goto(base + '/login');
    for (const { name, value, path, httpOnly, secure, sameSite } of await context.cookies()) {
      await command('POST', `/session/${session}/cookie`, { cookie: { name, value, path, httpOnly, secure, sameSite } });
    }
    return {
      goto, close,
      evaluate: (fn, arg) => command('POST', `/session/${session}/execute/sync`, {
        script: `return (${fn.toString()})(arguments[0]);`, args: [arg ?? null],
      }),
      type: character => command('POST', `/session/${session}/actions`, {
        actions: [{ type: 'key', id: 'typing', actions: [
          { type: 'keyDown', value: character }, { type: 'keyUp', value: character },
        ] }],
      }),
      press: key => {
        const keys = key.split('+').map(value => ({ Enter: '\uE007', Control: '\uE009' }[value] || value));
        return command('POST', `/session/${session}/actions`, { actions: [{ type: 'key', id: 'typing',
          actions: [...keys.map(value => ({ type: 'keyDown', value })), ...keys.reverse().map(value => ({ type: 'keyUp', value }))],
        }] });
      },
    };
  } catch (error) { await close(); throw error; }
}

async function waitFor(page, fn, arg) {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await page.evaluate(fn, arg)) return;
    await pause(100);
  }
  throw new Error('Editor did not become ready');
}
const metrics = page => page.evaluate(() => {
  const container = document.querySelector('.editor-container');
  const selection = getSelection();
  let rect = selection.rangeCount ? selection.getRangeAt(0).getBoundingClientRect() : null;
  if (rect?.height < 1 && selection.focusNode?.nodeType === Node.ELEMENT_NODE) {
    rect = selection.focusNode.getBoundingClientRect();
  }
  const bounds = container.getBoundingClientRect();
  let viewportTop = Math.max(bounds.top, visualViewport?.offsetTop || 0);
  let viewportBottom = Math.min(bounds.bottom, (visualViewport?.offsetTop || 0) + (visualViewport?.height || innerHeight));
  const toolbar = document.querySelector('.toolbar');
  if (toolbar && getComputedStyle(toolbar).position === 'fixed') viewportBottom = Math.min(viewportBottom, toolbar.getBoundingClientRect().top);
  return { top: container.scrollTop, height: container.scrollHeight,
    caretTop: rect?.top, caretBottom: rect?.bottom, caretLeft: rect?.left,
    viewportTop, viewportBottom,
    characters: document.querySelector('.editor-content').textContent.length };
});

(async () => {
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROME_BIN ? { executablePath: process.env.CHROME_BIN } : {}) });
  let context;
  let token, page, failure, imageServer, imageCertificateDir;
  const ids = [];
  const reports = [];
  async function api(method, route, data, binary = false) {
    const response = await context.request.fetch(base + '/api/v1' + route, {
      method, data, headers: { ...(token ? { Authorization: 'Bearer ' + token } : {}),
        ...(binary ? { 'Content-Type': 'application/octet-stream' } : {}) },
    });
    assert(response.ok(), `${method} ${route}: ${response.status()}`);
    return response.headers()['content-type']?.includes('application/json') ? response.json() : null;
  }
  try {
    context = await browser.newContext({ viewport: { width: viewportWidth, height: 800 }, ignoreHTTPSErrors: true });
    token = (await api('POST', '/auth/dev-login', {
      email: `scroll-${Date.now()}@ogrenotes.example.com`, name: 'Editor scroll regression',
    })).accessToken;
    const id = (await api('POST', '/documents', { title: 'Editor viewport regression' })).id;
    ids.push(id);
    if (!generatedOnly) await api('PUT', `/documents/${id}/content`, fs.readFileSync(snapshot), true);
    if (engine === 'firefox') page = await firefoxPage(context);
    else {
      const chromiumPage = await context.newPage();
      chromiumPage.on('pageerror', error => console.error('Browser error:', error.message));
      page = { goto: url => chromiumPage.goto(url), evaluate: (fn, arg) => chromiumPage.evaluate(fn, arg),
        type: character => chromiumPage.keyboard.type(character), press: key => chromiumPage.keyboard.press(key), close: () => chromiumPage.close() };
    }
    await page.goto(`${base}/d/${id}/regression`);
    await waitFor(page, () => !!document.querySelector('.editor-content[data-editor-ready=true]'));
    const cases = generatedOnly ? [] : [0.05, 0.5, 0.9, 'block-start', 'block-middle', 'long', 'code', 'bottom', 'wrap'];
    for (const scenario of cases) {
      const location = await page.evaluate(scenario => {
        const editor = document.querySelector('.editor-content');
        const container = document.querySelector('.editor-container');
        const paragraphs = [...editor.querySelectorAll(':scope > p')].filter(p => p.textContent.length > 40);
        const origin = editor.getBoundingClientRect().top;
        const desired = origin + editor.scrollHeight * (typeof scenario === 'number' ? scenario : 0.5);
        const target = scenario === 'code' ? [...editor.querySelectorAll(':scope > pre')].find(pre => !pre.classList.contains('mermaid-code-block')) :
          scenario === 'long' || scenario === 'wrap' ? paragraphs.reduce((a, b) =>
            a.textContent.length > b.textContent.length ? a : b) :
          paragraphs.reduce((a, b) => Math.abs(a.getBoundingClientRect().top - desired) <
            Math.abs(b.getBoundingClientRect().top - desired) ? a : b);
        if (!target) return null;
        editor.focus({ preventScroll: true });
        const walker = document.createTreeWalker(target, NodeFilter.SHOW_TEXT);
        const nodes = [];
        let node;
        while ((node = walker.nextNode())) {
          if (node.textContent.length && !node.parentElement.closest('[contenteditable=false]')) nodes.push(node);
        }
        if (!nodes.length) throw new Error('Block contains no editable text');
        node = scenario === 'wrap' ? nodes.at(-1) : nodes[0];
        let offset = scenario === 'wrap' ? node.length : scenario === 'block-start' ? 0 : Math.min(10, node.length);
        if (scenario === 'block-middle') {
          let remaining = Math.floor(nodes.reduce((n, text) => n + text.length, 0) / 2);
          for (const text of nodes) {
            node = text; offset = Math.min(remaining, text.length);
            if (remaining <= text.length) break;
            remaining -= text.length;
          }
        }
        // A single tall paragraph still needs distinct locations within its text.
        if (typeof scenario === 'number' && target.getBoundingClientRect().height > container.clientHeight) {
          let remaining = Math.floor(nodes.reduce((n, text) => n + text.length, 0) *
            Math.max(0, Math.min(1, (desired - target.getBoundingClientRect().top) / target.getBoundingClientRect().height)));
          for (const text of nodes) {
            node = text; offset = Math.min(remaining, text.length);
            if (remaining <= text.length) break;
            remaining -= text.length;
          }
        }
        if (offset > 0 && /[\uDC00-\uDFFF]/.test(node.textContent[offset] || '') &&
            /[\uD800-\uDBFF]/.test(node.textContent[offset - 1])) offset--;
        const range = document.createRange();
        range.setStart(node, offset); range.collapse(true);
        const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
        const caret = range.getBoundingClientRect();
        const fraction = (caret.top - origin) / editor.scrollHeight;
        container.scrollTop += caret.top - container.getBoundingClientRect().top -
          (scenario === 'bottom' ? container.clientHeight - 55 : 200);
        return { fraction };
      }, scenario);
      if (!location) { console.log(`SKIP ${engine} ${scenario}: snapshot has no code block`); continue; }
      if (typeof scenario === 'number') assert(Math.abs(location.fraction - scenario) < 0.15,
        `Fixture cannot cover rendered document position ${scenario}: ${location.fraction}`);
      await pause(250);
      const before = await metrics(page);
      const steps = [];
      reports.push({ scenario, location, before, steps });
      const text = scenario === 'wrap' ? ' wrapprobe '.repeat(12) : 'scrollprobe';
      for (const character of text) {
        await page.type(character); await pause(80);
        const after = await metrics(page);
        steps.push(after);
        assert.equal(after.characters, before.characters + steps.length, 'Character was lost or duplicated');
        assert(after.caretTop >= after.viewportTop - 1 && after.caretBottom <= after.viewportBottom + 1,
          `Caret left viewport during ${scenario}`);
        if (scenario !== 'wrap') {
          if (!['long', 'block-start', 'block-middle'].includes(scenario)) assert(Math.abs(after.height - before.height) <= 1, 'Fixture position must allow layout-neutral typing');
        }
        assert(Math.abs(after.top - before.top) <= 1, `Viewport drift at ${scenario}: ${before.top} -> ${after.top}`);
        if (scenario === 'wrap' && after.height > before.height) break;
      }
      if (scenario === 'wrap') assert(steps.at(-1).height > before.height, 'Typing must exercise a wrapping transition');
      console.log(`PASS ${engine} ${scenario}: characters delivered; viewport and caret checked`);
    }

    // Start both collaborators before seeding a generated document. The private
    // imported snapshot is used only by the single-client reproduction above.
    const remoteId = (await api('POST', '/documents', { title: 'Remote viewport regression' })).id;
    ids.push(remoteId);
    const peerContext = await browser.newContext({ viewport: { width: 1360, height: 800 }, ignoreHTTPSErrors: true });
    const login = await peerContext.request.post(base + '/api/v1/auth/dev-login', { data: {
      email: `scroll-peer-${Date.now()}@ogrenotes.example.com`, name: 'Viewport collaborator',
    } });
    assert(login.ok(), 'Peer login failed');
    const peerAuth = await login.json();
    await api('POST', `/documents/${remoteId}/members`, { userId: peerAuth.userId, accessLevel: 'EDIT' });
    const peer = await peerContext.newPage();
    await page.goto(`${base}/d/${remoteId}/remote`);
    await peer.goto(`${base}/d/${remoteId}/remote`);
    await waitFor(page, () => !!document.querySelector('.editor-content[data-editor-ready=true]'));
    await peer.locator('.editor-content[data-editor-ready=true]').waitFor();
    await page.evaluate(() => {
      const editor = document.querySelector('.editor-content'); editor.focus();
      const range = document.createRange(); range.selectNodeContents(editor.querySelector('p')); range.collapse(true);
      getSelection().removeAllRanges(); getSelection().addRange(range);
      const data = new DataTransfer();
      data.setData('text/html', '<h1>Remote viewport fixture</h1>' + Array.from({ length: 180 }, (_, n) =>
        `<p>${n === 0 ? 'SCROLL_GENERATED_0:' : `SCROLL_GENERATED_${n}: keep the editor viewport steady while another person types.`}</p>` +
        (n === 80 ? '<pre><code class="language-rust">let value = 1;</code></pre>' : '')).join(''));
      const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
      Object.defineProperty(event, 'clipboardData', { value: data });
      editor.dispatchEvent(event);
    });
    await waitFor(page, () => document.querySelectorAll('.editor-content > p').length >= 180);
    await peer.waitForFunction(() => document.querySelectorAll('.editor-content > p').length >= 180);
    for (const position of ['start', 'middle']) {
      await page.evaluate(position => {
        const editor = document.querySelector('.editor-content'); editor.focus({ preventScroll: true });
        const code = editor.querySelector('pre:not(.mermaid-code-block) > code');
        if (!code?.firstChild) throw new Error('Generated fixture must have an ordinary code block');
        const walker = document.createTreeWalker(code, NodeFilter.SHOW_TEXT);
        let node, remaining = position === 'start' ? 0 : Math.floor(code.textContent.length / 2);
        while ((node = walker.nextNode())) {
          if (remaining <= node.length) break;
          remaining -= node.length;
        }
        if (!node) throw new Error('Code must have editable text');
        const range = document.createRange();
        range.setStart(node, remaining);
        range.collapse(true); getSelection().removeAllRanges(); getSelection().addRange(range);
        const container = document.querySelector('.editor-container');
        container.scrollTop += range.getBoundingClientRect().top - container.getBoundingClientRect().top - 200;
      }, position);
      await pause(250);
      const before = await metrics(page); const steps = [];
      reports.push({ scenario: `generated-code-${position}`, before, steps });
      assert(before.top > 0, 'Code-block fixture must be scrolled');
      for (const character of 'scrollprobe') {
        await page.type(character); await pause(80);
        const after = await metrics(page); steps.push(after);
        assert.equal(after.characters, before.characters + steps.length);
        if (expectCaretVisible) {
          assert(after.caretTop >= after.viewportTop - 1 && after.caretBottom <= after.viewportBottom + 1,
            `Code typing left the caret outside the viewport (${position})`);
        }
        assert(Math.abs(after.top - before.top) <= 1, 'Ordinary code typing moved viewport');
      }
      console.log(`PASS ${engine} generated code ${position}: typing preserves viewport`);
    }
    // A code pane below the visible band must not stop the parent reveal.
    const codeBand = await metrics(page);
    await page.evaluate(bottom => {
      const container = document.querySelector('.editor-container');
      const range = getSelection().getRangeAt(0);
      container.scrollTop += range.getBoundingClientRect().top - bottom - 8;
    }, codeBand.viewportBottom);
    const codeEdgeBefore = await metrics(page);
    await page.type('x'); await pause(100);
    const codeEdgeAfter = await metrics(page);
    reports.push({ scenario: 'code-edge', before: codeEdgeBefore, steps: [codeEdgeAfter] });
    assert.equal(codeEdgeAfter.characters, codeEdgeBefore.characters + 1);
    if (expectCaretVisible) assert(codeEdgeAfter.caretTop >= codeEdgeAfter.viewportTop - 1 &&
      codeEdgeAfter.caretBottom <= codeEdgeAfter.viewportBottom + 1, 'Code pane below the viewport hid its caret');

    await page.evaluate(() => {
      const editor = document.querySelector('.editor-content'); editor.focus({ preventScroll: true });
      const target = editor.querySelectorAll(':scope > p')[90];
      const range = document.createRange(); range.setStart(target.firstChild, 10); range.collapse(true);
      getSelection().removeAllRanges(); getSelection().addRange(range);
      const container = document.querySelector('.editor-container');
      container.scrollTop += range.getBoundingClientRect().top - container.getBoundingClientRect().top - 200;
    });
    await pause(250);
    const before = await metrics(page);
    assert(before.top > 0, 'Remote regression must be scrolled');
    await peer.evaluate(() => {
      const editor = document.querySelector('.editor-content'); editor.focus();
      const target = [...editor.querySelectorAll(':scope > p')].find(p => p.textContent.includes('SCROLL_GENERATED_0:'));
      const range = document.createRange(); range.setStart(target.firstChild, 10); range.collapse(true);
      getSelection().removeAllRanges(); getSelection().addRange(range);
    });
    await peer.keyboard.type('remoteprobe');
    await waitFor(page, () => document.querySelector('.editor-content').textContent.includes('remoteprobe'));
    await pause(250);
    const after = await metrics(page);
    reports.push({ scenario: 'remote', before, steps: [after] });
    assert.equal(after.characters, before.characters + 'remoteprobe'.length, 'Remote text was lost or duplicated');
    assert(Math.abs(after.top - before.top) <= 1, 'Remote layout-neutral typing moved viewport');
    assert(Math.abs(after.caretTop - before.caretTop) <= 1, 'Remote typing moved local caret');
    console.log(`PASS ${engine} remote: text delivered without moving viewport or local caret`);
    await page.evaluate(() => { document.querySelector('.editor-container').scrollTop = 0; });
    const passive = await metrics(page);
    assert(passive.caretTop > passive.viewportBottom, 'Passive caret must be outside viewport');
    await peer.keyboard.type('passiveprobe');
    await waitFor(page, () => document.querySelector('.editor-content').textContent.includes('passiveprobe'));
    await pause(250);
    const passiveAfter = await metrics(page);
    reports.push({ scenario: 'remote-passive', before: passive, steps: [passiveAfter] });
    assert.equal(passiveAfter.characters, passive.characters + 'passiveprobe'.length);
    assert(Math.abs(passiveAfter.top - passive.top) <= 1, 'Remote edit pulled a passive reader back to their caret');
    console.log(`PASS ${engine} remote: offscreen passive caret does not move viewport`);

    // Record boundary and remote layout changes for comparison with a baseline
    // build that does not suppress anchoring during redraws.
    // --expect-caret-visible asserts caret reveal; otherwise wrapping behavior
    // is recorded for comparison with the baseline renderer.
    for (const anchoring of ['auto', 'none']) {
      const lineHeight = await page.evaluate(anchoring => {
        const editor = document.querySelector('.editor-content');
        const container = document.querySelector('.editor-container');
        container.style.overflowAnchor = anchoring;
        editor.focus({ preventScroll: true });
        const target = editor.querySelectorAll(':scope > p')[anchoring === 'auto' ? 92 : 93];
        const range = document.createRange(); range.setStart(target.lastChild, target.lastChild.length); range.collapse(true);
        getSelection().removeAllRanges(); getSelection().addRange(range);
        container.scrollTop += range.getBoundingClientRect().bottom - container.getBoundingClientRect().bottom + 2;
        return parseFloat(getComputedStyle(target).lineHeight);
      }, anchoring);
      await pause(250);
      const positioned = await metrics(page);
      await page.evaluate(delta => { document.querySelector('.editor-container').scrollTop += delta; },
        positioned.caretBottom - positioned.viewportBottom + 2);
      await pause(100);
      const before = await metrics(page);
      const steps = [];
      reports.push({ scenario: `wrap-edge-${anchoring}`, before, steps });
      for (const character of ' wrapprobe '.repeat(30)) {
        await page.type(character); await pause(30);
        const after = await metrics(page); steps.push(after);
        assert.equal(after.characters, before.characters + steps.length);
        if (expectCaretVisible) {
          assert(after.caretTop >= after.viewportTop - 1 && after.caretBottom <= after.viewportBottom + 1,
            `Local wrapping left the caret outside the viewport (${anchoring})`);
        }
        if (after.height - before.height >= lineHeight * 2 - 1) break;
      }
      assert(steps.at(-1).height - before.height >= lineHeight * 2 - 1, 'Boundary case must wrap across two lines');
      if (expectCaretVisible) {
        assert(steps.at(-1).top > before.top, 'Boundary typing must actually scroll to reveal the caret');
        const paragraphs = await page.evaluate(() => document.querySelectorAll('.editor-content > p').length);
        // Start a new history group so this undo isolates the Enter edit.
        await pause(1000);
        await page.press('Enter'); await pause(100);
        const entered = await metrics(page);
        assert.equal(await page.evaluate(() => document.querySelectorAll('.editor-content > p').length), paragraphs + 1);
        assert(entered.caretTop >= entered.viewportTop - 1 && entered.caretBottom <= entered.viewportBottom + 1, 'Enter hid the new empty paragraph caret');
        await page.press('Control+z'); await pause(100);
        assert.equal(await page.evaluate(() => document.querySelectorAll('.editor-content > p').length), paragraphs);
        const undone = await metrics(page);
        assert.equal(undone.characters, steps.at(-1).characters, 'Undo must restore the text before Enter');
        assert(undone.caretTop >= undone.viewportTop - 1 && undone.caretBottom <= undone.viewportBottom + 1, 'Undo hid the restored caret');
        // Toolbar history commands take a different production dispatch path.
        await page.evaluate(() => document.querySelector('.toolbar button[title="Redo (Ctrl+Shift+Z)"]').click());
        await pause(100);
        assert.equal(await page.evaluate(() => document.querySelectorAll('.editor-content > p').length), paragraphs + 1);
        await page.evaluate(() => document.querySelector('.toolbar button[title="Undo (Ctrl+Z)"]').click());
        await pause(100);
        const toolbarUndo = await metrics(page);
        assert.equal(toolbarUndo.characters, undone.characters);
        assert(toolbarUndo.caretTop >= toolbarUndo.viewportTop - 1 && toolbarUndo.caretBottom <= toolbarUndo.viewportBottom + 1,
          'Toolbar undo hid the restored caret');
      }

      // Select a caret in the middle, then insert and undo real blocks above it.
      await page.evaluate(() => {
        const editor = document.querySelector('.editor-content'); editor.focus({ preventScroll: true });
        const target = editor.querySelectorAll(':scope > p')[90];
        const range = document.createRange(); range.setStart(target.firstChild, 10); range.collapse(true);
        getSelection().removeAllRanges(); getSelection().addRange(range);
        const container = document.querySelector('.editor-container');
        container.scrollTop += range.getBoundingClientRect().top - container.getBoundingClientRect().top - 200;
      });
      await pause(600);
      const layoutBefore = await metrics(page);
      const marker = `REMOTE_LAYOUT_${anchoring}`;
      await peer.evaluate(marker => {
        const editor = document.querySelector('.editor-content'); editor.focus();
        const target = [...editor.querySelectorAll(':scope > p')].find(p => p.textContent.length > 0);
        const range = document.createRange(); range.setStart(target.firstChild, 10); range.collapse(true);
        getSelection().removeAllRanges(); getSelection().addRange(range);
        const data = new DataTransfer();
        data.setData('text/html', `<h2>${marker}</h2><p>Inserted above the local caret.</p><p>Another inserted paragraph.</p>`);
        const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
        Object.defineProperty(event, 'clipboardData', { value: data }); editor.dispatchEvent(event);
      }, marker);
      await waitFor(page, () => document.querySelector('.editor-content').textContent.includes('REMOTE_LAYOUT_'));
      await pause(250);
      const inserted = await metrics(page);
      assert(inserted.height > layoutBefore.height, 'Remote blocks must change layout');
      assert(inserted.caretTop >= inserted.viewportTop && inserted.caretBottom <= inserted.viewportBottom,
        'Remote insertion hid local caret');
      await peer.keyboard.press('Control+z');
      await waitFor(page, () => !document.querySelector('.editor-content').textContent.includes('REMOTE_LAYOUT_'));
      await pause(250);
      const deleted = await metrics(page);
      assert(Math.abs(deleted.height - layoutBefore.height) <= 1, 'Remote undo must restore block layout');
      assert(deleted.caretTop >= deleted.viewportTop && deleted.caretBottom <= deleted.viewportBottom,
        'Remote deletion hid local caret');
      reports.push({ scenario: `remote-layout-${anchoring}`, before: layoutBefore, steps: [inserted, deleted] });
    }
    console.log(`PASS ${engine} viewport-edge wrapping and remote block insertion/deletion recorded`);

    const pendingImages = new Map();
    const readyImages = new Set();
    const sendImage = response => {
      if (response.destroyed) return;
      response.writeHead(200, { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'max-age=3600' });
      response.end('<svg xmlns="http://www.w3.org/2000/svg" width="600" height="600"><rect width="600" height="600" fill="gray"/></svg>');
    };
    imageCertificateDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ogre-scroll-image-'));
    const key = path.join(imageCertificateDir, 'key.pem');
    const cert = path.join(imageCertificateDir, 'cert.pem');
    const config = path.join(imageCertificateDir, 'openssl.cnf');
    fs.writeFileSync(config, '[req]\ndistinguished_name=dn\nprompt=no\n[dn]\nCN=127.0.0.1\n[v3]\nsubjectAltName=IP:127.0.0.1\n');
    const generated = spawnSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes',
      '-keyout', key, '-out', cert, '-days', '1', '-config', config, '-extensions', 'v3'], { stdio: 'ignore' });
    assert.equal(generated.status, 0, 'openssl is required for the local delayed-image fixture');
    imageServer = https.createServer({ key: fs.readFileSync(key), cert: fs.readFileSync(cert) }, (request, response) => {
      if (readyImages.has(request.url)) sendImage(response);
      else {
        if (!pendingImages.has(request.url)) pendingImages.set(request.url, []);
        pendingImages.get(request.url).push(response);
      }
    });
    await new Promise((resolve, reject) => { imageServer.once('error', reject); imageServer.listen(0, '127.0.0.1', resolve); });
    for (const anchoring of ['auto', 'default']) {
      await page.evaluate(anchoring => {
        const editor = document.querySelector('.editor-content'); editor.focus({ preventScroll: true });
        const container = document.querySelector('.editor-container');
        if (anchoring === 'auto') container.style.overflowAnchor = 'auto';
        else container.style.removeProperty('overflow-anchor');
        const target = editor.querySelectorAll(':scope > p')[90];
        const range = document.createRange(); range.setStart(target.firstChild, 10); range.collapse(true);
        getSelection().removeAllRanges(); getSelection().addRange(range);
        container.scrollTop += range.getBoundingClientRect().top - container.getBoundingClientRect().top - 200;
      }, anchoring);
      const imagePath = `/delayed-${anchoring}.svg`;
      const imageUrl = `https://127.0.0.1:${imageServer.address().port}${imagePath}`;
      const marker = `Delayed viewport probe ${anchoring}`;
      await peer.evaluate(({ imageUrl, marker }) => {
        const editor = document.querySelector('.editor-content'); editor.focus();
        const target = [...editor.querySelectorAll(':scope > p')].find(p => p.textContent.length > 0);
        const range = document.createRange(); range.setStart(target.firstChild, 10); range.collapse(true);
        getSelection().removeAllRanges(); getSelection().addRange(range);
        const data = new DataTransfer();
        data.setData('text/html', `<h2>Delayed image fixture</h2><img src="${imageUrl}" alt="${marker}">`);
        const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
        Object.defineProperty(event, 'clipboardData', { value: data }); editor.dispatchEvent(event);
      }, { imageUrl, marker });
      await waitFor(page, marker => !!document.querySelector(`img[alt="${marker}"]`), marker);
      await pause(1500);
      assert.equal(await page.evaluate(() => getComputedStyle(document.querySelector('.editor-container')).overflowAnchor),
        'auto', 'Native anchoring must be restored after redraw');
      const imageBefore = await metrics(page);
      readyImages.add(imagePath); (pendingImages.get(imagePath) || []).forEach(sendImage);
      await waitFor(page, marker => document.querySelector(`img[alt="${marker}"]`).naturalHeight > 0, marker);
      await pause(250);
      const imageAfter = await metrics(page);
      reports.push({ scenario: `delayed-image-${anchoring}`, before: imageBefore, steps: [imageAfter] });
      assert(imageAfter.height - imageBefore.height > 300, 'Image must gain height above viewport');
      const shift = Math.abs(imageAfter.caretTop - imageBefore.caretTop);
      if (engine === 'firefox') assert(shift <= 1, 'Firefox must compensate for delayed image height');
    }
    console.log(`PASS ${engine} delayed images: native anchoring restored after redraw`);
    if (baseline) {
      assert.equal(baseline.browser, engine, 'Baseline must use the same browser');
      for (const scenario of ['wrap-edge-auto', 'remote-layout-auto', 'delayed-image-default']) {
        const before = baseline.reports.find(report => report.scenario === scenario);
        const after = reports.find(report => report.scenario === scenario);
        assert(before && after, `Baseline missing ${scenario}`);
        if (scenario === 'wrap-edge-auto') {
          const overflow = report => Math.max(...report.steps.map(step => Math.max(0, step.caretBottom - step.viewportBottom)));
          if (expectCaretVisible) assert(overflow(before) > 1, 'Baseline must reproduce an invisible caret');
          assert(overflow(after) <= overflow(before) + 1, 'New boundary caret regression against unpatched baseline');
          assert.equal(after.steps.length, before.steps.length, 'Wrapping must use matching transitions');
          after.steps.forEach((step, index) => {
            const old = before.steps[index];
            if (!expectCaretVisible || old.caretBottom <= old.viewportBottom + 1) {
              assert(Math.abs((step.top - after.before.top) - (old.top - before.before.top)) <= 1,
                'New wrapping scroll movement against unpatched baseline');
              assert(Math.abs((step.caretTop - after.before.caretTop) - (old.caretTop - before.before.caretTop)) <= 1,
                'New wrapping caret movement against unpatched baseline');
            }
          });
        } else {
          assert.equal(after.steps.length, before.steps.length);
          after.steps.forEach((step, index) => assert(
            Math.abs(step.caretTop - after.before.caretTop) <= Math.abs(before.steps[index].caretTop - before.before.caretTop) + 1,
            `New ${scenario} caret regression against unpatched baseline`));
        }
      }
      console.log(`PASS ${engine} layout changes match unsuppressed baseline build`);
    }

  } catch (error) {
    failure = error;
    throw error;
  } finally {
    const cleanupErrors = [];
    try {
      fs.writeFileSync(path.join(out, 'report.json'), JSON.stringify({ harnessVersion: 2, browser: engine, reports }, null, 2));
    } catch (error) { cleanupErrors.push(error); }
    try { if (page) await page.close(); } catch (error) { cleanupErrors.push(error); }
    const deleted = await Promise.allSettled(ids.map(async id => {
      await api('DELETE', `/documents/${id}`);
      await api('DELETE', `/documents/${id}/purge`);
    }));
    for (const result of deleted) if (result.status === 'rejected') cleanupErrors.push(result.reason);
    try { await browser.close(); } catch (error) { cleanupErrors.push(error); }
    try {
      if (imageServer?.listening) {
        imageServer.closeAllConnections();
        await new Promise((resolve, reject) => imageServer.close(error => error ? reject(error) : resolve()));
      }
    } catch (error) { cleanupErrors.push(error); }
    try { if (imageCertificateDir) fs.rmSync(imageCertificateDir, { recursive: true, force: true }); }
    catch (error) { cleanupErrors.push(error); }
    if (cleanupErrors.length) {
      const error = new AggregateError(cleanupErrors, 'Regression cleanup failed');
      if (failure) console.error(error);
      else throw error;
    }
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
