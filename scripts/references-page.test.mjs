// Page tests for /references and /fr/references: the committed pages on the local
// static server for what does not depend on the data (language, links, no-JS
// reading), and lists rendered in memory by referencesPage, empty or filled, served
// on a routed path. Never written into the engine's data. Locale pinned by page-test-helpers.mjs.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './static-server.mjs';
import { COPY } from '../assets/technologies/copy.js';
import { referencesPage } from './build-technologies.mjs';
import { CONTACT_EMAIL } from './site-chrome.mjs';
import { chromium, open as openPage } from './page-test-helpers.mjs';

const SITE = join(dirname(fileURLToPath(import.meta.url)), '..');
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
const open = (path, options = {}) => openPage(browser, server.url + (options.html ? '/__mem' : path), options);

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
  {
    id: 'exemple-transport', name: 'Exemple Transport', domain: 'exemple-transport.fr', since: '2026-08',
    request: 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/126',
    proofs: { report: { url: 'https://exemple-transport.fr/r.json', period_from: '2026-08-01', period_to: '2026-08-31', environments: [], signed: true } },
  },
  {
    // No environments key at all, not an empty list.
    id: 'exemple-sante', name: 'Exemple Santé', domain: 'exemple-sante.fr', since: '2026-08',
    request: 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/127',
    proofs: { report: { url: 'https://exemple-sante.fr/r.json', period_from: '2026-08-01', period_to: '2026-08-31', signed: true } },
  },
];
const LINKS = FILLED.flatMap((r) => [r.proofs.card?.verify_url, r.proofs.report?.url].filter(Boolean));
const TEAMS = [{ id: 't1', since: '2026-06' }];

test('an empty list shows the empty state and the request button', async () => {
  for (const [lang, path] of LANGS) {
    const { page, close } = await open(path);
    try {
      assert.equal(await page.getAttribute('html', 'lang'), lang);
      assert.equal(await page.getAttribute('#figurer a.btn', 'href'), ISSUE_FORM);
      assert.equal(await page.locator('#figurer a[href^="mailto:"]').count(), 1);
      assert.equal(await page.locator('#figurer ol.steps li').count(), 3);
      const proofs = await page.$$eval('dl.proofs dt', (dts) => dts.map((d) => d.textContent));
      assert.deepEqual(proofs.slice(2), [lang === 'fr' ? 'Équipe non nommée' : 'Unnamed team']);
      assert.equal(await page.locator('dl.proofs + .limit').count(), 1, 'the limit sentence follows the entries');
      const text = await page.textContent('main');
      assert.ok(!/sans preuve|no proof/i.test(text), 'an anonymous team sends a proof too');
    } finally { await close(); }
  }
  for (const [lang] of LANGS) {
    const { page, close } = await open(null, { html: referencesPage([], TEAMS, lang) });
    try {
      assert.equal(await page.locator('#liste table').count(), 0, 'no table without a row');
      assert.equal(await page.isVisible('#liste .empty'), true);
      assert.equal(await page.getAttribute('#liste .empty a', 'href'), '#figurer');
    } finally { await close(); }
  }
});

// What the owner approved, typed out here so that a slip in copy.js cannot pass for its own reference.
const MAIL = {
  fr: {
    subject: 'Perf Sentinel, équipe en production sans être nommée',
    body: [
      'Bonjour,',
      '',
      'Notre équipe utilise Perf Sentinel en production et souhaite être comptée sans être nommée.',
      '',
      'En production depuis : (mois et année)',
      'Notre pile : (langages et frameworks, bases de données, brokers, source de traces, cloud, intégration continue)',
      'Notre preuve : (lien de vérification d’une carte signée, ou adresse d’un rapport publié)',
    ],
  },
  en: {
    subject: 'Perf Sentinel, team in production without being named',
    body: [
      'Hello,',
      '',
      'Our team runs Perf Sentinel in production and would like to be counted without being named.',
      '',
      'In production since: (month and year)',
      'Our stack: (languages and frameworks, databases, brokers, trace source, cloud, continuous integration)',
      'Our proof: (verify link of a signed card, or address of a published report)',
    ],
  },
};

test('the anonymous route opens a prefilled message', async () => {
  for (const [lang, path] of LANGS) {
    const raw = await (await fetch(server.url + path)).text();
    const { page, close } = await open(path);
    try {
      const href = await page.getAttribute('#figurer a[href^="mailto:"]', 'href');
      const mail = new URL(href);
      assert.equal(mail.protocol, 'mailto:');
      assert.equal(mail.pathname, CONTACT_EMAIL, `${lang}: the address is the exported constant`);
      assert.deepEqual([...mail.searchParams.keys()], ['subject', 'body'], lang);
      const subject = mail.searchParams.get('subject');
      const body = mail.searchParams.get('body');
      assert.equal(subject, COPY[lang].ref.mailSubject, lang);
      assert.equal(body.replaceAll('\r\n', '\n'), COPY[lang].ref.mailBody, lang);
      assert.equal(subject, MAIL[lang].subject, lang);
      assert.equal(body, MAIL[lang].body.join('\r\n'), `${lang}: line breaks are CRLF`);
      assert.ok(href.includes('%0D%0A') && !/%0A/.test(href.replaceAll('%0D%0A', '')), `${lang}: line breaks are %0D%0A`);
      assert.ok(raw.includes('?subject=') && raw.includes('&amp;body='), `${lang}: the & between subject and body is written &amp; in the HTML`);
      // The sentence around the link does not change.
      assert.equal(await page.textContent('#figurer a[href^="mailto:"]'), lang === 'fr' ? 'Écrivez-nous par e-mail' : 'Write to us by e-mail');
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
      assert.deepEqual(links, LINKS);
      const details = await page.$$eval('#liste tr.det', (trs) => trs.map((tr) => tr.textContent));
      const [signed, hashed] = lang === 'fr' ? ['signé', 'empreinte vérifiée'] : ['signed', 'fingerprint verified'];
      assert.ok(details[0].includes(signed) && !details[0].includes(hashed), details[0]);
      assert.ok(details[2].includes(hashed), details[2]);
      assert.ok(details[0].includes(lang === 'fr' ? 'septembre 2026' : 'September 2026'), details[0]);
      assert.ok(details[1].includes(lang === 'fr' ? 'd’octobre 2026' : 'October 2026'), details[1]);
      assert.ok(!/undefined|environ/.test(details[3]), `no environments, no mention: ${details[3]}`);
      assert.ok(!/undefined|environ/.test(details[4]), `environments absent, no mention: ${details[4]}`);
      assert.ok(details[4].includes(lang === 'fr' ? 'août 2026' : 'August 2026'), details[4]);
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
      for (const sel of ['h1', '#liste h2', '#figurer a.btn', '#figurer a[href^="mailto:"]', 'dl.proofs', '.limit']) {
        assert.equal(await page.isVisible(sel), true, `${path}: ${sel}`);
      }
    } finally { await close(); }
  }
  for (const [lang] of LANGS) {
    const { page, close } = await open(null, { html: referencesPage(FILLED, TEAMS, lang), javaScriptEnabled: false });
    try {
      assert.equal(await page.locator('#liste tr.det:visible').count(), FILLED.length, 'every detail open');
      assert.equal(await page.locator('#liste tr.det a:visible').count(), LINKS.length);
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

test('a proof link that is not https stops the build, no page written', (t) => {
  const bad = (field, href) => {
    const r = structuredClone(FILLED[0]);
    if (field === 'verify_url') r.proofs.card.verify_url = href; else r.proofs.report.url = href;
    return r;
  };
  for (const [field, href] of [['verify_url', 'javascript:alert(1)'], ['url', 'data:text/html,x']]) {
    for (const [lang] of LANGS) {
      assert.throws(() => referencesPage([bad(field, href)], TEAMS, lang), (e) => e.message.includes('exemple-energie') && e.message.includes(field), `${field} ${lang}`);
    }
  }
  // A proof present without its link would render href="undefined".
  for (const [proof, field] of [['card', 'verify_url'], ['report', 'url']]) {
    const r = structuredClone(FILLED[0]);
    delete r.proofs[proof][field];
    for (const [lang] of LANGS) {
      assert.throws(() => referencesPage([r], TEAMS, lang), (e) => e.message.includes('exemple-energie') && e.message.includes(`proofs.${proof}.${field}`), `missing ${field} ${lang}`);
    }
  }
  const root = mkdtempSync(join(tmpdir(), 'references-build-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const [engine, lab, out] = ['engine', 'lab', 'out'].map((d) => join(root, d));
  for (const d of [join(engine, 'site-data'), lab, out]) mkdirSync(d, { recursive: true });
  writeFileSync(join(engine, 'Cargo.toml'), '[workspace.package]\nversion = "9.9.9"\n');
  writeFileSync(join(engine, 'site-data', 'technologies.json'), JSON.stringify({ teams: [], families: [] }));
  writeFileSync(join(engine, 'site-data', 'technologies-fr.json'), '{}');
  writeFileSync(join(engine, 'site-data', 'references.json'), JSON.stringify({ references: [bad('verify_url', 'javascript:alert(1)')] }));
  writeFileSync(join(lab, 'Makefile'), 'verify-all-scenarios:\n');
  const run = spawnSync(process.execPath, [join(SITE, 'scripts', 'build-technologies.mjs'), engine, lab, out], { encoding: 'utf8' });
  assert.notEqual(run.status, 0);
  assert.match(run.stderr, /exemple-energie.*verify_url/);
  assert.deepEqual(readdirSync(out), []);
});
