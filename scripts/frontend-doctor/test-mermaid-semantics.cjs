// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
// Real editor, modal and SVG checks. Requires a local DEV_MODE stack.
const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const args = process.argv.slice(2);
const option = (key, fallback) => args.includes(key) ? args[args.indexOf(key) + 1] : fallback;
const base = option('--base-url', 'http://localhost:3000');
const observe = args.includes('--observe');
const failures = [];
function check(condition, message) {
  if (condition) console.log('PASS ' + message);
  else if (observe) { failures.push(message); console.log('REPRO ' + message); }
  else assert(condition, message);
}
const cases = [
  ['calendar', 'gantt\nexcludes weekends\nt :2024-01-05, 2d', 'Line 2: unsupported Gantt directive'],
  ['date-format', 'gantt\ndateFormat YYYY-DD-MM\nt :2024-02-01, 2d', 'Line 2: unsupported Gantt directive'],
  ['invalid-axis', 'xychart-beta\ny-axis 0 --> nope\nbar [20,40]', 'Line 2:'],
  ['derived-axis', 'xychart-beta\nbar [1e308]\nline [-1e308]\nline [1]', 'Line 3:'],
  ['nonfinite-point', 'quadrantChart\nBad: [NaN,0.5]', 'Line 2:'],
  ['out-of-range-point', 'quadrantChart\nBad: [2,0.5]', 'Line 2:'],
  ['overlapping-fields', 'packet-beta\n0-7: "First"\n4-9: "Second"', 'Line 3:'],
  ['header-tail', 'graph LR A --> B', 'Line 1:'],
  ['unknown-C4', 'C4Context\nMadeUp(a, "Lost")', 'Line 2:'],
  ['treemap-style', 'treemap-beta\nclassDef wrong fill:red', 'Line 2:'],
];
(async () => {
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROME_BIN ? { executablePath: process.env.CHROME_BIN } : {}) });
  const context = await browser.newContext();
  let id, importedId, token, viewerContext;
  const errors = [];
  async function api(method, route, data) {
    const response = await context.request.fetch(base + '/api/v1' + route, {
      method, data, headers: token ? { Authorization: 'Bearer ' + token } : {},
    });
    assert(response.ok(), `${method} ${route}: ${response.status()}`);
    return response.headers()['content-type']?.includes('application/json') ? response.json() : null;
  }
  try {
    token = (await api('POST', '/auth/dev-login', {
      email: `mermaid-semantics-${Date.now()}@ogrenotes.example.com`, name: 'Mermaid regression',
    })).accessToken;
    id = (await api('POST', '/documents', { title: 'Mermaid semantics regression' })).id;
    const page = await context.newPage();
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(`${base}/d/${id}/regression`);
    await page.locator('.editor-content[data-editor-ready=true]').waitFor();
    await page.locator('.editor-content').focus();
    await page.evaluate(() => {
      const data = new DataTransfer();
      data.setData('text/plain', '```mermaid\nflowchart TD\nA --> B\n```');
      document.querySelector('.editor-content').dispatchEvent(new ClipboardEvent('paste', {
        clipboardData: data, bubbles: true, cancelable: true,
      }));
    });
    const block = page.locator('.mermaid-block');
    await block.waitFor();
    check(await block.locator('svg[role=img]').count() === 1, 'SVG has an image role');
    const button = block.getByRole('button', { name: 'Edit Diagram', exact: true });
    check(await button.count() === 1, 'Diagram has a native keyboard edit button');
    if (await button.count()) {
      await button.focus(); await page.keyboard.press('Enter');
    } else await block.click();
    const modal = page.locator('.mermaid-modal');
    await modal.waitFor();
    check(await page.getByRole('dialog', { name: 'Edit Diagram', exact: true }).count() === 1, 'Modal has an accessible name');
    check(await page.getByRole('textbox', { name: 'Diagram source', exact: true }).count() === 1, 'Source input has an accessible name');
    const input = page.locator('.mermaid-source');
    for (const [name, source, message] of cases) {
      await input.fill(source);
      await page.waitForTimeout(250);
      const error = await page.locator('.mermaid-preview').textContent();
      check(error.includes(message), `${name} reports its original source line`);
    }
    await input.fill('xychart-beta\nx-axis "Months" ["Jan, first", Feb]\ny-axis Revenue 0 --> 100\nbar [20,40]');
    await page.waitForTimeout(250);
    const labels = await page.locator('.mermaid-preview svg text').allTextContents();
    check(['Months', 'Jan, first', 'Feb', 'Revenue', '100'].every(label => labels.includes(label)), 'XY chart retains axis titles and quoted categories');
    await input.fill('gantt\ndateFormat YYYY-MM-DD\naxisFormat %Y-%m-%d\ntodayMarker off\nt :2024-01-02, 2d');
    await page.waitForTimeout(250);
    check(await page.locator('.mermaid-preview svg').count() === 1, 'Supported Gantt directive forms render');
    await input.fill('flowchart TD %% inline comment\nA --> B');
    await page.waitForTimeout(250);
    check(await page.locator('.mermaid-preview svg').count() === 1, 'Flowchart header accepts an inline comment');
    await input.fill('C4Context\nPerson(p, "Reader", "description")');
    await page.waitForTimeout(250);
    const descriptions = await page.locator('.mermaid-preview svg desc').allTextContents();
    check(descriptions.length === 1 && descriptions[0].split('Reader').length === 2, 'Default description includes each visible C4 label once');
    await input.fill('xychart-beta\nx-axis 10 --> 20\nline [1,2,3]');
    await page.waitForTimeout(250);
    const chart = await page.locator('.mermaid-preview svg').evaluate(svg => ({
      points: svg.querySelector('polyline')?.getAttribute('points'),
      ticks: [...svg.querySelectorAll('text')].filter(t => ['10','15','20'].includes(t.textContent)).map(t => Number(t.getAttribute('x'))),
    }));
    check(chart.points?.startsWith('76.0,') && chart.points.includes(' 536.0,') && chart.ticks.join(',') === '76,306,536', 'Numeric XY ticks align with line data endpoints');
    const accessible = 'flowchart TD\naccTitle: Deployment & health\naccDescr {\nProcesses run in order.\n}\nA --> B';
    await input.fill(accessible);
    await page.waitForTimeout(250);
    const svg = page.locator('.mermaid-preview svg');
    check(await svg.count() === 1 && await svg.getAttribute('aria-label') === 'Deployment & health', 'Accessibility title reaches rendered SVG');
    check(await svg.count() === 1 && await svg.locator('desc').textContent() === 'Processes run in order.', 'Accessibility description reaches rendered SVG');
    if (!observe) {
      const session = await context.newCDPSession(page);
      const tree = await session.send('Accessibility.getFullAXTree');
      const image = tree.nodes.find(node => node.role?.value === 'image' && node.name?.value === 'Deployment & health');
      check(image?.description?.value === 'Processes run in order.', 'Browser accessibility tree exposes the image name and description');
      await session.detach();
    }
    await page.locator('.mermaid-modal button.btn-primary').click();
    await modal.waitFor({ state: 'detached' });
    await page.waitForTimeout(100);
    if (await button.count()) check(await button.evaluate(button => document.activeElement === button), 'Save restores focus to the replacement edit button');
    await page.reload();
    await page.locator('.editor-content[data-editor-ready=true]').waitFor();
    check(await block.locator('svg').count() === 1 && await block.locator('svg').getAttribute('aria-label') === 'Deployment & health', 'Accessible source survives save and reload');
    if (await button.count()) {
      await button.focus(); await page.keyboard.press('Space');
      await modal.waitFor();
      await page.locator('.mermaid-source:focus').waitFor();
      await page.keyboard.press('Escape');
      await modal.waitFor({ state: 'detached' });
      // Focus restoration runs on a deferred task after modal teardown.
      await page.locator('.mermaid-block .mermaid-edit-control:focus').waitFor({ timeout: 5000 });
      check(await button.evaluate(button => document.activeElement === button), 'Space opens the modal and Escape restores button focus');
    }
    if (!observe) {
      viewerContext = await browser.newContext();
      const response = await viewerContext.request.post(base + '/api/v1/auth/dev-login', { data: {
        email: `mermaid-viewer-${Date.now()}@ogrenotes.example.com`, name: 'Diagram reader',
      }});
      assert(response.ok());
      const viewer = await response.json();
      await api('POST', `/documents/${id}/members`, { userId: viewer.userId, accessLevel: 'VIEW' });
      const reader = await viewerContext.newPage();
      reader.on('pageerror', error => errors.push(error.message));
      await reader.goto(`${base}/d/${id}/regression`);
      await reader.locator('.editor-content-readonly .mermaid-block svg').waitFor();
      check(await reader.locator('.mermaid-edit-control').count() === 0, 'Read-only diagrams have no edit control');
      await reader.locator('.mermaid-block').click();
      check(await reader.locator('.mermaid-modal').count() === 0, 'Read-only diagrams cannot open the edit modal');
    }
    if (!observe) {
      await button.click(); await modal.waitFor();
      await input.fill('gantt\nexcludes weekends\nt :2024-01-05, 2d');
      await page.locator('.mermaid-modal button.btn-primary').click();
      await modal.waitFor({ state: 'detached' });
      // The sync pill is polled and may still show "Saved" from the prior
      // edit. Check the actual export until this specific update is visible.
      const deadline = Date.now() + 5000;
      let exportHasError = false;
      do {
        const exported = await context.request.get(base + `/api/v1/documents/${id}/export/html`, {
          headers: { Authorization: 'Bearer ' + token },
        });
        assert(exported.ok());
        exportHasError = (await exported.text()).includes('Line 2: unsupported Gantt directive');
        if (!exportHasError) await page.waitForTimeout(100);
      } while (!exportHasError && Date.now() < deadline);
      check(exportHasError, 'HTML export keeps the original Mermaid error line');
    }
    importedId = (await api('POST', '/documents/import', {
      title: 'Imported Mermaid regression', format: 'markdown',
      content: 'Before.\n\n```mermaid\nflowchart LR\nImported --> Editable\n```\n\nAfter.',
    })).id;
    const importedPage = await context.newPage();
    importedPage.on('pageerror', error => errors.push(error.message));
    await importedPage.goto(`${base}/d/${importedId}/regression`);
    await importedPage.locator('.editor-content[data-editor-ready=true]').waitFor();
    const importedBlock = importedPage.locator('.mermaid-block');
    await importedBlock.locator('svg').waitFor();
    check(Boolean(await importedBlock.getAttribute('data-block-id')), 'Imported diagram has a stable editing anchor');
    await importedBlock.getByRole('button', { name: 'Edit Diagram', exact: true }).click();
    const wideSource = 'flowchart LR\n' + Array.from({length: 12}, (_, index) =>
      `n${index}["Updated imported diagram node ${index}"] --> n${index+1}`).join('\n');
    await importedPage.locator('.mermaid-source').fill(wideSource);
    await importedPage.locator('.mermaid-modal button.btn-primary').click();
    await importedPage.locator('.mermaid-modal').waitFor({ state: 'detached' });
    let importedHtml = '';
    const importedDeadline = Date.now() + 8000;
    do {
      const response = await context.request.get(base + `/api/v1/documents/${importedId}/export/html`, {
        headers: { Authorization: 'Bearer ' + token },
      });
      assert(response.ok());
      importedHtml = await response.text();
      if (!importedHtml.includes('Updated imported diagram node 11')) await importedPage.waitForTimeout(100);
    } while (!importedHtml.includes('Updated imported diagram node 11') && Date.now() < importedDeadline);
    check(importedHtml.includes('Updated imported diagram node 11'), 'Imported diagram edits persist through Yrs to server export');
    await importedPage.reload();
    await importedPage.locator('.mermaid-block svg').waitFor();
    check((await importedPage.locator('.mermaid-block svg').textContent()).includes('Updated imported diagram node 11'), 'Imported live diagram survives reload');
    const printPage = await context.newPage();
    await printPage.setContent('<div style="width:700px">' + importedHtml + '</div>');
    await printPage.emulateMedia({ media: 'print' });
    const printed = await printPage.locator('.mermaid-block svg').evaluate(svg => ({
      width: svg.getBoundingClientRect().width,
      available: svg.parentElement.getBoundingClientRect().width,
      ratio: svg.getBoundingClientRect().width / svg.getBoundingClientRect().height,
      intrinsicRatio: Number(svg.getAttribute('width')) / Number(svg.getAttribute('height')),
    }));
    check(printed.width <= printed.available + 1 && Math.abs(printed.ratio / printed.intrinsicRatio - 1) < 0.02,
      'Wide exported Mermaid scales to printed width without changing aspect ratio');
    assert.deepEqual(errors, [], 'Browser raised an error');
    if (observe) console.log(`Observed ${failures.length} reproduced defects`);
  } finally {
    for (const documentId of [id, importedId].filter(Boolean)) {
      if (token) {
        await api('DELETE', `/documents/${documentId}`);
        await api('DELETE', `/documents/${documentId}/purge`);
      }
    }
    if (viewerContext) await viewerContext.close();
    await context.close();
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
