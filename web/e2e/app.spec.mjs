import { firefox, chromium } from 'playwright';
import { startServer } from './serve.mjs';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const distDir = resolve(__dirname, '../dist');
const dbDir = process.env.E2E_DB_DIR ?? resolve(__dirname, '../../target/fixture-export');

const args = process.argv.slice(2);
const engineIdx = args.indexOf('--engine');
const engineName = engineIdx !== -1 ? args[engineIdx + 1] : 'firefox';

const engines = { firefox, chromium };
const engine = engines[engineName];
if (!engine) {
  console.error(`Unknown --engine "${engineName}". Valid: firefox, chromium`);
  process.exit(1);
}

function fail(msg) {
  throw new Error(`FAIL: ${msg}`);
}

async function waitForHash(page, expected, timeout = 10000) {
  await page.waitForFunction(
    (h) => window.location.hash === h,
    expected,
    { timeout },
  );
}

async function assertText(page, selector, substr, timeout = 10000) {
  await page.waitForSelector(selector, { timeout });
  const text = await page.$eval(selector, (el) => el.textContent ?? '');
  if (!text.includes(substr)) fail(`"${selector}" text "${text}" does not include "${substr}"`);
}

async function assertCount(page, selector, expected, timeout = 10000) {
  await page.waitForSelector(selector, { timeout });
  const count = await page.$$eval(selector, (els) => els.length);
  if (count !== expected) fail(`Expected ${expected} "${selector}" elements, got ${count}`);
}

async function run() {
  const { port, close } = await startServer(distDir, { dbDir });
  const base = `http://127.0.0.1:${port}`;
  console.log(`server  ${base}  (${engineName})`);

  const browser = await engine.launch({ headless: true });
  const context = await browser.newContext();
  const page = await context.newPage();

  const consoleErrors = [];
  page.on('console', (m) => { if (m.type() === 'error') consoleErrors.push(m.text()); });

  try {
    // 1. Landing page — search input focused, fixture specs listed
    await page.goto(`${base}/#/`);
    await page.waitForSelector('input[type=search]', { timeout: 10000 });
    const focused = await page.evaluate(
      () => document.activeElement?.getAttribute('type') === 'search',
    );
    if (!focused) fail('search input not focused on landing');
    await page.waitForSelector('a[href="#/HTML"]', { timeout: 10000 });
    console.log('ok  landing: search focused, HTML link visible');

    // 2. Section view — navigate algorithm
    await page.goto(`${base}/#/HTML/navigate`);
    await page.waitForSelector('h1', { timeout: 10000 });
    const h1Text = await page.$eval('h1', (el) => el.textContent ?? '');
    if (!h1Text.toLowerCase().includes('navigate')) {
      fail(`h1 "${h1Text}" does not contain "navigate"`);
    }

    // Outgoing ref to DOM#concept-tree (in the refs section)
    await page.waitForSelector('a[href="#/DOM/concept-tree"]', { timeout: 10000 });
    await page.click('a[href="#/DOM/concept-tree"]');
    await waitForHash(page, '#/DOM/concept-tree');
    console.log('ok  HTML/navigate: outgoing ref to DOM/concept-tree, click navigates');

    // 3. DOM/concept-tree — incoming refs back to HTML/navigate
    await page.waitForSelector('h1', { timeout: 10000 });
    // "Incoming references" heading
    await page.waitForFunction(
      () => Array.from(document.querySelectorAll('h3')).some((h) => h.textContent?.includes('Incoming')),
      { timeout: 10000 },
    );
    await page.waitForSelector('a[href="#/HTML/navigate"]', { timeout: 10000 });
    console.log('ok  DOM/concept-tree: "Incoming" heading visible, back-link to HTML/navigate');

    // 4. Full-URL redirect
    await page.goto(`${base}/#/https://html.spec.whatwg.org/#navigate`);
    await waitForHash(page, '#/HTML/navigate');
    console.log('ok  full-URL redirect → #/HTML/navigate');

    // 5. Search
    await page.goto(`${base}/#/search?q=navigate`);
    await page.waitForSelector('.search-result-card', { timeout: 10000 });
    const firstCard = await page.$eval('.search-result-card', (el) => el.textContent ?? '');
    if (!firstCard.toLowerCase().includes('navigate')) {
      fail(`First search result "${firstCard}" does not include "navigate"`);
    }
    console.log('ok  search: first result contains "navigate"');

    // 6. Headings list — fixture has 2 HTML sections
    await page.goto(`${base}/#/HTML`);
    await assertCount(page, '.headings-entry', 1);
    console.log('ok  HTML headings: 1 heading entry (navigate is algorithm type)');

    // 7. Error banner — FETCH not in index
    await page.goto(`${base}/#/FETCH/x`);
    await assertText(page, '.error-banner', 'not part of this index');
    console.log('ok  error banner: "not part of this index"');

    // 8. Effects panel header on section view (fixture has effect_runs but no per-section results)
    await page.goto(`${base}/#/HTML/navigate`);
    await page.waitForSelector('.effects-panel-title', { timeout: 10000 });
    const effectsTitle = await page.$eval('.effects-panel-title', (el) => el.textContent ?? '');
    if (!effectsTitle.includes('Possible effects')) {
      fail(`Effects panel title "${effectsTitle}" does not include "Possible effects"`);
    }
    console.log('ok  effects panel header "Possible effects"');

    // 9a. Reference graph — Graph toggle renders svg, wheel and drag update transform
    await page.goto(`${base}/#/HTML/navigate`);
    await page.waitForSelector('.refs-graph-toggle', { timeout: 10000 });
    await page.click('.refs-graph-toggle');
    await page.waitForSelector('.refs-graph svg[role="img"]', { timeout: 10000 });
    // Wait for layout to complete (g[transform] appears after async dagre layout)
    await page.waitForFunction(
      () => document.querySelector('.refs-graph svg[role="img"] > g[transform]') !== null,
      null,
      { timeout: 10000 },
    );
    const getGraphTransform = () => page.$eval(
      '.refs-graph svg[role="img"] > g[transform]',
      (g) => g.getAttribute('transform') ?? '',
    );

    // Scroll SVG into viewport and use ElementHandle for reliable coordinates
    const svgHandle = await page.$('.refs-graph svg[role="img"]');
    await svgHandle.scrollIntoViewIfNeeded();
    const svgBox = await svgHandle.boundingBox();
    const svgCx = svgBox.x + svgBox.width / 2;
    const svgCy = svgBox.y + svgBox.height / 2;

    const t0 = await getGraphTransform();
    // Verify Zoom In button changes transform (sanity check)
    await page.click('.refs-graph .diagram-toolbar button:has-text("Zoom in")');
    const tAfterZoomIn = await getGraphTransform();
    if (tAfterZoomIn === t0) fail(`Zoom in button did not change transform (t0="${t0}")`);
    // Reset via Fit
    await page.click('.refs-graph .diagram-toolbar button:has-text("Fit")');
    const tAfterFit = await getGraphTransform();

    // Dispatch wheel via Playwright locator API (reliable across browsers)
    await page.locator('.refs-graph svg[role="img"]').dispatchEvent('wheel', {
      deltaY: -120,
      deltaMode: 0,
      bubbles: true,
      cancelable: true,
      clientX: svgCx,
      clientY: svgCy,
    });
    await page.waitForFunction(
      (initial) => {
        const g = document.querySelector('.refs-graph svg[role="img"] > g[transform]');
        return g !== null && g.getAttribute('transform') !== initial;
      },
      tAfterFit,
      { timeout: 8000 },
    );
    console.log('ok  Graph toggle: svg[role="img"] appears and wheel changes transform');

    const t1 = await getGraphTransform();
    await page.mouse.move(svgCx, svgCy);
    await page.mouse.down();
    await page.mouse.move(svgCx + 60, svgCy + 40, { steps: 5 });
    await page.mouse.up();
    await page.waitForFunction(
      (initial) => {
        const g = document.querySelector('.refs-graph svg[role="img"] > g[transform]');
        return g !== null && g.getAttribute('transform') !== initial;
      },
      t1,
      { timeout: 8000 },
    );
    console.log('ok  Graph toggle: pointer drag changes transform');

    // 9b. Flow view — segmented control, nodes (branch + terminal), step click sets ?step=,
    //     Show calls as nodes reveals DOM#concept-tree link
    await page.goto(`${base}/#/HTML/navigate`);
    await page.waitForSelector('h1', { timeout: 10000 });
    // Turn off Graph toggle if still on (avoid a second svg[role=img] from the refs graph)
    const graphToggle = await page.$('.refs-graph-toggle--on');
    if (graphToggle) await graphToggle.click();

    await page.waitForSelector('button[aria-pressed]', { timeout: 10000 });
    // Click the Flow button in the segmented control
    const flowBtnSel = 'button[aria-pressed]:has-text("Flow")';
    await page.waitForSelector(flowBtnSel, { timeout: 5000 });
    await page.click(flowBtnSel);
    // Flow view makes a WASM API request before rendering; allow extra time
    await page.waitForSelector('.flow-view svg[role="img"]', { timeout: 20000 });
    // Wait for node groups to appear inside the flow svg
    await page.waitForFunction(
      () => document.querySelectorAll('.flow-view svg[role="img"] g[role="button"]').length >= 4,
      null,
      { timeout: 15000 },
    );
    const flowNodeCount = await page.$$eval('.flow-view svg[role="img"] g[role="button"]', (els) => els.length);
    if (flowNodeCount < 4) fail(`Expected >= 4 flow node groups, got ${flowNodeCount}`);
    await page.waitForSelector('.flow-view .diagram-node--branch', { timeout: 5000 });
    await page.waitForSelector('.flow-view .diagram-node--terminal', { timeout: 5000 });
    console.log(`ok  Flow view: ${flowNodeCount} g[role=button] nodes, branch and terminal present`);

    // Click a step node via evaluate to avoid SVG pointer-events subtleties, assert ?step= in hash
    await page.evaluate(() => {
      const nodes = document.querySelectorAll('.flow-view svg[role="img"] g[role="button"]');
      if (nodes.length > 0) nodes[0].dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    });
    await page.waitForFunction(
      () => window.location.hash.includes('?step='),
      null,
      { timeout: 8000 },
    );
    console.log('ok  Flow view: clicking step node sets ?step= in hash');

    // Show calls as nodes — external node for DOM#concept-tree appears as <a>
    await page.click('.flow-toggle');
    await page.waitForSelector('.flow-view svg[role="img"] a[href="#/DOM/concept-tree"]', { timeout: 15000 });
    console.log('ok  Flow view: Show calls as nodes reveals a[href="#/DOM/concept-tree"]');

    // 9c. Text mode — step-select-btn click shows section-step-card in aside
    await page.goto(`${base}/#/HTML/navigate`);
    await page.waitForSelector('h1', { timeout: 10000 });
    // Switch to Text mode (may be in Flow from localStorage)
    const textBtnSel = 'button[aria-pressed]:has-text("Text")';
    const textBtn = await page.$(textBtnSel);
    if (textBtn) {
      const pressed = await textBtn.getAttribute('aria-pressed');
      if (pressed !== 'true') await textBtn.click();
    }
    await page.waitForSelector('.step-select-btn', { timeout: 10000 });
    // Use evaluate to click because the button is positioned in the gutter (opacity:0, left:-2.75em)
    // and Playwright's viewport-based click sees the parent div at that coordinate.
    await page.evaluate(() => {
      const btn = document.querySelector('.step-select-btn');
      if (btn) btn.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    });
    await page.waitForSelector('.section-step-card', { timeout: 8000 });
    console.log('ok  Text mode: step-select-btn click shows .section-step-card in aside');

    // 9d. Trace panel — open via header button, press t to record, Diagram toggle shows svg
    await page.goto(`${base}/#/HTML/navigate`);
    await page.waitForSelector('h1', { timeout: 10000 });
    // Ensure trace panel is open
    const tracePanelOpen = await page.$('.trace-panel');
    if (!tracePanelOpen) {
      await page.click('.trace-toggle');
    }
    await page.waitForSelector('.trace-panel', { timeout: 5000 });
    // Press t to record the current section into the trace
    await page.keyboard.press('t');
    // Diagram button appears only when entries > 0
    await page.waitForSelector('.trace-diagram-toggle', { timeout: 5000 });
    await page.click('.trace-diagram-toggle');
    await page.waitForSelector('.trace-panel svg[role="img"]', { timeout: 15000 });
    console.log('ok  Trace panel: Diagram toggle shows svg[role="img"]');

    // Console errors
    if (consoleErrors.length > 0) {
      fail(`${consoleErrors.length} browser console error(s):\n      ${consoleErrors.join('\n      ')}`);
    }
    console.log('ok  no browser console errors');

    console.log(`\ne2e ok`);

  } finally {
    await browser.close();
    await close();
  }
}

run().catch((err) => {
  console.error(err.message ?? String(err));
  process.exit(1);
});
