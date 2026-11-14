// Page tests for /references and /fr/references: the committed pages (built from
// the engine's data, empty today) on the local static server, and a filled list
// rendered in memory by referencesPage and served on a routed path. Never written
// into the engine's data. Locale pinned: the English page redirects on navigator.language.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { startServer } from './static-server.mjs';
import { referencesPage } from './build-technologies.mjs';
import { CONTACT_EMAIL } from './site-chrome.mjs';

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require('playwright')); }
catch { ({ chromium } = require('/Users/robintrassard/RustroverProjects/perf-sentinel/crates/sentinel-cli/tests/browser/node_modules/playwright')); }

const ISSUE_FORM = 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/new?template=reference.yml';

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

// html: a page served at /__mem instead of a committed file.
async function open(path, { javaScriptEnabled = true, viewport = { width: 1280, height: 900 }, theme, html } = {}) {
  const context = await browser.newContext({ javaScriptEnabled, viewport, locale: 'en-GB' });
  if (theme) await context.addInitScript((t) => { try { localStorage.setItem('ps-theme', t); } catch { /* the page then follows the system */ } }, theme);
  const external = [];
  await context.route('**/*', (route) => {
    const url = new URL(route.request().url());
    if (url.hostname === '127.0.0.1') return route.fallback();
    external.push(url.href);
    return route.abort();
  });
  if (html) await context.route(`${server.url}/__mem`, (route) => route.fulfill({ body: html, contentType: 'text/html' }));
  context.setDefaultTimeout(10000);
  const page = await context.newPage();
  const dialogs = [];
  page.on('dialog', (d) => { dialogs.push(d.message()); d.dismiss(); });
  await page.goto(server.url + (html ? '/__mem' : path));
  return { page, external, dialogs, close: () => context.close() };
}

const LANGS = [['en', '/references'], ['fr', '/fr/references']];

const FILLED = [
  {
    id: 'exemple-energie', name: 'Exemple Énergie', domain: 'exemple-energie.fr', since: '2026-07',
    request: 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/123',
    proofs: {
      card: { verify_url: 'https://perf-sentinel.dev/verifier#c=AAAA', period_from: '2026-09-01', period_to: '2026-09-30' },
      report: { url: 'https://www.exemple-energie.fr/perf-sentinel/rapport.json', period_from: '2026-07-01', period_to: '2026-12-31', environments: ['production'], signed: true },
    },
  },
  {
    id: 'exemple-logistique', name: 'Exemple Logistique', domain: 'exemple-logistique.eu', since: '2026-10',
    request: 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/124',
    proofs: { card: { verify_url: 'https://perf-sentinel.dev/fr/verifier#c=BBBB', period_from: '2026-10-01', period_to: '2026-10-31' } },
  },
  {
    id: 'exemple-mutuelle', name: 'Exemple <img src=x onerror=alert(1)> Mutuelle', domain: 'exemple-mutuelle.fr', since: '2026-09',
    request: 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/125',
    proofs: { report: { url: 'https://exemple-mutuelle.fr/r.json?a=1&b="2"', period_from: '2026-09-01', period_to: '2026-09-30', environments: ['production', 'staging'], signed: false } },
  },
];
const TEAMS = [{ id: 't1', since: '2026-06' }];

test('an empty list shows the empty state and the request button', async () => {
  for (const [lang, path] of LANGS) {
    const { page, close } = await open(path);
    try {
      assert.equal(await page.getAttribute('html', 'lang'), lang);
      assert.equal(await page.locator('#liste table').count(), 0, 'no table without a row');
      assert.equal(await page.isVisible('#liste .empty'), true);
      assert.equal(await page.getAttribute('#liste .empty a', 'href'), '#figurer');
      assert.equal(await page.getAttribute('#figurer a.btn', 'href'), ISSUE_FORM);
      assert.equal(await page.getAttribute('#figurer a[href^="mailto:"]', 'href'), `mailto:${CONTACT_EMAIL}`);
      assert.equal(await page.locator('#figurer ol.steps li').count(), 3);
      assert.equal(await page.locator('dl.proofs dt').count(), 2);
    } finally { await close(); }
  }
});

test('a listed organisation shows its proofs and verify links', async () => {
  for (const [lang] of LANGS) {
    const { page, dialogs, close } = await open(null, { html: referencesPage(FILLED, TEAMS, lang), javaScriptEnabled: false });
    try {
      assert.equal(await page.locator('#liste .empty').count(), 0);
      const rows = await page.$$eval('#liste tr.row', (trs) => trs.map((tr) => ({
        name: tr.querySelector('th').textContent,
        ticks: [...tr.querySelectorAll('td.c .mk')].map((s) => s.classList.contains('y')),
      })));
      assert.equal(rows.length, FILLED.length);
      assert.deepEqual(rows.map((r) => r.ticks), FILLED.map((r) => [!!r.proofs.card, !!r.proofs.report]));
      assert.ok(rows[0].name.includes('exemple-energie.fr'));
      assert.ok(rows[2].name.includes('<img src=x onerror=alert(1)>'), 'markup in the data reads as text');
      assert.equal(await page.locator('#liste img').count(), 0);
      const links = await page.$$eval('#liste tr.det a', (as) => as.map((a) => a.getAttribute('href')));
      const expected = FILLED.flatMap((r) => [r.proofs.card?.verify_url, r.proofs.report?.url].filter(Boolean));
      assert.deepEqual(links, expected);
      const details = await page.$$eval('#liste tr.det', (trs) => trs.map((tr) => tr.textContent));
      const [signed, hashed] = lang === 'fr' ? ['signé', 'empreinte vérifiée'] : ['signed', 'fingerprint verified'];
      assert.ok(details[0].includes(signed) && !details[0].includes(hashed), details[0]);
      assert.ok(details[2].includes(hashed), details[2]);
      assert.ok(details[0].includes(lang === 'fr' ? 'septembre 2026' : 'September 2026'), details[0]);
      assert.ok(details[1].includes(lang === 'fr' ? 'd’octobre 2026' : 'October 2026'), details[1]);
      assert.deepEqual(dialogs, []);
    } finally { await close(); }
  }
});

test('the anonymous line counts teams and links to the production filter', async () => {
  const cases = [
    ['en', [{ id: 'a', since: '2026-06' }, { id: 'b', since: '2025-11' }], ['2 teams', 'November 2025']],
    ['fr', [{ id: 'a', since: '2026-06' }, { id: 'b', since: '2025-11' }], ['2 équipes', 'novembre 2025']],
    ['fr', TEAMS, ['Une équipe', 'juin 2026']],
  ];
  for (const [lang, teams, words] of cases) {
    const { page, close } = await open(null, { html: referencesPage([], teams, lang) });
    try {
      const text = await page.textContent('#liste .anon');
      for (const w of words) assert.ok(text.includes(w), `${w} in ${text}`);
      assert.equal(await page.getAttribute('#liste .anon a', 'href'), lang === 'fr' ? '/fr/technologies#production' : '/technologies#production');
      assert.equal(await page.locator('#liste .anon svg.lv.pr').count(), 1, 'the ringed sign');
    } finally { await close(); }
  }
  const { page, close } = await open(null, { html: referencesPage([], [], 'en') });
  try { assert.equal(await page.locator('#liste .anon').count(), 0, 'no line without a team'); }
  finally { await close(); }
});

test('the page reads without JavaScript', async () => {
  for (const [, path] of LANGS) {
    const { page, close } = await open(path, { javaScriptEnabled: false });
    try {
      for (const sel of ['h1', '#liste .empty', '#liste .anon a', '#figurer a.btn', '#figurer a[href^="mailto:"]', 'dl.proofs', '.limit']) {
        assert.equal(await page.isVisible(sel), true, `${path}: ${sel}`);
      }
    } finally { await close(); }
  }
  for (const [lang] of LANGS) {
    const { page, close } = await open(null, { html: referencesPage(FILLED, TEAMS, lang), javaScriptEnabled: false });
    try {
      assert.equal(await page.locator('#liste tr.det:visible').count(), FILLED.length, 'every detail open');
      assert.equal(await page.locator('#liste tr.det a:visible').count(), 4);
    } finally { await close(); }
  }
});

test('no horizontal scroll at 390px', async () => {
  const pages = [...LANGS.map(([, path]) => [path, null]), ...LANGS.map(([lang]) => [`filled ${lang}`, referencesPage(FILLED, TEAMS, lang)])];
  for (const [name, html] of pages) {
    const { page, close } = await open(html ? null : name, { html, viewport: { width: 390, height: 844 } });
    try {
      const width = await page.evaluate(() => document.documentElement.scrollWidth);
      assert.ok(width <= 390, `${name}: ${width}`);
    } finally { await close(); }
  }
});

