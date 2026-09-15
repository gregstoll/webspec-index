import { firefox, chromium } from 'playwright';
import { startServer } from './serve.mjs';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const distDir = resolve(__dirname, '../dist');

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
  const { port, close } = await startServer(distDir);
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
