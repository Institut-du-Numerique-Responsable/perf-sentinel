// Page tests for /technologies and /fr/technologies: the committed pages on a local
// static server and Playwright (resolved like verifier-page.test.mjs), every
// third-party request aborted and recorded. The expected rows are those of the data.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './static-server.mjs';

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require('playwright')); }
catch { ({ chromium } = require('/Users/robintrassard/RustroverProjects/perf-sentinel/crates/sentinel-cli/tests/browser/node_modules/playwright')); }

const SITE = join(dirname(fileURLToPath(import.meta.url)), '..');

let server;
let browser;
before(async () => {
  server = await startServer();
  browser = await chromium.launch();
});
after(async () => {
  await browser?.close();
  server?.close();
});

async function open(path, { javaScriptEnabled = true, viewport = { width: 1280, height: 900 } } = {}) {
  const context = await browser.newContext({ javaScriptEnabled, viewport });
  const external = [];
  await context.route('**/*', (route) => {
    const url = new URL(route.request().url());
    if (url.hostname === '127.0.0.1') return route.fallback();
    external.push(url.href);
    return route.abort();
  });
  context.setDefaultTimeout(10000);
  const page = await context.newPage();
  const dialogs = [];
  page.on('dialog', (d) => { dialogs.push(d.message()); d.dismiss(); });
  await page.goto(server.url + path);
  return { page, external, dialogs, close: () => context.close() };
}

const FAMILIES = {
  en: ['Frameworks and ORMs', 'Databases', 'Brokers', 'Trace sources', 'Clouds', 'Energy measurement', 'Electricity grids', 'Continuous integration'],
  fr: ['Frameworks et ORM', 'Bases de données', 'Brokers', 'Sources de traces', 'Clouds', 'Mesure d’énergie', 'Réseaux électriques', 'Intégration continue'],
};
const IDS = ['frameworks', 'databases', 'brokers', 'traces', 'clouds', 'energy', 'grids', 'ci'];
const ROWS = 86;
const PRODUCTION = ['Spring Boot', 'PostgreSQL', 'RabbitMQ', 'OTLP', 'Tempo', 'AWS', 'Jenkins'];

const count = (page, sel) => page.locator(sel).count();
const visibleRows = (page) => page.locator('tr.row:visible').count();
const visibleFamilies = (page) => page.locator('section.fam:visible').count();
const search = async (page, q) => { await page.fill('#tp-q', q); };

test('renders one section per family, in order, in both languages', async () => {
  for (const [lang, path] of [['en', '/technologies'], ['fr', '/fr/technologies']]) {
    const { page, close } = await open(path);
    try {
      assert.equal(await page.getAttribute('html', 'lang'), lang);
      const sections = await page.$$eval('section.fam', (els) => els.map((s) => [s.id, s.querySelector('h2').textContent.trim()]));
      assert.deepEqual(sections, IDS.map((id, i) => [id, FAMILIES[lang][i]]));
      assert.equal(await count(page, 'tr.row'), ROWS);
      assert.equal(await count(page, 'section.fam table'), IDS.length);
      const rail = await page.$$eval('.tp-rail a', (els) => els.map((a) => a.getAttribute('href')));
      assert.deepEqual(rail, IDS.map((id) => '#' + id));
    } finally { await close(); }
  }
});

test('search keeps matching rows and hides empty families', async () => {
  const { page, close } = await open('/technologies');
  try {
    await search(page, 'KOTLIN');
    assert.equal(await visibleRows(page), 2);
    assert.equal(await visibleFamilies(page), 1);
    assert.deepEqual(await page.$$eval('tr.row:not([hidden]) th .n', (els) => els.map((e) => e.textContent.trim())), ['Ktor', 'Exposed']);
    assert.equal(await page.locator('tr.grp:visible').count(), 1);
    assert.equal(await page.isVisible('#tp-none'), false);
    await search(page, 'hibernate');
    assert.equal(await visibleRows(page), 2, 'Spring Boot by its detail and alias, NHibernate by its name');
    await search(page, '');
    assert.equal(await visibleRows(page), ROWS);
    assert.equal(await visibleFamilies(page), IDS.length);
  } finally { await close(); }
});

test('search treats markup and pattern characters as text', async () => {
  for (const [path, word] of [['/technologies', 'is not recognized yet'], ['/fr/technologies', 'n’est pas encore reconnue']]) {
    const { page, dialogs, close } = await open(path);
    try {
      for (const q of ['<img src=x onerror=alert(1)>', '.*', '$&']) {
        await search(page, q);
        assert.equal(await visibleRows(page), 0, q);
        assert.equal(await visibleFamilies(page), 0, q);
        assert.equal(await page.isVisible('#tp-none'), true, q);
        const heading = await page.textContent('#tp-none h2');
        assert.ok(heading.includes(q) && heading.includes(word), heading);
        assert.equal(await count(page, '#tp-none img'), 0);
      }
      await page.waitForTimeout(100);
      assert.deepEqual(dialogs, []);
    } finally { await close(); }
  }
});

test('a row expands with click, Enter and Space', async () => {
  const { page, close } = await open('/technologies');
  try {
    const row = page.locator('#grids tr.row').first();
    const button = row.locator('button');
    const detail = page.locator('#grids tr.det').first();
    assert.equal(await button.getAttribute('aria-expanded'), 'false');
    assert.equal(await detail.isVisible(), false);
    await row.locator('td.lvc').click();
    assert.equal(await button.getAttribute('aria-expanded'), 'true');
    assert.equal(await detail.isVisible(), true);
    await button.click();
    assert.equal(await button.getAttribute('aria-expanded'), 'false');
    assert.equal(await detail.isVisible(), false);
    await button.focus();
    await page.keyboard.press('Enter');
    assert.equal(await button.getAttribute('aria-expanded'), 'true');
    assert.equal(await detail.isVisible(), true);
    await search(page, 'kotlin');
    assert.equal(await detail.isVisible(), false, 'a row hidden by the search hides its open detail');
    await search(page, '');
    assert.equal(await detail.isVisible(), true);
    await button.focus();
    await page.keyboard.press('Space');
    assert.equal(await button.getAttribute('aria-expanded'), 'false');
    assert.equal(await detail.isVisible(), false);
  } finally { await close(); }
});

test('the details fold before the page script runs, never painted open', async () => {
  const context = await browser.newContext();
  try {
    // page.js never arrives: the inline html.js and page.css alone must fold every detail.
    await context.route('**/assets/technologies/page.js', (route) => route.abort());
    const page = await context.newPage();
    await page.goto(server.url + '/technologies');
    assert.equal(await page.evaluate(() => document.documentElement.classList.contains('js')), true);
    assert.equal(await page.locator('tr.det:visible').count(), 0);
    assert.equal(await count(page, 'tr.row button[aria-expanded]'), 0, 'no state claimed before the script owns it');
  } finally { await context.close(); }
  const { page, close } = await open('/technologies');
  try {
    assert.equal(await count(page, 'tr.row button[aria-expanded="false"]'), ROWS);
    assert.equal(await page.locator('tr.det:visible').count(), 0);
  } finally { await close(); }
});

test('each detail links to what is missing, and only that', async () => {
  const { page, close } = await open('/technologies', { javaScriptEnabled: false });
  const LAB = 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel-simulation-lab';
  const CONTRIBUTING = 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/blob/main/CONTRIBUTING.md#what-to-contribute-by-difficulty';
  const links = (id) => page.$$eval(`#${id} .lk a`, (els) => els.map((a) => [a.textContent, a.getAttribute('href')]));
  const report = ['Report a production use', '/references#figurer'];
  try {
    // Complete: nothing to contribute, its proof.
    assert.deepEqual(await links('d-frameworks-django'), [['See the lab', `${LAB}/tree/main/services/django-svc`], report]);
    // Incomplete with a proof: how to contribute, its first proof.
    assert.deepEqual(await links('d-frameworks-ktor'), [['How to contribute', CONTRIBUTING], ['See the lab', `${LAB}/tree/main/services/ktor-svc`], report]);
    // Incomplete, no proof, in a family with a lab criterion: the lab itself.
    assert.deepEqual(await links('d-brokers-kafka'), [['How to contribute', CONTRIBUTING], ['See the lab', LAB], report]);
    // A family without a lab criterion: never a lab link, complete or not.
    assert.deepEqual(await links('d-clouds-ovhcloud'), [['How to contribute', CONTRIBUTING], report]);
    assert.deepEqual(await links('d-clouds-aws'), [report]);
    // The contribute and report rules hold on every row.
    const all = await page.$$eval('tr.row', (rows) => rows.map((r) => ({
      full: r.querySelector('td.lvc').textContent.startsWith('Complete'),
      hrefs: [...r.nextElementSibling.querySelectorAll('.lk a')].map((a) => a.getAttribute('href')),
    })));
    assert.equal(all.length, ROWS);
    for (const { full, hrefs } of all) {
      assert.equal(hrefs.includes(CONTRIBUTING), !full);
      assert.equal(hrefs.at(-1), '/references#figurer');
    }
  } finally { await close(); }
});

test('the production filter keeps only ringed rows', async () => {
  const { page, close } = await open('/technologies#production');
  try {
    const names = () => page.$$eval('tr.row:not([hidden]) th .n', (els) => els.map((e) => e.textContent.trim()));
    assert.deepEqual(await names(), PRODUCTION);
    assert.equal(await page.locator('tr.row:visible svg.lv.pr').count(), PRODUCTION.length);
    assert.equal(await page.getAttribute('#tp-prod', 'aria-pressed'), 'true');
    await page.click('#tp-prod');
    assert.equal(await visibleRows(page), ROWS);
    assert.equal(await page.getAttribute('#tp-prod', 'aria-pressed'), 'false');
    await page.click('#tp-prod');
    assert.deepEqual(await names(), PRODUCTION);
    await search(page, 'postgres');
    assert.deepEqual(await names(), ['PostgreSQL']);
  } finally { await close(); }
});

test('the rail follows the scroll', async () => {
  const { page, close } = await open('/technologies');
  try {
    const current = async (href) => {
      await page.waitForFunction((h) => {
        const cur = document.querySelectorAll('.tp-rail a[aria-current]');
        return cur.length === 1 && cur[0].getAttribute('href') === h;
      }, href, { timeout: 3000 }).catch(async () => {
        const got = await page.$$eval('.tp-rail a[aria-current]', (els) => els.map((a) => a.getAttribute('href')));
        throw new Error(`rail on ${got}, expected ${href}`);
      });
    };
    assert.equal(await page.isVisible('.tp-rail'), true);
    await current('#frameworks');
    const jump = (id) => page.evaluate((i) => {
      document.documentElement.style.scrollBehavior = 'auto';
      document.getElementById(i).scrollIntoView();
    }, id);
    await jump('brokers');
    await current('#brokers');
    await jump('clouds');
    await current('#clouds');
    await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
    await current('#ci');
  } finally { await close(); }
});

test('without JavaScript every note and add link is visible', async () => {
  for (const path of ['/technologies', '/fr/technologies']) {
    const { page, close } = await open(path, { javaScriptEnabled: false });
    try {
      assert.equal(await page.locator('tr.det:visible').count(), ROWS);
      assert.equal(await page.locator('tr.det .note:visible').count(), await count(page, 'tr.det .note'));
      assert.ok(await count(page, 'tr.det .note') > 50);
      const add = await count(page, '.add a');
      assert.equal(add, 9);
      assert.equal(await page.locator('.add a:visible').count(), add);
      assert.equal(await page.locator('#grids .after:visible').count(), 1);
    } finally { await close(); }
  }
});

test('no horizontal scroll at 390px, grid table included', async () => {
  for (const javaScriptEnabled of [true, false]) {
    for (const path of ['/technologies', '/fr/technologies']) {
      const { page, close } = await open(path, { javaScriptEnabled, viewport: { width: 390, height: 844 } });
      try {
        if (javaScriptEnabled) for (const b of await page.locator('#grids tr.row button').all()) await b.click();
        const widths = await page.evaluate(() => ({
          page: document.documentElement.scrollWidth,
          grids: document.querySelector('#grids table').getBoundingClientRect().right,
          cells: Math.max(...[...document.querySelectorAll('#grids td, #grids th')].map((c) => c.getBoundingClientRect().right)),
        }));
        assert.ok(widths.page <= 390, `${path} js=${javaScriptEnabled}: page ${widths.page}`);
        assert.ok(widths.grids <= 390, `${path} js=${javaScriptEnabled}: grids table ${widths.grids}`);
        assert.ok(widths.cells <= 390, `${path} js=${javaScriptEnabled}: grids cell ${widths.cells}`);
        assert.equal(await page.isVisible('#grids td.x >> nth=0'), true, 'the country column stays');
      } finally { await close(); }
    }
  }
});

test('no third-party request is made', async () => {
  for (const path of ['/technologies', '/fr/technologies', '/technologies#production']) {
    const { page, external, close } = await open(path);
    try {
      await page.waitForLoadState('networkidle');
      await search(page, 'java');
      await page.locator('tr.row:visible button').first().click();
      assert.deepEqual(external, []);
    } finally { await close(); }
  }
});

test('the build refuses to write when a lab proof is missing', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'technologies-build-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const engine = join(root, 'engine');
  const lab = join(root, 'lab');
  const out = join(root, 'out');
  for (const d of [join(engine, 'site-data'), lab, out]) mkdirSync(d, { recursive: true });
  writeFileSync(join(engine, 'Cargo.toml'), '[workspace.package]\nversion = "9.9.9"\n');
  writeFileSync(join(engine, 'site-data', 'technologies.json'), JSON.stringify({
    teams: [],
    families: [{ id: 'ci', title: 'CI', row_header: 'Technology', extra_column: null, criteria: ['Proven'], lab_criterion: 0, after: null, add: [], rows: [{ id: 'x', name: 'X', criteria: [true], lab: ['scenarios/ci-missing'] }] }],
  }));
  writeFileSync(join(engine, 'site-data', 'technologies-fr.json'), '{}');
  writeFileSync(join(engine, 'site-data', 'references.json'), '[]');
  // The truncated lab: its Makefile, none of the cited scenarios.
  writeFileSync(join(lab, 'Makefile'), 'verify-all-scenarios:\n\t@for s in ci-missing; do echo $$s; done\n');
  const run = spawnSync(process.execPath, [join(SITE, 'scripts', 'build-technologies.mjs'), engine, lab, out], { encoding: 'utf8' });
  assert.equal(run.status, 1, run.stderr);
  assert.match(run.stderr, /scenarios\/ci-missing/);
  assert.deepEqual(readdirSync(out), []);
});

test('grid feed names stay on one line at 1280px', async () => {
  const { page, close } = await open('/fr/technologies', { viewport: { width: 1280, height: 900 } });
  try {
    const lines = await page.$$eval('#grids tr.row th .n', (els) => els.map((e) => {
      const r = e.getClientRects();
      return [e.textContent, r.length];
    }));
    for (const [name, n] of lines) assert.equal(n, 1, `${name} wraps on ${n} lines`);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth) <= 1280);
  } finally { await close(); }
});
