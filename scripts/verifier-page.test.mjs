// Page tests for /verifier and /fr/verifier: a local static server and Playwright
// (resolved like build-prerender.mjs), every third-party request aborted and recorded.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { dirname, extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { COPY } from '../assets/verifier/copy.js';
import { readPngCard } from '../assets/verifier/files.js';
import {
  tokenOf, flip, concat, stripChunk, jxlParts, jxlBox, pngChunks, replaceChunk, cardChunk, reorderEnvelope, spaceEnvelope,
} from './verifier-test-helpers.mjs';

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require('playwright')); }
catch { ({ chromium } = require('/Users/robintrassard/RustroverProjects/perf-sentinel/crates/sentinel-cli/tests/browser/node_modules/playwright')); }

const SITE = join(dirname(fileURLToPath(import.meta.url)), '..');
const FIXTURES = join(SITE, 'scripts', 'fixtures', 'verifier');
const MIME = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.json': 'application/json', '.png': 'image/png', '.gif': 'image/gif', '.ico': 'image/x-icon', '.woff2': 'font/woff2', '.woff': 'font/woff', '.cast': 'text/plain', '.xml': 'application/xml', '.txt': 'text/plain' };

export function startServer() {
  const server = createServer(async (req, res) => {
    let p = normalize(decodeURIComponent(req.url.split('?')[0]));
    if (p.endsWith('/')) p += 'index.html';
    let fp = join(SITE, p);
    if (existsSync(fp) && statSync(fp).isDirectory()) fp = join(fp, 'index.html');
    if (!existsSync(fp) && !extname(fp) && existsSync(fp + '.html')) fp += '.html';
    if (!fp.startsWith(SITE) || !existsSync(fp)) { res.writeHead(404); res.end('not found'); return; }
    res.writeHead(200, { 'Content-Type': MIME[extname(fp)] || 'application/octet-stream', 'Cache-Control': 'no-store' });
    res.end(await readFile(fp));
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({
    url: `http://127.0.0.1:${server.address().port}`,
    close: () => server.close(),
  })));
}

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

export async function open(path, { locale = 'en-GB', init } = {}) {
  const context = await browser.newContext({ locale });
  const external = [];
  await context.route('**/*', (route) => {
    const url = new URL(route.request().url());
    if (url.hostname === '127.0.0.1') return route.fallback();
    external.push(url.href);
    return route.abort();
  });
  if (init) await context.addInitScript(init);
  context.setDefaultTimeout(10000);
  const page = await context.newPage();
  await page.goto(server.url + path);
  return { page, external, close: () => context.close() };
}

const fx = JSON.parse(readFileSync(join(FIXTURES, 'fixtures.json'), 'utf8'));
const fixture = (name) => new Uint8Array(readFileSync(join(FIXTURES, name)));
const token = (angle) => tokenOf(fx.cards[angle].compact_url);
const link = (angle) => '/verifier#c=' + token(angle);
const enc = (s) => new TextEncoder().encode(s);
const dec = (b) => new TextDecoder().decode(b);

async function waitState(page, state) {
  try {
    await page.waitForFunction((s) => document.querySelector('#verifier-result')?.dataset.state === s, state, { timeout: 8000 });
  } catch {
    throw new Error(`state ${await page.getAttribute('#verifier-result', 'data-state')}, expected ${state}`);
  }
}

const choose = (page, name, bytes, mimeType = 'application/octet-stream') =>
  page.setInputFiles('#verifier-file', { name, mimeType, buffer: Buffer.from(bytes) });

async function drop(page, name, bytes) {
  const data = await page.evaluateHandle(({ name, b64 }) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], name));
    return dt;
  }, { name, b64: Buffer.from(bytes).toString('base64') });
  for (const type of ['dragenter', 'dragover', 'drop']) await page.dispatchEvent('#verifier-drop', type, { dataTransfer: data });
}

/** A file opened on a fresh /verifier page, chosen or dropped. */
async function openFile(name, bytes, { how = 'choose', path = '/verifier', init } = {}) {
  const opened = await open(path, { init });
  await (how === 'drop' ? drop(opened.page, name, bytes) : choose(opened.page, name, bytes));
  return opened;
}

const text = (page, sel) => page.locator(sel).first().textContent();
// A signed figure shows once, in #verifier-figures; every other value in the details list.
const cell = (path, part) => `[data-field="${path}"] ${part}, [data-figure="${path.replace(/^figures\./, '')}"] ${part}`;
const label = (page, path) => text(page, cell(path, 'dt'));
const value = (page, path) => text(page, cell(path, 'dd'));
const visible = (page, sel) => page.locator(sel).first().isVisible();
const pageText = (page) => page.evaluate(() => document.body.innerText);

/** Elements of #verifier-result painted in the accent, the tokens resolved through a probe. */
const accentHits = (page) => page.evaluate(() => {
  const root = document.querySelector('[data-ps-root]');
  const probe = document.createElement('span');
  root.appendChild(probe);
  // Background, not color: the legal pages force inline `color:var(--accent)` to the strong accent in light.
  const resolve = (v) => {
    probe.style.backgroundColor = v;
    return getComputedStyle(probe).backgroundColor;
  };
  const accents = [resolve('var(--accent)'), resolve('var(--accent-strong)')];
  probe.remove();
  const result = document.querySelector('#verifier-result');
  const props = ['color', 'background-color', 'border-top-color', 'border-right-color', 'border-bottom-color', 'border-left-color'];
  const hits = [];
  for (const el of [result, ...result.querySelectorAll('*')]) {
    const cs = getComputedStyle(el);
    for (const p of props) if (accents.includes(cs.getPropertyValue(p))) hits.push(`${el.tagName}.${el.className} ${p}`);
  }
  return { accents, hits };
});

async function assertNoAccent(page) {
  for (const theme of ['light', 'dark']) {
    await page.evaluate((t) => document.querySelector('[data-ps-root]').setAttribute('data-theme', t), theme);
    const { accents, hits } = await accentHits(page);
    assert.ok(accents.every((c) => c.startsWith('rgb')) && accents[0] !== accents[1], `accent resolved: ${accents}`);
    assert.deepEqual(hits, [], theme);
  }
}

test('the empty page explains both ways in', async () => {
  const { page, external, close } = await open('/verifier');
  await waitState(page, 'empty');
  assert.equal(await visible(page, '#verifier-drop'), true);
  assert.equal(await page.getAttribute('#verifier-drop', 'data-compact'), null);
  assert.equal(await visible(page, '#verifier-drop small'), true);
  assert.equal(await text(page, '#verifier-drop small'), COPY.en.dropSub);
  assert.equal(await visible(page, '#verifier-phone'), true);
  assert.equal(await text(page, '#verifier-phone'), COPY.en.phone);
  assert.deepEqual(external, []);
  await close();
});

test('a loaded card comes before the drop zone', async () => {
  const { page, close } = await open(link('story'));
  await waitState(page, 'level1');
  const before = await page.evaluate(() => Boolean(document.querySelector('#verifier-result')
    .compareDocumentPosition(document.querySelector('#verifier-drop')) & Node.DOCUMENT_POSITION_FOLLOWING));
  assert.equal(before, true);
  assert.equal(await page.getAttribute('#verifier-drop', 'data-compact'), '');
  assert.equal((await pageText(page)).includes(COPY.en.phone), false);
  await close();
});

test('a valid link shows the neutral level 1 state', async () => {
  const { page, external, close } = await open(link('story'));
  await waitState(page, 'level1');
  assert.equal(await text(page, '#verifier-headline'), COPY.en.level1);
  const spans = await page.$$eval('#verifier-headline span', (els) => els.map((e) => {
    const cs = getComputedStyle(e);
    return [cs.fontSize, cs.fontWeight, cs.color].join();
  }));
  assert.equal(spans.length, 2);
  assert.equal(spans[0], spans[1]);
  assert.equal(await page.locator('#verifier-result [data-ico]').count(), 0);
  assert.ok((await value(page, 'org.domain')).includes('cards.example.org'));
  assert.ok((await value(page, 'figures.before_fix_ops')).includes('1.9 million'));
  assert.equal((await pageText(page)).includes('1,937,000'), false);
  assert.ok((await value(page, 'before_fix_period')).includes('July 2026'));
  assert.equal(await label(page, 'before_fix_period'), 'Period before the fix');
  assert.equal(await visible(page, '#verifier-compact-note'), true);
  assert.ok((await text(page, '#verifier-compact-note')).includes('environment'));
  assert.equal(await visible(page, '#verifier-dns'), true);
  await assertNoAccent(page);
  assert.deepEqual(external, []);
  await close();
});

/** Text written straight into #verifier-result: every state wraps its words in elements. */
const strayText = (page) => page.$$eval('#verifier-result', ([r]) =>
  [...r.childNodes].filter((n) => n.nodeType === Node.TEXT_NODE).map((n) => n.textContent));

test('a card without a domain shows no domain check', async () => {
  let { page, close } = await open(link('nodomain'));
  await waitState(page, 'level1');
  assert.equal(await page.locator('[data-field="org.domain"]').count(), 0);
  assert.equal(await visible(page, '#verifier-dns'), false);
  assert.equal(await page.locator('#verifier-org').count(), 0);
  assert.deepEqual(await strayText(page), []);
  await close();

  ({ page, close } = await openFile('nodomain.png', fixture('nodomain.png')));
  await waitState(page, 'level1');
  assert.equal(await text(page, '#verifier-org'), COPY.en.level1OrgOnly.replace('{org}', 'Example Org'));
  assert.equal(await visible(page, '#verifier-dns'), false);
  assert.equal((await text(page, '#verifier-result')).includes('()'), false);
  await close();
});

test('every angle renders', async () => {
  let { page, close } = await open(link('curve'));
  await waitState(page, 'level1');
  assert.ok(await page.locator('[data-figure]').count() >= 3);
  assert.ok((await label(page, 'figures.previous_waste_permille')).includes('August 2026'));
  assert.equal(await value(page, 'figures.previous_waste_permille'), '4.8%');
  await close();

  ({ page, close } = await open(link('collective')));
  await waitState(page, 'level1');
  assert.ok(await page.locator('[data-figure]').count() >= 3);
  assert.ok((await value(page, 'before_fix_period')).includes('August 2026'));
  await close();
});

test('a slow story shows its period and an alert threshold, not a measured duration', async () => {
  let { page, close } = await open(link('slow'));
  await waitState(page, 'level1');
  assert.equal(await label(page, 'figures.cited_ms'), 'Headline figure on the card');
  assert.equal(await label(page, 'figures.threshold_ms'), 'Alert threshold the card cites');
  assert.ok((await value(page, 'before_fix_period')).includes('August 2026'));
  await close();

  ({ page, close } = await openFile('slow.png', fixture('slow.png')));
  await waitState(page, 'level1');
  assert.equal(await label(page, 'figures.cited_ms'), 'Slowest runs before the fix, measured');
  assert.ok((await value(page, 'figures.cited_ms')).includes('2.3 s or more'));
  assert.ok((await label(page, 'figures.threshold_ms')).startsWith('Alert threshold'));
  assert.equal(await label(page, 'story.done_at'), 'Fix declared on');
  assert.equal((await pageText(page)).includes('No longer detected since'), false);
  await close();

  ({ page, close } = await open('/fr' + link('slow'), { locale: 'fr-FR' }));
  await waitState(page, 'level1');
  assert.ok((await label(page, 'figures.threshold_ms')).startsWith('Seuil d’alerte'));
  assert.ok((await value(page, 'before_fix_period')).includes('août 2026'));
  await close();

  ({ page, close } = await openFile('slow.png', fixture('slow.png'), { path: '/fr/verifier' }));
  await waitState(page, 'level1');
  assert.ok((await label(page, 'figures.threshold_ms')).startsWith('Seuil d’alerte'));
  assert.ok((await value(page, 'figures.cited_ms')).includes('2,3 s ou plus'));
  assert.equal(await label(page, 'story.done_at'), 'Correction déclarée le');
  await close();
});

test('one changed character reads as tampered', async () => {
  const t = token('story');
  const { page, close } = await open('/verifier#c=' + flip(t, 10));
  await waitState(page, 'tampered');
  assert.equal(await text(page, '#verifier-headline'), COPY.en.tampered);
  assert.equal(await text(page, '#verifier-next'), COPY.en.tamperedNext);
  assert.ok(await page.locator('#verifier-result [data-open-limits]').count() > 0);
  assert.equal(await page.locator('[data-field]').count(), 0);
  await close();
});

test('a cut link reads as damaged', async () => {
  const { page, close } = await open('/verifier#c=' + token('story').slice(0, -10));
  await waitState(page, 'invalid-link');
  assert.equal(await text(page, '#verifier-headline'), COPY.en.invalidLink);
  assert.ok(await page.locator('#verifier-result [data-open-limits]').count() > 0);
  await close();
});

test('the original PNG verifies with its free fields as text', async () => {
  const { page, close } = await openFile('story.png', fixture('story.png'));
  await waitState(page, 'level1');
  assert.ok((await value(page, 'free.title')).includes('<b>Q3</b> & "review"'));
  assert.equal(await label(page, 'free.title'), COPY.en.freeChosenTitle);
  assert.equal(await page.locator('#verifier-result b').count(), 0);
  assert.equal(await page.locator('#verifier-compact-note').count(), 0);
  assert.ok((await value(page, 'org.name')).includes('Example Org'));
  assert.equal(await value(page, 'equivalence.unit'), 'at least 5 average meals, in July 2026');
  assert.equal(await value(page, 'equivalence.per_unit_ug'), '1 average meal = 2.04 kg CO₂e');
  assert.equal(await label(page, 'story.observed_at'), 'Seen on');
  assert.equal(await value(page, 'story.observed_at'), '20 August 2026');
  assert.equal(await text(page, '#verifier-org'), COPY.en.level1Org.replace('{org}', 'Example Org').replace('{domain}', 'cards.example.org'));
  await close();
});

test('the original PDF verifies', async () => {
  const { page, close } = await openFile('story.pdf', fixture('story.pdf'));
  await waitState(page, 'level1');
  assert.ok((await value(page, 'key')).includes(fx.fingerprint));
  await close();
});

test('the original SVG verifies', async () => {
  for (const how of ['choose', 'drop']) {
    const { page, close } = await openFile('story.svg', fixture('story.svg'), { how });
    await waitState(page, 'level1');
    assert.ok((await value(page, 'free.title')).includes('<b>Q3</b> & "review"'), how);
    assert.ok((await value(page, 'key')).includes(fx.fingerprint), how);
    await close();
  }
});

const COUNT_BLOBS = `window.__pwned = 0; window.__blobs = 0;
  { const create = URL.createObjectURL; URL.createObjectURL = function (...a) { window.__blobs += 1; return create.apply(this, a); }; }`;

test('a dropped SVG is never rendered', async () => {
  const svg = dec(fixture('story.svg'))
    .replace('<svg ', '<svg onload="window.__pwned=2" ')
    .replace('</svg>', '<script>window.__pwned=1</script><image href="https://evil.example/x.png"/></svg>');
  assert.ok(svg.includes('onload=') && svg.includes('evil.example'));
  let { page, external, close } = await openFile('card.svg', enc(svg), { init: COUNT_BLOBS });
  // The envelope still reads, the added script makes the file content modified.
  await waitState(page, 'modified');
  await page.waitForTimeout(200);
  assert.equal(await page.evaluate(() => window.__pwned), 0);
  const html = await page.evaluate(() => document.documentElement.outerHTML);
  assert.equal(html.includes('__pwned'), false);
  assert.equal(html.includes('evil.example'), false);
  assert.equal(await page.evaluate(() => window.__blobs), 0);
  assert.deepEqual(external, []);
  await close();

  const bare = svg.replace(/<metadata id="perf-sentinel-card">[\s\S]*?<\/metadata>/, '');
  ({ page, close } = await openFile('card.svg', enc(bare)));
  await waitState(page, 'no-card-data');
  await close();
});

test('the original JXL verifies', async () => {
  for (const how of ['choose', 'drop']) {
    const { page, close } = await openFile('story.jxl', fixture('story.jxl'), { how, init: COUNT_BLOBS });
    await waitState(page, 'level1');
    assert.ok((await value(page, 'key')).includes(fx.fingerprint), how);
    assert.equal(await page.evaluate(() => window.__blobs), 0);
    assert.equal(await page.locator('#verifier-result img').count(), 0);
    await close();
  }
  const parts = jxlParts(fixture('story.jxl'));
  const noBox = concat(parts['JXL '], parts.ftyp, parts.jxlc);
  const bare = parts.jxlc.subarray(8);
  assert.deepEqual([...bare.subarray(0, 2)], [0xff, 0x0a]);
  for (const [name, bytes] of [['story.jxl', noBox], ['card.jxl', bare]]) {
    const { page, close } = await openFile(name, bytes);
    await waitState(page, 'no-card-data');
    assert.equal(await visible(page, '#verifier-result .vf-phone'), true, name);
    await close();
  }
});

test('a dropped file uses the same path as the picker', async () => {
  const { page, close } = await openFile('story.png', fixture('story.png'), { how: 'drop' });
  await waitState(page, 'level1');
  await close();
});

test('a re-shared PNG explains the metadata loss', async () => {
  const { page, close } = await openFile('story.png', stripChunk(fixture('story.png'), 'iTXt'));
  await waitState(page, 'no-card-data');
  assert.equal(await text(page, '#verifier-headline'), COPY.en.noCardData);
  assert.equal(await text(page, '#verifier-result .vf-phone'), COPY.en.phone);
  assert.equal(await page.evaluate(() => document.querySelector('#verifier-limits').open), false);
  await page.click('#verifier-result [data-open-limits]');
  assert.equal(await page.evaluate(() => document.querySelector('#verifier-limits').open), true);
  assert.equal(await page.evaluate(() => location.hash), '');
  await close();
});

test('other files are refused before reading', async () => {
  let { page, close } = await openFile('big.png', new Uint8Array(17 * 1024 * 1024));
  await waitState(page, 'file-too-large');
  assert.equal(await text(page, '#verifier-headline'), COPY.en.fileTooLarge);
  await close();
  ({ page, close } = await openFile('card.png', enc('just some text, not a card')));
  await waitState(page, 'file-type');
  assert.equal(await text(page, '#verifier-headline'), COPY.en.fileType);
  await close();
});

test('a re-encoded image explains the metadata loss', async () => {
  const jpeg = Uint8Array.from([0xff, 0xd8, 0xff, 0xe0, 0, 16, 0x4a, 0x46, 0x49, 0x46, 0, 1]);
  const webp = concat(enc('RIFF'), Uint8Array.from([30, 0, 0, 0]), enc('WEBPVP8 '), new Uint8Array(22));
  for (const [name, bytes] of [['card.jpg', jpeg], ['card.webp', webp]]) {
    const { page, close } = await openFile(name, bytes);
    await waitState(page, 'no-card-data');
    assert.ok(await page.locator('#verifier-result [data-open-limits]').count() > 0, name);
    await close();
  }
});

test('the language toggle never repeats the fragment', async () => {
  const { page, close } = await open(link('story'));
  await waitState(page, 'level1');
  assert.equal(await page.getAttribute('[data-lang-switch]', 'href'), '/fr/verifier#c=' + token('story'));
  for (const angle of ['curve', 'collective']) {
    await page.evaluate((h) => { location.hash = h; }, '#c=' + token(angle));
    await page.waitForFunction((h) => document.querySelector('[data-lang-switch]').getAttribute('href').endsWith(h), token(angle));
  }
  const href = await page.getAttribute('[data-lang-switch]', 'href');
  assert.equal(href, '/fr/verifier#c=' + token('collective'));
  assert.equal(href.split('#').length, 2);
  await close();
});

const NO_ED25519 = `{ const importKey = SubtleCrypto.prototype.importKey;
  SubtleCrypto.prototype.importKey = function (format, key, alg, ...rest) {
    if ((alg?.name ?? alg) === 'Ed25519') return Promise.reject(new DOMException('no Ed25519', 'NotSupportedError'));
    return importKey.call(this, format, key, alg, ...rest);
  }; }`;
const NOBLE = '/assets/vendor/noble-ed25519-3.2.0.js';

test('without WebCrypto Ed25519 the vendored fallback verifies', async () => {
  const { page, close } = await open('/verifier', { init: NO_ED25519 });
  const served = [];
  page.on('response', (r) => { if (r.url().endsWith(NOBLE) && r.ok()) served.push(r.url()); });
  await page.evaluate((h) => { location.hash = h; }, '#c=' + token('story'));
  await waitState(page, 'level1');
  assert.equal(served.length, 1);
  await close();
});

test('a modified vendored file stops the check', async () => {
  const { page, close } = await open('/verifier', { init: NO_ED25519 });
  const body = readFileSync(join(SITE, NOBLE));
  body[100] ^= 1;
  await page.route('**' + NOBLE, (route) => route.fulfill({ status: 200, contentType: 'text/javascript', body }));
  await page.evaluate((h) => { location.hash = h; }, '#c=' + token('story'));
  await waitState(page, 'cannot-verify');
  assert.equal(await text(page, '#verifier-headline'), COPY.en.cannotVerify);
  await close();
});

// ---- the file content check and the signed figures (Task 5b) ----

const FORMATS = ['png', 'svg', 'jxl', 'pdf'];
const precedes = (page, a, b) => page.evaluate(([x, y]) => Boolean(document.querySelector(x)
  .compareDocumentPosition(document.querySelector(y)) & Node.DOCUMENT_POSITION_FOLLOWING), [a, b]);
const figure = (page, name) => text(page, `[data-figure="${name}"]`);

test('a signed figure shows once, with the label its file gives it', async () => {
  for (const [how, opened] of [['link', () => open(link('story'))], ['file', () => openFile('story.png', fixture('story.png'))],
    ['curve', () => open(link('curve'))], ['collective', () => open(link('collective'))]]) {
    const { page, close } = await opened();
    await waitState(page, 'level1');
    const names = await page.$$eval('#verifier-figures [data-figure]', (els) => els.map((e) => e.dataset.figure));
    const fields = await page.$$eval('#verifier-details [data-field]', (els) => els.map((e) => e.dataset.field));
    for (const name of names) {
      assert.equal(fields.includes(name) || fields.includes('figures.' + name), false, `${how}: ${name} twice`);
    }
    await close();
  }
  const { page, close } = await openFile('story.png', fixture('story.png'));
  await waitState(page, 'level1');
  assert.equal(await text(page, '[data-figure="cited_ops"] dt'), COPY.en.typedFields.nPlusOneCitedOps);
  await close();
});

test('the signed figures come first, with the compare line', async () => {
  let { page, close } = await open(link('story'));
  await waitState(page, 'level1');
  assert.equal(await precedes(page, '#verifier-headline', '#verifier-figures'), true);
  assert.equal(await precedes(page, '#verifier-figures', '#verifier-details'), true);
  assert.equal(await text(page, '#verifier-figures h2'), COPY.en.figuresTitle);
  assert.ok((await text(page, '#verifier-figures')).includes(COPY.en.figuresCompare));
  assert.ok((await figure(page, 'cited_ops')).includes('41'));
  assert.ok((await figure(page, 'before_fix_ops')).includes('1.9 million'));
  assert.ok((await figure(page, 'before_fix_period')).includes('July 2026'));
  assert.ok((await figure(page, 'period')).includes('September 2026'));
  const names = await page.$$eval('#verifier-figures [data-figure]', (els) => els.map((e) => e.dataset.figure));
  assert.deepEqual(names, ['period', 'before_fix_period', 'cited_ops', 'before_fix_ops']);
  assert.equal(await page.locator('#verifier-content').count(), 0);
  await assertNoAccent(page);
  await close();

  ({ page, close } = await open('/fr' + link('story'), { locale: 'fr-FR' }));
  await waitState(page, 'level1');
  assert.ok((await figure(page, 'before_fix_ops')).includes('1,9 million'));
  assert.ok((await figure(page, 'before_fix_period')).includes('juillet 2026'));
  assert.ok((await text(page, '#verifier-figures')).includes(COPY.fr.figuresCompare));
  await close();

  ({ page, close } = await open(link('slow')));
  await waitState(page, 'level1');
  assert.ok((await figure(page, 'cited_ms')).includes('2.3 s or more'));
  await close();
});

test('an original file says intact', async () => {
  for (const format of FORMATS) {
    const { page, close } = await openFile(`story.${format}`, fixture(fx.cards.story[format]));
    await waitState(page, 'level1');
    assert.equal(await page.getAttribute('#verifier-content', 'data-content'), 'intact', format);
    assert.equal(await text(page, '#verifier-content'), COPY.en.contentIntact, format);
    assert.equal(await visible(page, '#verifier-figures'), true, format);
    assert.equal(await precedes(page, '#verifier-content', '#verifier-figures'), true, format);
    if (format === 'png') await assertNoAccent(page);
    await close();
  }
});

test('a copied envelope reads modified', async () => {
  for (const format of FORMATS) {
    const { page, external, close } = await openFile(`card.${format}`, fixture(fx.swapped[format]));
    await waitState(page, 'modified');
    assert.equal(await text(page, '#verifier-headline'), COPY.en.modified, format);
    assert.equal(await visible(page, '#verifier-next'), true, format);
    assert.equal(await text(page, '#verifier-next'), COPY.en.modifiedNext, format);
    assert.ok(await page.locator('#verifier-result [data-open-limits]').count() > 0, format);
    assert.equal(await page.locator('#verifier-result [data-ico="shield-alert"]').count(), 1, format);
    for (const sel of ['[data-field]', '#verifier-figures', '#verifier-content', '#verifier-dns']) {
      assert.equal(await page.locator(sel).count(), 0, `${format} ${sel}`);
    }
    assert.deepEqual(external, [], format);
    await assertNoAccent(page);
    await close();
  }
});

test('the limits explain a reposted card', async () => {
  for (const [path, lang] of [['/verifier', 'en'], ['/fr/verifier', 'fr']]) {
    const { page, close } = await open(path);
    assert.ok((await text(page, 'details#verifier-limits')).includes(COPY[lang].limitRepost), lang);
    await close();
  }
});

test('a broken carrier reads no card data, a changed image modified', async () => {
  const png = fixture('story.png');
  const svg = dec(fixture('story.svg'));
  const element = /<metadata id="perf-sentinel-card">[\s\S]*?<\/metadata>/.exec(svg)[0];
  const env = /<metadata id="perf-sentinel-card">([\s\S]*?)<\/metadata>/.exec(svg)[1];
  const pngText = readPngCard(png);
  const parts = jxlParts(fixture('story.jxl'));
  const jxlEnv = dec(parts.pscd.subarray(8));
  const wide = new Uint8Array(16);
  new DataView(wide.buffer).setUint32(0, 1);
  wide.set(enc('pscd'), 4);
  new DataView(wide.buffer).setUint32(12, 8 + parts.pscd.length);
  const crc = png.slice();
  crc[pngChunks(png).find((c) => c.type === 'iTXt').end - 1] ^= 1;
  const unread = {
    'bad envelope CRC.png': crc,
    'no IEND.png': stripChunk(png, 'IEND'),
    'png reordered.png': replaceChunk(png, 'iTXt', cardChunk(reorderEnvelope(pngText))),
    'png space.png': replaceChunk(png, 'iTXt', cardChunk(spaceEnvelope(pngText))),
    'in title.svg': enc(svg.replace(element + '\n', '').replace('</title>', element + '</title>')),
    'svg reordered.svg': enc(svg.replace(env, reorderEnvelope(env))),
    'pscd after jxlc.jxl': concat(parts['JXL '], parts.ftyp, parts.jxlc, parts.pscd),
    '64-bit pscd.jxl': concat(parts['JXL '], parts.ftyp, wide, parts.pscd.subarray(8), parts.jxlc),
    'jxl space.jxl': concat(parts['JXL '], parts.ftyp, jxlBox('pscd', enc(spaceEnvelope(jxlEnv))), parts.jxlc),
  };
  for (const [name, bytes] of Object.entries(unread)) {
    const { page, close } = await openFile(name, bytes);
    await waitState(page, 'no-card-data');
    assert.equal(await page.locator('#verifier-content').count(), 0, name);
    await close();
  }
  for (const [name, bytes] of Object.entries({
    'after IEND.png': concat(png, enc('retouched')),
    'bom.svg': concat(Uint8Array.of(0xef, 0xbb, 0xbf), enc('\n'), fixture('story.svg')),
  })) {
    const { page, close } = await openFile(name, bytes);
    await waitState(page, 'modified');
    await close();
  }
});

// ---- the domain check over DNS-over-HTTPS (Task 6) ----

const DNS_NAME = '_perf-sentinel.cards.example.org';
const txt = (extra) => 'v=ps1; k=' + fx.fingerprint + '; from=2026-01-01' + extra;
const quoted = (extra) => '"' + txt(extra) + '"';
const cloudflareTxt = (extra) => ({ cloudflare: { json: { Status: 0, Answer: [{ type: 16, data: quoted(extra) }] } } });

/** Fake Cloudflare and Google resolvers: each handler `{status, json}` or 'abort' (the default). */
async function fakeDns(page, handlers) {
  const requests = [];
  for (const [resolver, pattern] of [['cloudflare', 'https://cloudflare-dns.com/**'], ['google', 'https://dns.google/**']]) {
    await page.route(pattern, (route) => {
      const req = route.request();
      requests.push({ url: req.url(), accept: req.headers().accept });
      const h = handlers[resolver] ?? 'abort';
      if (h === 'abort') return route.abort();
      const json = structuredClone(h.json);
      // lookupTxt reads only answers for the queried name; Google writes it with a trailing dot.
      for (const a of json.Answer ?? []) a.name ??= DNS_NAME + (resolver === 'google' ? '.' : '');
      return route.fulfill({
        status: h.status ?? 200, contentType: 'application/dns-json',
        headers: { 'access-control-allow-origin': '*' }, body: JSON.stringify(json),
      });
    });
  }
  return requests;
}

/** Opens `path`, waits for level 1, clicks the DNS button and waits for the binding line. */
async function checkDomain(path, handlers, opts) {
  const opened = path.startsWith('/') ? await open(path, opts) : await openFile(path, fixture(path), opts);
  await waitState(opened.page, 'level1');
  const requests = await fakeDns(opened.page, handlers);
  await opened.page.click('#verifier-dns');
  await opened.page.locator('#verifier-binding').waitFor();
  return { ...opened, requests };
}
const binding = (page) => page.getAttribute('#verifier-binding', 'data-binding');

test('nothing leaves the page before the button', async () => {
  const { page, external, close } = await open(link('story'));
  await waitState(page, 'level1');
  const requests = await fakeDns(page, cloudflareTxt(''));
  await page.waitForTimeout(200);
  assert.deepEqual(requests, []);
  assert.deepEqual(external, []);
  assert.equal(await text(page, '#verifier-dns'), COPY.en.dnsButton.replace('{domain}', 'cards.example.org'));
  const note = await text(page, '#verifier-dns-note');
  assert.ok(note.includes('Cloudflare') && note.includes('Google'), note);
  assert.ok(note.includes(DNS_NAME), note);
  await close();
});

test('a published key gives the verified state', async () => {
  let { page, requests, close } = await checkDomain(link('story'), cloudflareTxt(''));
  await waitState(page, 'level2');
  assert.equal(await text(page, '#verifier-headline'), 'Signed by cards.example.org');
  assert.equal(await binding(page), 'bound');
  assert.ok((await text(page, '#verifier-binding')).includes('Cloudflare DNS'));
  const domainLine = COPY.en.level1Domain.replace('{domain}', 'cards.example.org');
  assert.equal((await pageText(page)).includes(domainLine), false);
  assert.equal(await page.locator('#verifier-result [data-ico="shield-check"]').count(), 1);
  assert.equal(await visible(page, '#verifier-figures'), true);
  assert.equal(await label(page, 'org.domain'), COPY.en.domainVerified);
  assert.equal(await page.locator('#verifier-dns').count(), 0);
  assert.deepEqual(requests, [{ url: DOH_URL.cloudflare, accept: 'application/dns-json' }]);
  await close();

  ({ page, requests, close } = await checkDomain('story.png', cloudflareTxt('')));
  await waitState(page, 'level2');
  assert.equal(await text(page, '#verifier-org'), COPY.en.level2Org.replace('{domain}', 'cards.example.org').replace('{org}', 'Example Org'));
  const orgLine = COPY.en.level1Org.replace('{org}', 'Example Org').replace('{domain}', 'cards.example.org');
  assert.equal((await pageText(page)).includes(orgLine), false);
  assert.equal(await page.getAttribute('#verifier-content', 'data-content'), 'intact');
  assert.equal(await visible(page, '#verifier-figures'), true);
  assert.equal(requests.length, 1);
  assert.ok(requests[0].url.endsWith('?name=_perf-sentinel.cards.example.org&type=TXT'));
  await close();
});

const DOH_URL = { cloudflare: 'https://cloudflare-dns.com/dns-query?name=_perf-sentinel.cards.example.org&type=TXT' };

test('the DNS answer sits under the verified headline', async () => {
  for (const path of [link('story'), 'story.png']) {
    const { page, close } = await checkDomain(path, cloudflareTxt(''));
    await waitState(page, 'level2');
    assert.equal(await page.locator('.vf-hd #verifier-binding').count(), 1, path);
    assert.equal(await precedes(page, '#verifier-headline', '#verifier-binding'), true, path);
    assert.equal(await precedes(page, '#verifier-binding', '#verifier-figures'), true, path);
    assert.deepEqual(await strayText(page), [], path);
    await close();
  }
});

test('a key exposed before the card is not verified', async () => {
  for (const [path, lang, day] of [[link('story'), 'en', 'exposed since 1 October 2026'], ['/fr' + link('story'), 'fr', '1er octobre 2026']]) {
    const { page, close } = await checkDomain(path, cloudflareTxt('; revoked=2026-10-01'), { locale: lang });
    await waitState(page, 'level1');
    assert.equal(await binding(page), 'revoked', lang);
    const said = await text(page, '#verifier-binding');
    assert.ok(said.includes(day), said);
    assert.equal(/withdrew the key .* on /.test(said), false, said);
    if (lang === 'en') assert.ok(said.includes('stolen key'), said);
    assert.equal(await page.locator('#verifier-binding [data-ico="shield-alert"]').count(), 1, lang);
    assert.ok((await text(page, '#verifier-binding strong')).includes(day), lang);
    await close();
  }
});

test('a revoked key never verifies, whatever the card\'s date', async () => {
  const { page, close } = await checkDomain(link('story'), cloudflareTxt('; revoked=2026-12-01'));
  await waitState(page, 'level1');
  assert.equal(await binding(page), 'revoked');
  const said = await text(page, '#verifier-binding');
  assert.ok(said.includes('1 December 2026') && said.includes('whatever date it shows'), said);
  await assertNoAccent(page);
  await close();
});

test('a key outside its dates is not verified', async () => {
  const { page, close } = await checkDomain(link('story'), cloudflareTxt('; to=2026-06-30'));
  await waitState(page, 'level1');
  assert.equal(await binding(page), 'out-of-validity');
  assert.equal(await text(page, '#verifier-binding'), COPY.en.outOfValidity.replaceAll('{domain}', 'cards.example.org'));
  await assertNoAccent(page);
  await close();
});

test('no matching key reads as not published', async () => {
  let { page, close } = await checkDomain(link('story'), { cloudflare: { json: { Status: 3 } } });
  await waitState(page, 'level1');
  assert.equal(await binding(page), 'not-published');
  assert.equal(await text(page, '#verifier-binding'), COPY.en.notPublished.replace('{domain}', 'cards.example.org'));
  await close();

  const other = { cloudflare: { json: { Status: 0, Answer: [{ type: 16, data: '"v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-01-01"' }] } } };
  ({ page, close } = await checkDomain(link('story'), other));
  await waitState(page, 'level1');
  assert.equal(await binding(page), 'not-published');
  await assertNoAccent(page);
  await close();
});

test('Cloudflare down falls back to Google and says so', async () => {
  const { page, requests, close } = await checkDomain(link('story'), {
    cloudflare: 'abort', google: { json: { Status: 0, Answer: [{ type: 16, data: txt('') }] } },
  });
  await waitState(page, 'level2');
  assert.equal(await binding(page), 'bound');
  assert.ok((await text(page, '#verifier-binding')).includes('Google Public DNS'));
  assert.equal(requests.length, 2);
  await close();
});

test('an unreadable file never leaves the previous verdict standing', async () => {
  const { page, close } = await checkDomain('story.png', cloudflareTxt(''));
  await waitState(page, 'level2');
  // NotReadableError: the file changed after it was picked, or a cloud placeholder failed to download.
  await page.evaluate(() => {
    File.prototype.arrayBuffer = () => Promise.reject(new DOMException('unreadable', 'NotReadableError'));
  });
  await choose(page, 'other.png', fixture('story.png'));
  await waitState(page, 'file-unreadable');
  assert.equal(await text(page, '#verifier-headline'), COPY.en.fileUnreadable);
  assert.equal(await page.locator('#verifier-binding').count(), 0);
  await close();
});

test('both resolvers down reads as could not check', async () => {
  const { page, close } = await checkDomain(link('story'), { cloudflare: 'abort', google: 'abort' });
  await waitState(page, 'level1');
  assert.equal(await binding(page), 'unavailable');
  assert.equal(await text(page, '#verifier-binding'), COPY.en.unavailable);
  assert.equal(await page.isEnabled('#verifier-dns'), true);
  assert.equal(await text(page, '#verifier-dns'), 'Try cards.example.org again');
  assert.equal(await visible(page, '#verifier-dns-note'), true);
  await assertNoAccent(page);
  await close();
});

test('a French reader of the QR link lands on the French page with the card', async () => {
  const { page, close } = await open(link('story'), { locale: 'fr-FR' });
  await page.waitForURL('**/fr/verifier#c=*');
  assert.ok(page.url().endsWith('/fr/verifier#c=' + token('story')));
  await waitState(page, 'level1');
  assert.equal(await page.getAttribute('html', 'lang'), 'fr');
  assert.equal(await text(page, '#verifier-headline'), COPY.fr.level1);
  await close();
});

test('a stored English choice keeps the English page', async () => {
  const { page, close } = await open(link('story'), { locale: 'fr-FR', init: `localStorage.setItem('ps-lang', 'en')` });
  await waitState(page, 'level1');
  assert.ok(page.url().endsWith('/verifier#c=' + token('story')));
  assert.ok(!page.url().includes('/fr/'));
  assert.equal(await text(page, '#verifier-headline'), COPY.en.level1);
  await close();
});

test('the French URL never redirects', async () => {
  const { page, close } = await open('/fr/verifier#c=' + token('story'), { locale: 'en-US' });
  await waitState(page, 'level1');
  assert.ok(page.url().endsWith('/fr/verifier#c=' + token('story')));
  assert.equal(await text(page, '#verifier-headline'), COPY.fr.level1);
  await close();
});

test('the language toggle keeps the card', async () => {
  let { page, close } = await open(link('story'));
  await waitState(page, 'level1');
  assert.ok((await page.getAttribute('[data-lang-switch]', 'href')).endsWith('/fr/verifier#c=' + token('story')));
  await close();
  ({ page, close } = await open('/fr/verifier#c=' + token('story')));
  await waitState(page, 'level1');
  const href = await page.getAttribute('[data-lang-switch]', 'href');
  assert.ok(href.endsWith('/verifier#c=' + token('story')) && !href.includes('/fr/'));
  await close();
});

test('the privacy policy names both resolvers', async () => {
  for (const path of ['/privacy-policy', '/confidentialite']) {
    const { page, close } = await open(path);
    const body = await pageText(page);
    assert.ok(body.includes('Cloudflare'), path);
    assert.ok(body.includes('Google Public DNS'), path);
    await close();
  }
});
