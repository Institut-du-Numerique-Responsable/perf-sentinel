// Links from the home page and the guide to /technologies and /references, in both
// languages, on the hydrated pages and in their prerendered copies. Local static
// server, third-party requests aborted, locale pinned.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { startServer } from './static-server.mjs';

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require('playwright')); }
catch { ({ chromium } = require('/Users/robintrassard/RustroverProjects/perf-sentinel/crates/sentinel-cli/tests/browser/node_modules/playwright')); }

const CONTRIB = 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/blob/main/CONTRIBUTING.md#what-to-contribute-by-difficulty';
const L = {
  en: {
    home: '/', guide: '/guide', refs: '/references', tech: '/technologies', ref: 'Reference', who: 'Who uses it', contrib: 'Contribute',
    gov: 'Who uses Perf Sentinel', stackGuide: '/docs/INSTRUMENTATION', stackQ: 'Is your technology missing from this grid?', stackL: 'See where it stands and how to add it',
    mat: 'See who uses it', pub: 'Get listed among the organisations that use it',
    qsLink: 'technologies table', qsSentence: 'The technologies table says where yours stands.',
    docsT: 'Technologies', docsD: 'What Perf Sentinel supports, technology by technology',
  },
  fr: {
    home: '/fr/', guide: '/fr/guide', refs: '/fr/references', tech: '/fr/technologies', ref: 'Référence', who: 'Qui l’utilise', contrib: 'Contribuer',
    gov: 'Qui utilise Perf Sentinel', stackGuide: '/docs/fr/INSTRUMENTATION', stackQ: 'Votre technologie n’est pas dans cette grille ?', stackL: 'Voir où elle en est et comment l’ajouter',
    mat: 'Voir qui l’utilise', pub: 'Figurer parmi les organisations qui l’utilisent',
    qsLink: 'tableau des technologies', qsSentence: 'Le tableau des technologies dit où en est la vôtre.',
    docsT: 'Technologies', docsD: 'Ce que Perf Sentinel prend en charge, technologie par technologie',
  },
};
const LANGS = Object.entries(L);

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

async function open(path, { viewport = { width: 1280, height: 900 }, depth } = {}) {
  const context = await browser.newContext({ viewport, locale: 'en-GB' });
  // Label of a link without its arrow, for the in-page checks.
  await context.addInitScript(() => { window.__label = (a) => a.textContent.replace(/[→↗▸]/g, '').replace(/\s+/g, ' ').trim(); });
  if (depth) await context.addInitScript((d) => { try { localStorage.setItem('ps-depth', d); } catch { /* default depth then */ } }, depth);
  await context.route('**/*', (route) => (new URL(route.request().url()).hostname === '127.0.0.1' ? route.fallback() : route.abort()));
  context.setDefaultTimeout(10000);
  const page = await context.newPage();
  await page.goto(server.url + path);
  await page.waitForSelector('#dc-root header');
  return { page, close: () => context.close() };
}


test('the home header links to the references page and to the contribution guide', async () => {
  for (const [, t] of LANGS) {
    const { page, close } = await open(t.home);
    try {
      const nav = await page.$$eval('#dc-root nav[data-nav] a', (as) => as.map((a) => ({ text: window.__label(a), href: a.getAttribute('href'), target: a.getAttribute('target'), arrow: a.querySelector('[data-arrow]')?.textContent.trim() })));
      const i = nav.findIndex((a) => a.text === t.ref);
      assert.ok(i >= 0, `${t.home}: no ${t.ref} in the nav`);
      assert.deepEqual(nav.slice(i + 1), [
        { text: t.who, href: t.refs, target: null, arrow: '→' },
        { text: t.contrib, href: CONTRIB, target: '_blank', arrow: '↗' },
      ], `${t.home}: desktop nav`);
      const mob = await page.$$eval('#dc-root nav[data-mobnav] a', (as) => as.map((a) => ({ text: window.__label(a), href: a.getAttribute('href'), ext: a.textContent.includes('↗') })));
      const j = mob.findIndex((a) => a.text === t.ref);
      assert.ok(j >= 0, `${t.home}: no ${t.ref} in the mobile menu`);
      assert.deepEqual(mob.slice(j + 1), [
        { text: t.who, href: t.refs, ext: false },
        { text: t.contrib, href: CONTRIB, ext: true },
      ], `${t.home}: mobile menu`);
    } finally { await close(); }
    const m = await open(t.home, { viewport: { width: 390, height: 844 } });
    try {
      await m.page.click('#dc-root [data-burger]');
      assert.ok(await m.page.locator(`#dc-root nav[data-mobnav] a[href="${t.refs}"]`).isVisible(), `${t.home}: who-uses-it in the open menu`);
      assert.ok(await m.page.locator(`#dc-root nav[data-mobnav] a[href="${CONTRIB}"]`).isVisible(), `${t.home}: contribute in the open menu`);
    } finally { await m.close(); }
  }
});

test('the contribution entry is for the technical audience only', async () => {
  for (const [, t] of LANGS) {
    const { page, close } = await open(t.home + '?profile=exec');
    try {
      assert.equal(await page.locator(`#dc-root nav[data-nav] a[href="${CONTRIB}"]`).isVisible(), false, `${t.home}: contribute hidden in the nav`);
      assert.ok(await page.locator(`#dc-root nav[data-nav] a[href="${t.refs}"]`).isVisible(), `${t.home}: who-uses-it visible in the nav`);
      assert.ok(await page.locator(`#dc-root #governance a[href="${t.refs}"]`).isVisible(), `${t.home}: who-uses-it visible in #governance`);
    } finally { await close(); }
    const m = await open(t.home + '?profile=exec', { viewport: { width: 390, height: 844 } });
    try {
      await m.page.click('#dc-root [data-burger]');
      assert.equal(await m.page.locator(`#dc-root nav[data-mobnav] a[href="${CONTRIB}"]`).isVisible(), false, `${t.home}: contribute hidden in the menu`);
      assert.ok(await m.page.locator(`#dc-root nav[data-mobnav] a[href="${t.refs}"]`).isVisible(), `${t.home}: who-uses-it visible in the menu`);
    } finally { await m.close(); }
  }
});

test('the home page links to the two pages from its sections', async () => {
  for (const [, t] of LANGS) {
    for (const depth of ['ess', 'det']) {
      const { page, close } = await open(t.home, { depth });
      try {
        const got = await page.evaluate((t) => {
          const R = document.querySelector('#dc-root');
          const link = (a) => a && { text: window.__label(a), href: a.getAttribute('href') };
          const stack = R.querySelector('#stack');
          const guide = stack.querySelector(':scope > a');
          const line = guide.nextElementSibling;
          const mat = [...R.querySelectorAll('p')].find((p) => p.querySelector(`a[href="${t.refs}"]`) && p.querySelector('strong'));
          const pub = R.querySelector(`#publier [data-links] a[href="${t.refs}#figurer"]`);
          const gov = R.querySelector('#governance [data-links] a');
          return {
            guide: link(guide),
            question: line?.firstElementChild?.textContent.trim(),
            lineLink: link(line?.querySelector('a')),
            mat: mat && { link: link(mat.querySelector('a')), end: mat.textContent.trim().slice(-(t.mat.length + 1)) },
            pub: link(pub), pubShown: !!pub && pub.getBoundingClientRect().height > 0,
            gov: link(gov),
          };
        }, t);
        const where = `${t.home} ${depth}`;
        assert.equal(got.guide.href, t.stackGuide, `${where}: #stack guide link first`);
        assert.equal(got.question, t.stackQ, `${where}: #stack question`);
        assert.deepEqual(got.lineLink, { text: t.stackL, href: t.tech }, `${where}: #stack link`);
        assert.deepEqual(got.mat?.link, { text: t.mat, href: t.refs }, `${where}: maturity link`);
        assert.equal(got.mat.end, t.mat + '.', `${where}: maturity line ends with the link and a full stop`);
        assert.deepEqual(got.pub, { text: t.pub, href: t.refs + '#figurer' }, `${where}: #publier link`);
        assert.ok(got.pubShown, `${where}: #publier link shown`);
        assert.deepEqual(got.gov, { text: t.gov, href: t.refs }, `${where}: #governance first link`);
      } finally { await close(); }
    }
  }
});

test('the guide links to the technologies page', async () => {
  for (const [, t] of LANGS) {
    const { page, close } = await open(t.guide);
    try {
      const got = await page.evaluate((t) => {
        const R = document.querySelector('#dc-root');
        const intro = R.querySelector('#quickstart > p');
        const a = intro.querySelector(`a[href="${t.tech}"]`);
        const first = R.querySelector('#docs > div > a');
        return { end: intro.textContent.trim().slice(-t.qsSentence.length), link: a && window.__label(a), first: { href: first.getAttribute('href'), text: first.textContent.replace(/\s+/g, ' ').trim() } };
      }, t);
      assert.equal(got.end, t.qsSentence, `${t.guide}: #quickstart sentence`);
      assert.equal(got.link, t.qsLink, `${t.guide}: #quickstart link`);
      assert.deepEqual(got.first, { href: t.tech, text: `${t.docsT} → ${t.docsD}` }, `${t.guide}: first #docs entry`);
    } finally { await close(); }
  }
});

test('five header links stay on one line at 930px', async () => {
  for (const [, t] of LANGS) {
    const { page, close } = await open(t.home, { viewport: { width: 930, height: 900 } });
    try {
      const m = await page.evaluate(() => {
        const nav = document.querySelector('#dc-root nav[data-nav]');
        const as = [...nav.querySelectorAll('a')].map((a) => a.getBoundingClientRect());
        const right = nav.parentElement.querySelector('div[style*="margin-left"]').getBoundingClientRect();
        return { n: as.length, tops: new Set(as.map((r) => Math.round(r.top))).size, tallest: Math.max(...as.map((r) => r.height)), lastRight: as.at(-1).right, rightLeft: right.left, scrollW: document.documentElement.scrollWidth, clientW: document.documentElement.clientWidth };
      });
      assert.equal(m.n, 5, `${t.home}: five links`);
      assert.equal(m.tops, 1, `${t.home}: one line`);
      assert.ok(m.tallest < 30, `${t.home}: no link wraps (${m.tallest}px)`);
      assert.ok(m.lastRight < m.rightLeft, `${t.home}: nav clear of the buttons`);
      assert.ok(m.scrollW <= m.clientW, `${t.home}: no horizontal scroll (${m.scrollW} > ${m.clientW})`);
    } finally { await close(); }
  }
});

test('no horizontal scroll at 390px on the home page and the guide', async () => {
  for (const [, t] of LANGS) {
    for (const path of [t.home, t.guide]) {
      const { page, close } = await open(path, { viewport: { width: 390, height: 844 } });
      try {
        const [sw, cw] = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]);
        assert.ok(sw <= cw, `${path}: ${sw} > ${cw}`);
      } finally { await close(); }
    }
  }
});

test('the prerendered copies carry the new links', async () => {
  for (const [, t] of LANGS) {
    const raw = async (path) => {
      const html = await (await fetch(server.url + path)).text();
      return html.slice(html.indexOf('<div id="seo-prerender"'), html.indexOf('<x-dc'));
    };
    const home = await raw(t.home);
    for (const s of [`href="${t.refs}"`, `href="${t.refs}#figurer"`, `href="${t.tech}"`, `href="${CONTRIB}"`, t.who, t.contrib, t.gov, t.stackQ, t.stackL, t.mat, t.pub]) {
      assert.ok(home.includes(s), `${t.home} prerender: ${s}`);
    }
    const guide = await raw(t.guide);
    for (const s of [`href="${t.tech}"`, t.qsLink, t.docsD]) {
      assert.ok(guide.includes(s), `${t.guide} prerender: ${s}`);
    }
  }
});
