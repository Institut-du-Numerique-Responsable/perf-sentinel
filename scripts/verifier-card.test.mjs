import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { crc32 } from 'node:zlib';
import {
  b64urlDecode, b64urlEncode, parseToken, parseEnvelope, parseFragment, fingerprint,
  verifyEd25519, defaultLoadNoble, EngineUnavailable, openCard, expandCompact,
  formatValue, periodName, printedCo2Ug, equivalenceCount, leafPaths, contentMessage, bigFigures,
} from '../assets/verifier/card.js';
import {
  inflateCapped, pngEnvelopeRange, readPngCard, jxlBoxes, jxlEnvelopeRange, svgEnvelopeRange,
  readPdfAttachment, readPdfCard, readJxlCard, readSvgCard, cardContent,
} from '../assets/verifier/files.js';
import { COPY } from '../assets/verifier/copy.js';
import { DOH, isValidDomain, lookupTxt, parseTxtRecord, bindingState } from '../assets/verifier/dns.js';
import {
  tokenOf, flip, concat, stripChunk, jxlBox, jxlParts, pngChunks, moveChunk, replaceChunk, cardChunk,
  reorderEnvelope, spaceEnvelope,
} from './verifier-test-helpers.mjs';

const here = (p) => new URL(p, import.meta.url);
const bytesOf = (p) => new Uint8Array(readFileSync(here(p)));
const fixture = (name) => bytesOf(`fixtures/verifier/${name}`);
const fx = JSON.parse(readFileSync(here('fixtures/verifier/fixtures.json'), 'utf8'));
const enc = (s) => new TextEncoder().encode(s);
const dec = (b) => new TextDecoder().decode(b);
const NOBLE = '../assets/vendor/noble-ed25519-3.2.0.js';
const ANGLES = ['story', 'slow', 'curve', 'collective'];

const storyToken = tokenOf(fx.cards.story.compact_url);
const storyPair = () => parseFragment('#c=' + storyToken);

test('b64url round-trips and rejects foreign characters', () => {
  for (let n = 0; n <= 70; n++) {
    const bytes = Uint8Array.from({ length: n }, (_, i) => (i * 37 + n) & 255);
    assert.deepEqual(b64urlDecode(b64urlEncode(bytes)), bytes);
  }
  assert.throws(() => b64urlDecode('ab+c'));
  assert.throws(() => b64urlDecode('ab=c'));
});

test('b64url refuses non-canonical text', () => {
  assert.deepEqual(b64urlDecode('AA'), new Uint8Array([0]));
  assert.throws(() => b64urlDecode('AB'));
  assert.throws(() => b64urlDecode('A'));
  assert.throws(() => b64urlDecode('AA=='));
  const [payload, sig] = storyToken.split('.');
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  const last = sig.length - 1;
  const lax = sig.slice(0, last) + alphabet[alphabet.indexOf(sig[last]) ^ 1];
  assert.equal(parseToken(`${payload}.${lax}`), null);
});

test('the file envelope parses', () => {
  const sig = 'A'.repeat(86);
  const canonical = (cs, p, s) => `{"content_sig":"${cs}","payload":"${p}","signature":"${s}"}`;
  const e = parseEnvelope(canonical(sig, 'e30', sig));
  assert.equal(e.payload.length, 2);
  assert.equal(e.sig.length, 64);
  assert.equal(e.contentSig.length, 64);
  assert.equal(parseEnvelope(`{"payload":"e30","signature":"${sig}"}`), null);
  assert.equal(parseEnvelope(`{"content_sig":"${sig}","extra":"x","payload":"e30","signature":"${sig}"}`), null);
  assert.equal(parseEnvelope(`{"content_sig":"${sig}","payload":"e30"}`), null);
  assert.equal(parseEnvelope(canonical(sig, 'e30=', sig)), null);
  assert.equal(parseEnvelope('not json'), null);
  assert.equal(parseEnvelope(`{"payload":"e30","content_sig":"${sig}","signature":"${sig}"}`), null);
  assert.equal(parseEnvelope(canonical(sig, 'e30', sig).replace(':', ': ')), null);
  assert.equal(parseEnvelope(canonical(sig, 'e30', sig) + '\n'), null);
  assert.equal(parseEnvelope(canonical('\\u0041' + sig.slice(1), 'e30', sig)), null);
});

test('fingerprint matches the engine', async () => {
  const fp = await fingerprint(b64urlDecode(fx.public_key));
  assert.equal(fp, fx.fingerprint);
  assert.match(fp, /^ps1:[A-Za-z0-9_-]{22}$/);
});

test('parseFragment refuses malformed fragments', () => {
  const [payload, sig] = storyToken.split('.');
  const short = b64urlEncode(b64urlDecode(sig).subarray(0, 63));
  for (const hash of ['', '#c=', '#x=a.b', '#c=a', '#c=a.b.c', `#c=${payload}.${short}`, '#c=' + 'A'.repeat(4094)]) {
    assert.equal(parseFragment(hash), null, hash.slice(0, 40));
  }
  assert.equal(('#c=' + 'A'.repeat(4094)).length, 4097);
});

test('a compact link opens at the signed values', async () => {
  const before = {
    story: { from: '2026-07-01', to: '2026-07-31' },
    slow: { from: '2026-08-01', to: '2026-08-31' },
    collective: { from: '2026-08-01', to: '2026-08-31' },
  };
  for (const name of ANGLES) {
    const r = await openCard(parseFragment('#c=' + tokenOf(fx.cards[name].compact_url)), 'compact');
    assert.equal(r.state, 'ok', name);
    assert.equal(r.card.org.domain, 'cards.example.org');
    assert.deepEqual(r.card.period, { kind: 'month', from: '2026-09-01', to: '2026-09-30' });
    assert.equal(r.date, '2026-10-15');
    assert.equal(r.fingerprint, fx.fingerprint);
    assert.equal(r.card.angle, fx.cards[name].figures.angle);
    if (name === 'curve') assert.equal('before_fix_period' in r.card, false);
    else assert.deepEqual(r.card.before_fix_period, before[name], name);
    if (name === 'curve') assert.equal(r.card.figures.previous_waste_permille, 48);
    if (name === 'slow') {
      assert.equal(r.card.figures.cited_ms, 2349);
      assert.equal(r.card.figures.threshold_ms, 500);
    }
  }
  const nodomain = await openCard(parseFragment('#c=' + tokenOf(fx.cards.nodomain.compact_url)), 'compact');
  assert.equal(nodomain.state, 'ok');
  assert.equal('org' in nodomain.card, false);
});

test('a self-signed payload missing what the page reads is unsupported', async () => {
  const keys = await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify']);
  const pub = new Uint8Array(await crypto.subtle.exportKey('raw', keys.publicKey));
  const base = {
    angle: 'story',
    generated_at_ms: 1792054800000,
    key: await fingerprint(pub),
    period: { kind: 'month', from: '2026-09-01', to: '2026-09-30' },
    public_key: b64urlEncode(pub),
    v: 1,
  };
  const open = async (card) => {
    const payload = enc(JSON.stringify(card));
    const sig = new Uint8Array(await crypto.subtle.sign('Ed25519', keys.privateKey, payload));
    return openCard({ payload, sig }, 'full');
  };
  assert.equal((await open(base)).state, 'unsupported');
  assert.equal((await open({ ...base, org: { name: '' } })).state, 'unsupported');
  const ok = await open({ ...base, org: { name: 'X' } });
  assert.equal(ok.state, 'ok');
  assert.equal(ok.date, '2026-10-15');
  const withOrg = { ...base, org: { name: 'X' } };
  for (const bad of [null, 5, 'x', { from: 5, to: 6 }, { from: 'x', to: '2026-08-31' }, { from: '2026-08-01' }]) {
    assert.equal((await open({ ...withOrg, before_fix_period: bad })).state, 'unsupported', JSON.stringify(bad));
  }
  for (const period of [{ kind: 'month', from: 'x', to: '2026-09-30' }, { kind: 'month', from: '2026-09-01', to: '2026-9-30' }]) {
    assert.equal((await open({ ...withOrg, period })).state, 'unsupported', JSON.stringify(period));
  }
  const good = await open({ ...withOrg, before_fix_period: { from: '2026-08-01', to: '2026-08-31' } });
  assert.equal(good.state, 'ok');
});

test('one changed character reads as tampered', async () => {
  const [payload, sig] = storyToken.split('.');
  for (const [part, other, join] of [[payload, sig, (p, o) => `${p}.${o}`], [sig, payload, (s, o) => `${o}.${s}`]]) {
    for (const i of [0, Math.floor(part.length / 2), part.length - 1]) {
      const pair = parseFragment('#c=' + join(flip(part, i), other));
      assert.notEqual(pair, null, `index ${i}`);
      assert.equal((await openCard(pair, 'compact')).state, 'tampered', `index ${i}`);
    }
  }
});

test('an unknown version is unsupported', async () => {
  const r = await openCard({ payload: enc('{"v":2}'), sig: new Uint8Array(64) }, 'compact');
  assert.equal(r.state, 'unsupported');
});

test('the noble fallback agrees with WebCrypto', async () => {
  const { payload, sig } = storyPair();
  const pub = b64urlDecode(fx.public_key);
  const subtle = crypto.subtle;
  const realImport = subtle.importKey;
  subtle.importKey = async (...args) => {
    if (JSON.stringify(args).includes('Ed25519')) throw new Error('no Ed25519 here');
    return realImport.apply(subtle, args);
  };
  try {
    const loadNoble = () => import(NOBLE);
    assert.equal(await verifyEd25519(pub, sig, payload, loadNoble), true);
    const bad = sig.slice();
    bad[0] ^= 1;
    assert.equal(await verifyEd25519(pub, bad, payload, loadNoble), false);
  } finally {
    delete subtle.importKey;
  }
  assert.equal(subtle.importKey, realImport);
});

test('a noble file with the wrong hash is refused', async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => new Response(concat(bytesOf(NOBLE), new Uint8Array([10])));
  try {
    await assert.rejects(defaultLoadNoble(), EngineUnavailable);
  } finally {
    globalThis.fetch = realFetch;
  }
});

const pngCard = async () => openCard(parseEnvelope(await readPngCard(fixture('story.png'))), 'full');

test('the PNG carries the full payload', async () => {
  const r = await pngCard();
  assert.equal(r.state, 'ok');
  assert.equal(r.card.free.title, '<b>Q3</b> & "review"');
  assert.equal(r.card.key, fx.fingerprint);
});

test('a PNG without the chunk has no card', async () => {
  assert.equal(await readPngCard(stripChunk(fixture('story.png'), 'iTXt')), null);
  assert.equal(await readPngCard(fixture('story.jxl')), null);
  assert.equal(await readPngCard(fixture('story.png').subarray(0, 100)), null);
});

test('the PDF carries the full payload', async () => {
  const r = await openCard(parseEnvelope(await readPdfCard(fixture('story.pdf'))), 'full');
  assert.equal(r.state, 'ok');
  assert.equal(r.card.key, fx.fingerprint);
});

test('the JXL carries the full payload', async () => {
  const r = await openCard(parseEnvelope(readJxlCard(fixture('story.jxl'))), 'full');
  const png = await pngCard();
  assert.equal(r.state, 'ok');
  assert.equal(r.card.key, png.card.key);
  assert.equal(r.card.free.title, png.card.free.title);
});

const story = jxlParts(fixture('story.jxl'));
const head = () => [story['JXL '], story.ftyp];

test('a JXL without its box has no card', () => {
  const codestream = story.jxlc.subarray(8);
  assert.equal(codestream[0], 0xff);
  assert.equal(codestream[1], 0x0a);
  const cases = {
    'no pscd': concat(...head(), story.jxlc),
    twice: concat(...head(), story.pscd, story.jxlc, story.pscd),
    'bare codestream': codestream,
    'over 64 KiB': concat(...head(), jxlBox('pscd', enc('A'.repeat(65537))), story.jxlc),
    'not UTF-8': concat(...head(), jxlBox('pscd', new Uint8Array([0xff, 0xfe])), story.jxlc),
    'not a JXL': fixture('story.png'),
    'pscd after jxlc': concat(...head(), story.jxlc, story.pscd),
  };
  for (const [name, bytes] of Object.entries(cases)) assert.equal(readJxlCard(bytes), null, name);
});

test('a pscd inside another box is not seen', () => {
  const codestream = story.jxlc.subarray(8);
  const inJxlc = concat(...head(), jxlBox('jxlc', concat(story.pscd, codestream)));
  const inBrob = concat(...head(), jxlBox('brob', concat(enc('pscd'), story.pscd)), story.jxlc);
  assert.equal(readJxlCard(inJxlc), null);
  assert.equal(readJxlCard(inBrob), null);
});

test('JXL box sizes are bounds-checked', () => {
  const sig = story['JXL '];
  const header = (...words) => {
    const b = new Uint8Array(words.length * 4);
    words.forEach((w, i) => new DataView(b.buffer).setUint32(i * 4, typeof w === 'string' ? new DataView(enc(w).buffer).getUint32(0) : w));
    return b;
  };
  const cases = {
    'size 0xFFFFFFFF': concat(sig, header(0xffffffff, 'jxlc'), new Uint8Array(16)),
    'size 4': concat(sig, header(4, 'jxlc'), new Uint8Array(16)),
    'huge 64-bit size': concat(sig, header(1, 'jxlc', 0xffffffff, 0), new Uint8Array(16)),
    '64-bit size 8': concat(sig, header(1, 'jxlc', 0, 8), new Uint8Array(16)),
    'cut header': concat(sig, header(20).subarray(0, 4), enc('j')),
  };
  for (const [name, bytes] of Object.entries(cases)) {
    assert.equal(jxlBoxes(bytes), null, name);
    assert.equal(readJxlCard(bytes), null, name);
  }
});

const svgText = () => dec(fixture('story.svg'));
const svgElement = () => {
  const t = svgText();
  const at = t.indexOf('<metadata id="perf-sentinel-card">');
  return t.slice(at, t.indexOf('</metadata>', at) + '</metadata>'.length);
};

test('the SVG carries the full payload', async () => {
  const r = await openCard(parseEnvelope(readSvgCard(fixture('story.svg'))), 'full');
  const png = await pngCard();
  assert.equal(r.state, 'ok');
  assert.equal(r.card.key, png.card.key);
  assert.equal(r.card.free.title, png.card.free.title);
});

test('an SVG without its metadata has no card', () => {
  const t = svgText();
  const el = svgElement();
  const without = t.replace(el, '');
  const cases = {
    'cut out': enc(without),
    twice: enc(t.replace('</svg>', el + '</svg>')),
    script: enc(t.replace('<metadata id="perf-sentinel-card">', '<metadata id="perf-sentinel-card"><script>')),
    'no closing tag': enc(t.replace(el, el.slice(0, -'</metadata>'.length))),
    html: enc('<!doctype html><html><body>' + t.slice(t.indexOf('<svg')) + '</body></html>'),
    'not UTF-8': concat(fixture('story.svg'), new Uint8Array([0xff])),
    'inside title': enc(without.replace('</title>', el + '</title>')),
    'inside d': enc(without.replace(/ d="/, ` d="${el}`)),
  };
  assert.ok(without.includes(' d="'));
  for (const [name, bytes] of Object.entries(cases)) assert.equal(readSvgCard(bytes), null, name);
  const bom = concat(new Uint8Array([0xef, 0xbb, 0xbf]), enc('\n'), fixture('story.svg'));
  assert.equal(readSvgCard(bom), readSvgCard(fixture('story.svg')));
  assert.notEqual(readSvgCard(bom), null);
});

test('the shared walkers return byte ranges', () => {
  const png = fixture('story.png');
  const range = pngEnvelopeRange(png);
  assert.equal(range instanceof Promise, false);
  const [cs, ce] = range.chunk;
  const view = new DataView(png.buffer, png.byteOffset);
  assert.equal(dec(png.subarray(cs + 4, cs + 8)), 'iTXt');
  assert.equal(cs + 12 + view.getUint32(cs), ce);
  assert.equal(view.getUint32(ce - 4), crc32(png.subarray(cs + 4, ce - 4)));
  const text = readPngCard(png);
  assert.equal(typeof text, 'string');
  assert.equal(text, dec(png.subarray(...range.text)));

  const jxl = fixture('story.jxl');
  const boxes = jxlBoxes(jxl);
  assert.equal(boxes instanceof Promise, false);
  assert.deepEqual(boxes.map((b) => b.type), ['JXL ', 'ftyp', 'pscd', 'jxlc']);
  boxes.forEach((b, i) => assert.equal(b.start, i === 0 ? 0 : boxes[i - 1].end));
  assert.equal(boxes.at(-1).end, jxl.length);
  const env = jxlEnvelopeRange(jxl);
  assert.deepEqual(env.box, [boxes[2].start, boxes[2].end]);

  const svg = fixture('story.svg');
  const s = svgEnvelopeRange(svg);
  assert.equal(s instanceof Promise, false);
  const element = dec(svg.subarray(...s.element));
  assert.ok(element.startsWith('<metadata id="perf-sentinel-card">'));
  assert.ok(element.endsWith('</metadata>'));
  assert.equal(dec(svg.subarray(s.element[0] - 9, s.element[0])), '</title>\n');
});

test('the PDF reader handles a real krilla attachment', async () => {
  const pdf = bytesOf('../exemple/disclosure/report.pdf');
  assert.deepEqual(await readPdfAttachment(pdf, 'report.json'), bytesOf('../exemple/disclosure/report.json'));
  assert.equal(await readPdfCard(pdf), null);
});

test('inflation is capped', async () => {
  const zeros = new Uint8Array(2 << 20);
  const deflated = new Uint8Array(await new Response(new Blob([zeros]).stream().pipeThrough(new CompressionStream('deflate'))).arrayBuffer());
  assert.equal(await inflateCapped(deflated), null);
});

const compactOf = (name) => JSON.parse(dec(parseFragment('#c=' + tokenOf(fx.cards[name].compact_url)).payload));

test('fixture compact payloads all expand', () => {
  for (const name of Object.keys(fx.cards)) assert.notEqual(expandCompact(compactOf(name)), null, name);
  const s = compactOf('story');
  const c = compactOf('curve');
  const cases = {
    'extra key': { ...s, z: 1 },
    'unknown angle': { ...s, a: 'z' },
    'unknown kind': { ...s, p: ['z', s.p[1], s.p[2]] },
    'curve f on a story': { ...s, f: c.f },
    'r on a curve': { ...c, r: [s.p[1] - 30, s.p[1] - 1] },
    'r after p[2]': { ...s, r: [s.r[0], s.p[2] + 1] },
  };
  for (const [name, compact] of Object.entries(cases)) assert.equal(expandCompact(compact), null, name);
});

test('values are formatted by unit', () => {
  assert.equal(formatValue('x_ug', 2_460_000_000, 'en'), 'at least 2.4 kg');
  assert.equal(formatValue('x_ug', 12_400_000_000, 'en'), 'at least 12 kg');
  const fr = formatValue('x_ug', 2_500_000_000, 'fr');
  for (const part of ['au moins', '2,5', 'kg']) assert.ok(fr.includes(part), fr);
  assert.equal(formatValue('x_permille', 125, 'en'), '12.5%');
  assert.equal(formatValue('x_ms', 2349, 'en'), '2.3 s');
  assert.equal(formatValue('figures.cited_ms', 2349, 'en'), '2.3 s or more');
  assert.equal(formatValue('x_ms', 640, 'en'), '0.6 s');
  assert.equal(formatValue('x_ms', 500, 'en'), '0.5 s');
  assert.equal(formatValue('x_ms', 40, 'en'), '40 ms');
  assert.equal(formatValue('x_permille', 0, 'en'), 'under 0.1%');
  assert.equal(formatValue('x_permille', 0, 'fr'), COPY.fr.underTenthPercent);
  assert.equal(formatValue('x_permille', 250, 'en'), '25%');
  assert.equal(formatValue('x_ops', 1_937_000, 'en'), '1.9 million');
  assert.equal(formatValue('x_ops', 1_937_000, 'fr'), '1,9 million');
  assert.equal(formatValue('x_ops', 2_480_000, 'fr'), '2,4 millions');
  assert.equal(formatValue('x_ops', 41, 'en'), '41');
  assert.equal(formatValue('x_count', 9_999, 'en'), '9,999');
  assert.equal(formatValue('period.kind', 'quarter', 'fr'), COPY.fr.kinds.quarter);
  assert.equal(formatValue('story.done_at', '2026-10-01', 'fr'), '1er octobre 2026');
  assert.equal(periodName('month', '2026-07-01', 'fr'), 'juillet 2026');
});

test('the equivalence count uses the printed CO2', () => {
  assert.equal(equivalenceCount(12_400_000_000, 2_040_000_000), 5);
  assert.equal(equivalenceCount(15_800_000_000, 2_040_000_000), 7);
  assert.equal(equivalenceCount(19_800_000_000, 2_040_000_000), 9);
  assert.equal(printedCo2Ug(999_999), 990_000);
});

// ---- DNS-over-HTTPS and the binding rule ----

const fp = 'ps1:' + 'A'.repeat(22);
const rec = (extra) => 'v=ps1; k=' + fp + '; from=2026-01-01' + extra;
const DNS_NAME = '_perf-sentinel.cards.example.org';
const QUERY = `name=${DNS_NAME}&type=TXT`;

/**
 * A fetch answering `replies` in order: an Error is thrown, a number is that HTTP
 * status, a string a 200 non-JSON body, an object a 200 JSON body whose `Answer`
 * items get the query name (Google adds the trailing dot) unless they set `name`.
 */
function fakeDoh(...replies) {
  const calls = [];
  const fetchImpl = async (url, init) => {
    calls.push({ url, init });
    const reply = replies[calls.length - 1];
    if (reply instanceof Error) throw reply;
    if (typeof reply === 'number') return new Response('', { status: reply });
    if (typeof reply === 'string') return new Response(reply, { status: 200 });
    const name = DNS_NAME + (url.startsWith(DOH.google) ? '.' : '');
    const Answer = reply.Answer?.map((a) => ('name' in a ? a : { ...a, name }));
    return new Response(JSON.stringify({ ...reply, Answer }), { status: 200 });
  };
  return { fetchImpl, calls };
}

test('TXT records parse in both resolver shapes', () => {
  const want = { k: fp, from: '2026-01-01' };
  assert.deepEqual(parseTxtRecord('"' + rec('') + '"'), want);
  assert.deepEqual(parseTxtRecord(rec('')), want);
  assert.equal(parseTxtRecord('"v=ps1; k=' + fp.slice(0, 10) + '" "' + fp.slice(10) + '; from=2026-01-01"').k, fp);
  assert.deepEqual(parseTxtRecord(rec('; to=2026-12-31; revoked=2026-12-01')), { ...want, to: '2026-12-31', revoked: '2026-12-01' });
  for (const bad of ['v=ps2; k=' + fp + '; from=2026-01-01', 'v=ps1; from=2026-01-01', 'v=ps1; k=' + fp, 'v=ps1; k=' + fp + '; from=2026-1-1', rec('; from=2026-02-01')]) {
    assert.equal(parseTxtRecord(bad), null, bad);
  }
});

test('binding follows the key dates', () => {
  const state = (records) => bindingState(records, fp, '2026-10-15').state;
  const other = 'v=ps1; k=ps1:' + 'B'.repeat(22) + '; from=2026-01-01';
  const cases = [
    [[rec('')], 'bound'],
    [[rec('; revoked=2026-10-01')], 'revoked'],
    [[rec('; revoked=2026-10-15')], 'revoked'],
    [[rec('; revoked=2026-12-01')], 'revoked'],
    [[rec('; to=2026-12-31; revoked=2026-12-01')], 'revoked'],
    [[rec(''), rec('; revoked=2026-12-01')], 'revoked'],
    [[rec('; to=2026-06-30')], 'out-of-validity'],
    [[rec('; to=2026-10-15')], 'bound'],
    [[other], 'not-published'],
    [[], 'not-published'],
    [['garbage', rec('')], 'bound'],
    [[rec(''), rec('; to=2026-06-30')], 'out-of-validity'],
    [[rec('; to=2026-06-30'), rec('')], 'out-of-validity'],
    [['v=ps1; k=' + fp + '; from=2026-11-01', rec('')], 'out-of-validity'],
    [[rec('; to=2026-06-30'), rec('; to=2026-12-31')], 'out-of-validity'],
  ];
  for (const [records, want] of cases) assert.equal(state(records), want, JSON.stringify(records));
  const revoked = bindingState([rec('; revoked=2026-12-01'), rec('; revoked=2026-11-01')], fp, '2026-10-15');
  assert.equal(revoked.record.revoked, '2026-11-01');
});

test('domains are checked before any lookup', () => {
  for (const d of ['cards.example.org', 'example.co.uk', 'example.xn--p1ai']) assert.equal(isValidDomain(d), true, d);
  const long = Array.from({ length: 4 }, () => 'a'.repeat(63)).join('.');
  assert.equal(long.length, 255);
  for (const d of ['', 'localhost', '10.0.0.1', 'Example.org', '-a.org', 'a..org', 'a.org/x', 'a.org.', 'exa mple.org', long.slice(1)]) {
    assert.equal(isValidDomain(d), false, d);
  }
});

test('lookup asks Cloudflare first with the JSON accept header', async () => {
  const { fetchImpl, calls } = fakeDoh({ Status: 0, Answer: [{ type: 16, data: '"x"' }, { type: 5, data: 'y' }] });
  assert.deepEqual(await lookupTxt('cards.example.org', fetchImpl), { resolver: 'cloudflare', records: ['"x"'] });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, 'https://cloudflare-dns.com/dns-query?' + QUERY);
  assert.equal(calls[0].init.headers.accept, 'application/dns-json');
  assert.ok(calls[0].init.signal instanceof AbortSignal);
});

test('NXDOMAIN is an answer, not a failure', async () => {
  const { fetchImpl, calls } = fakeDoh({ Status: 3 });
  assert.deepEqual(await lookupTxt('cards.example.org', fetchImpl), { resolver: 'cloudflare', records: [] });
  assert.equal(calls.length, 1);
});

test('an empty NOERROR answer is an answer, not a failure', async () => {
  // Cloudflare and Google send this NODATA reply, with no Answer field, under a wildcard or an empty non-terminal.
  const { fetchImpl, calls } = fakeDoh({ Status: 0, Question: [{ name: DNS_NAME, type: 16 }] });
  assert.deepEqual(await lookupTxt('cards.example.org', fetchImpl), { resolver: 'cloudflare', records: [] });
  assert.equal(calls.length, 1);
});

test('lookup refuses a bad domain and an answer for another name', async () => {
  for (const d of ['cards.example.org&name=_perf-sentinel.evil.com', 'Cards.example.org', 'localhost']) {
    const { fetchImpl, calls } = fakeDoh();
    assert.deepEqual(await lookupTxt(d, fetchImpl), { resolver: null }, d);
    assert.equal(calls.length, 0, d);
  }
  const evil = { Status: 0, Question: [{ name: '_perf-sentinel.evil.com', type: 16 }], Answer: [{ name: '_perf-sentinel.evil.com', type: 16, data: rec('') }] };
  const lookup = async (...replies) => {
    const doh = fakeDoh(...replies);
    const result = await lookupTxt('cards.example.org', doh.fetchImpl);
    for (const { url } of doh.calls) assert.ok(url.endsWith('?' + QUERY), url);
    return { result, calls: doh.calls.length };
  };
  assert.deepEqual(await lookup(evil, evil), { result: { resolver: null }, calls: 2 });
  assert.deepEqual(
    await lookup(
      { Status: 3, Question: [{ name: '_perf-sentinel.evil.com.', type: 16 }] },
      { Status: 0, Answer: [{ name: '_PERF-SENTINEL.Cards.Example.Org.', type: 16, data: 'x' }] },
    ),
    { result: { resolver: 'google', records: ['x'] }, calls: 2 },
  );
  const google = { Status: 0, Answer: [{ type: 16, data: 'g' }] };
  const fellBack = { result: { resolver: 'google', records: ['g'] }, calls: 2 };
  assert.deepEqual(await lookup({ Status: 0, Answer: [{ type: 16, data: 'x', name: undefined }] }, google), fellBack);
  assert.deepEqual(
    await lookup({ Status: 0, Answer: [{ name: DNS_NAME, type: 5, data: 'target.example.net.' }, { name: 'target.example.net', type: 16, data: rec('') }] }),
    { result: { resolver: 'cloudflare', records: [rec('')] }, calls: 1 },
  );
  const chain = (hops) => {
    const names = [DNS_NAME, ...Array.from({ length: hops }, (_, i) => `h${i + 1}.example.net`)];
    const cnames = names.slice(1).map((target, i) => ({ name: names[i], type: 5, data: target + '.' }));
    return { Status: 0, Answer: [...cnames, { name: names[hops], type: 16, data: 'deep' }] };
  };
  assert.deepEqual(await lookup(chain(8)), { result: { resolver: 'cloudflare', records: ['deep'] }, calls: 1 });
  assert.deepEqual(await lookup(chain(9), google), fellBack);
  const stray = { Status: 0, Answer: [{ name: DNS_NAME, type: 5, data: 'target.example.net' }, { name: 'other.example.net', type: 16, data: rec('') }] };
  assert.deepEqual(await lookup(stray, google), fellBack);
  const strayCname = { Status: 0, Answer: [{ type: 16, data: rec('') }, { name: 'other.example.net', type: 5, data: 'x.example.net' }] };
  assert.deepEqual(await lookup(strayCname, google), fellBack);
});

test('a failing Cloudflare falls back to Google once', async () => {
  for (const failure of [503, new TypeError('network'), { Status: 2 }]) {
    const { fetchImpl, calls } = fakeDoh(failure, { Status: 0, Answer: [{ type: 16, data: 'x' }] });
    assert.deepEqual(await lookupTxt('cards.example.org', fetchImpl), { resolver: 'google', records: ['x'] });
    assert.equal(calls.length, 2);
    assert.equal(calls[1].url, 'https://dns.google/resolve?' + QUERY);
    assert.equal(calls[1].init.headers, undefined);
    assert.ok(calls[1].init.signal instanceof AbortSignal);
  }
});

test('both resolvers failing reads as unavailable', async () => {
  for (const replies of [[new TypeError('a'), new TypeError('b')], ['not json', '<html>']]) {
    const { fetchImpl } = fakeDoh(...replies);
    assert.deepEqual(await lookupTxt('cards.example.org', fetchImpl), { resolver: null });
  }
});

test('a single DNS source has no partial state', async () => {
  const { fetchImpl } = fakeDoh(503, { Status: 0, Answer: [{ type: 16, data: rec('; revoked=2026-12-01') }, { type: 16, data: rec('') }] });
  const found = await lookupTxt('cards.example.org', fetchImpl);
  assert.deepEqual(found, { resolver: 'google', records: [rec('; revoked=2026-12-01'), rec('')] });
  assert.equal(bindingState(found.records, fp, '2026-10-15').state, 'revoked');
  assert.deepEqual(await lookupTxt('cards.example.org', fakeDoh(503, 503).fetchImpl), { resolver: null });
});

test('every signed key has a label in both languages', async () => {
  const cards = [];
  for (const name of [...ANGLES, 'nodomain']) {
    const full = await openCard(parseEnvelope(readPngCard(fixture(fx.cards[name].png))), 'full');
    const compact = await openCard(parseFragment('#c=' + tokenOf(fx.cards[name].compact_url)), 'compact');
    assert.equal(full.state, 'ok', name);
    assert.equal(compact.state, 'ok', name);
    cards.push(full.card, compact.card);
  }
  const listed = ['angle', 'lang', 'period', 'kind', 'from', 'to', 'scope', 'figures', 'story', 'before_fix_period',
    'equivalence', 'id', 'per_unit_ug', 'unit', 'source', 'vintage', 'free', 'title', 'team', 'label', 'org', 'name',
    'domain', 'key', 'public_key', 'generated_at_ms', 'generated_on', 'last_waste_permille', 'previous_waste_permille'];
  const enums = { angle: 'angles', lang: 'langs', 'period.kind': 'kinds', 'story.finding_type': 'findingTypes', 'equivalence.unit': 'units' };
  for (const lang of ['en', 'fr']) {
    const { fields } = COPY[lang];
    for (const key of listed) assert.equal(typeof fields[key], 'string', `${lang} ${key}`);
    for (const card of cards) {
      for (const path of leafPaths(card)) {
        assert.equal(typeof fields[path.split('.').at(-1)], 'string', `${lang} ${path}`);
        if (Object.hasOwn(enums, path)) {
          const value = path.split('.').reduce((o, k) => o[k], card);
          assert.ok(Object.hasOwn(COPY[lang][enums[path]], value), `${lang} ${path}=${value}`);
        }
      }
    }
  }
});

// ---- the file content check and the signed figures (Task 5b) ----

const READ = { png: readPngCard, svg: readSvgCard, jxl: readJxlCard, pdf: readPdfCard };
const FORMATS = ['png', 'svg', 'jxl', 'pdf'];
const openFileCard = async (format, bytes) => openCard(parseEnvelope(await READ[format](bytes)), 'full', undefined, { format, bytes });
const storyFile = (format) => fixture(fx.cards.story[format]);
const envelopeOf = async (format, bytes) => parseEnvelope(await READ[format](bytes));
/** Byte offset of `needle` in `bytes`, by a plain byte search. */
const find = (bytes, needle, from = 0) => Buffer.from(bytes.buffer, bytes.byteOffset, bytes.length).indexOf(Buffer.from(needle), from);
const withByte = (bytes, at, xor = 1) => {
  const out = bytes.slice();
  out[at] ^= xor;
  return out;
};
const CRC_OF = (bytes, type) => pngChunks(bytes).find((c) => c.type === type).end - 1;

test('the content message matches the engine', async () => {
  assert.equal(dec(await contentMessage('png', enc('{}'), enc('abc'))),
    'perf-sentinel-card-content-v1\npng\n44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a\n'
    + 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
});

test('each original file reads intact', async () => {
  for (const format of FORMATS) {
    const r = await openFileCard(format, storyFile(format));
    assert.equal(r.state, 'ok', format);
    assert.equal(r.content, 'intact', format);
  }
  const link = await openCard(storyPair(), 'compact');
  assert.equal(link.state, 'ok');
  assert.equal(link.content, 'not_checked');
});

test('a copied envelope reads modified', async () => {
  for (const format of FORMATS) {
    assert.deepEqual(await openFileCard(format, fixture(fx.swapped[format])), { state: 'modified' }, format);
  }
});

test('the content ignores only the envelope', async () => {
  const sigText = async (format, bytes) => b64urlEncode((await envelopeOf(format, bytes)).contentSig);
  const png = storyFile('png');
  assert.deepEqual(cardContent('png', png, await sigText('png', png)), stripChunk(png, 'iTXt'));

  const svg = storyFile('svg');
  const open = find(svg, '<metadata id="perf-sentinel-card">');
  const close = find(svg, '</metadata>', open) + '</metadata>'.length;
  assert.deepEqual(cardContent('svg', svg, await sigText('svg', svg)), concat(svg.subarray(0, open), svg.subarray(close)));

  const jxl = storyFile('jxl');
  const parts = jxlParts(jxl);
  assert.deepEqual(cardContent('jxl', jxl, await sigText('jxl', jxl)), concat(parts['JXL '], parts.ftyp, parts.jxlc));

  const pdf = storyFile('pdf');
  const content = cardContent('pdf', pdf, await sigText('pdf', pdf));
  assert.equal(content.length, pdf.length);
  const changed = [...pdf.keys()].filter((i) => pdf[i] !== content[i]);
  assert.ok(changed.length > 0 && changed.length <= 86);
  const slot = find(pdf, '{"content_sig":"') + 16;
  assert.ok(changed.every((i) => i >= slot && i < slot + 86));
  assert.deepEqual([...content.subarray(slot, slot + 86)], Array(86).fill(0x41));
  assert.equal(cardContent('pdf', pdf, 'A'.repeat(86)), null);

  const appended = {
    png: concat(png, Uint8Array.of(0)),
    svg: concat(svg, enc(' ')),
    jxl: concat(jxl, jxlBox('free', new Uint8Array(0))),
    pdf: concat(pdf, enc('\n')),
  };
  for (const format of FORMATS) assert.deepEqual(await openFileCard(format, appended[format]), { state: 'modified' }, format);
});

test('PNG envelope chunks are read once and before the image data', () => {
  const png = storyFile('png');
  const card = pngChunks(png).find((c) => c.type === 'iTXt');
  const iend = pngChunks(png).find((c) => c.type === 'IEND');
  const twice = concat(png.subarray(0, iend.start), png.subarray(card.start, card.end), png.subarray(iend.start));
  assert.equal(readPngCard(twice), null);
  assert.equal(readPngCard(moveChunk(png, 'iTXt', 'IDAT')), null);
  const long = png.slice();
  new DataView(long.buffer).setUint32(card.start, 0x7ffffff0);
  assert.equal(readPngCard(long), null);
});

/** The story PNG cases of plan 2 Task 15 `the_png_walk_follows_the_shared_byte_rules`. */
async function pngWalkCases() {
  const png = storyFile('png');
  const text = readPngCard(png);
  const last = pngChunks(png).filter((c) => c.type === 'IDAT').at(-1);
  return {
    unread: {
      'first byte': withByte(png, 0),
      'envelope before IHDR': moveChunk(png, 'iTXt', null),
      'no IEND': stripChunk(png, 'IEND'),
      'cut after the last IDAT': png.subarray(0, last.end),
      'envelope CRC': withByte(png, CRC_OF(png, 'iTXt')),
      'compression flag 1': replaceChunk(png, 'iTXt', cardChunk(text, { flag: 1 })),
      'compression method 1': replaceChunk(png, 'iTXt', cardChunk(text, { method: 1 })),
      'language tag': replaceChunk(png, 'iTXt', cardChunk(text, { lang: 'en' })),
      'translated keyword': replaceChunk(png, 'iTXt', cardChunk(text, { translated: 'x' })),
      '1 KiB language tag': replaceChunk(png, 'iTXt', cardChunk(text, { lang: 'a'.repeat(1024) })),
    },
    modified: {
      'bytes after IEND': concat(png, enc('retouched')),
      'IDAT CRC': withByte(png, CRC_OF(png, 'IDAT')),
    },
  };
}

test('the PNG walk follows the engine\'s byte rules', async () => {
  const { unread, modified } = await pngWalkCases();
  for (const [name, bytes] of Object.entries(unread)) assert.equal(readPngCard(bytes), null, name);
  for (const [name, bytes] of Object.entries(modified)) {
    assert.equal(typeof readPngCard(bytes), 'string', name);
    assert.deepEqual(await openFileCard('png', bytes), { state: 'modified' }, name);
  }
});

test('a moved carrier is refused', () => {
  const svg = dec(storyFile('svg'));
  const element = /<metadata id="perf-sentinel-card">[\s\S]*?<\/metadata>/.exec(svg)[0];
  const inTitle = enc(svg.replace(element + '\n', '').replace('</title>', element + '</title>'));
  assert.equal(readSvgCard(inTitle), null);
  assert.equal(svgEnvelopeRange(inTitle), null);

  const parts = jxlParts(storyFile('jxl'));
  const payload = parts.pscd.subarray(8);
  const wide = new Uint8Array(16);
  new DataView(wide.buffer).setUint32(0, 1);
  wide.set(enc('pscd'), 4);
  new DataView(wide.buffer).setUint32(12, 16 + payload.length);
  for (const [name, bytes] of Object.entries({
    'pscd after jxlc': concat(parts['JXL '], parts.ftyp, parts.jxlc, parts.pscd),
    '64-bit pscd size': concat(parts['JXL '], parts.ftyp, wide, payload, parts.jxlc),
  })) {
    assert.ok(jxlBoxes(bytes), name);
    assert.equal(readJxlCard(bytes), null, name);
    assert.equal(jxlEnvelopeRange(bytes), null, name);
  }
});

test('a non-canonical envelope never reads intact', async () => {
  const png = storyFile('png');
  const svg = dec(storyFile('svg'));
  const env = readSvgCard(storyFile('svg'));
  const parts = jxlParts(storyFile('jxl'));
  const jxlEnv = readJxlCard(storyFile('jxl'));
  const cases = {
    'png reordered': ['png', replaceChunk(png, 'iTXt', cardChunk(reorderEnvelope(readPngCard(png))))],
    'png space': ['png', replaceChunk(png, 'iTXt', cardChunk(spaceEnvelope(readPngCard(png))))],
    'svg reordered': ['svg', enc(svg.replace(env, reorderEnvelope(env)))],
    'jxl space': ['jxl', concat(parts['JXL '], parts.ftyp, jxlBox('pscd', enc(spaceEnvelope(jxlEnv))), parts.jxlc)],
  };
  for (const [name, [format, bytes]] of Object.entries(cases)) {
    assert.notEqual(await READ[format](bytes), null, name);
    assert.equal(parseEnvelope(await READ[format](bytes)), null, name);
  }
});

test('a prepended BOM is content', async () => {
  const bytes = concat(Uint8Array.of(0xef, 0xbb, 0xbf), enc('\n'), storyFile('svg'));
  assert.equal(typeof readSvgCard(bytes), 'string');
  assert.deepEqual(await openFileCard('svg', bytes), { state: 'modified' });
});

test('a PDF with a second slot reads modified', async () => {
  const bytes = concat(storyFile('pdf'), enc('{"content_sig":"' + 'A'.repeat(86)));
  assert.deepEqual(await openFileCard('pdf', bytes), { state: 'modified' });
});

test('big figures are the ones the QR signs', async () => {
  const ORDER = ['cited_ops', 'cited_ms', 'before_fix_ops', 'threshold_ops', 'threshold_ms', 'last_waste_permille',
    'previous_waste_permille', 'confirmed_fixes_count', 'services_count'];
  const expected = {
    story: [['cited_ops', 41], ['before_fix_ops', 1937000]],
    slow: [['cited_ms', 2349], ['threshold_ms', 500]],
    curve: [['last_waste_permille', 31], ['previous_waste_permille', 48]],
    collective: [['confirmed_fixes_count', 7], ['services_count', 3]],
  };
  for (const angle of ANGLES) {
    const full = await openCard(parseEnvelope(readPngCard(fixture(fx.cards[angle].png))), 'full');
    const compact = await openCard(parseFragment('#c=' + tokenOf(fx.cards[angle].compact_url)), 'compact');
    // verify-card prints `angle` and the figures flat: read them as bigFigures does.
    const { angle: _, period, before_fix_period: before, ...flat } = fx.cards[angle].figures;
    const engine = { period, ...(before && { before_fix_period: before }), values: ORDER.filter((k) => k in flat).map((k) => [k, flat[k]]) };
    assert.deepEqual(bigFigures(full.card), bigFigures(compact.card), angle);
    assert.deepEqual(bigFigures(full.card), engine, angle);
    assert.deepEqual(bigFigures(full.card).values, expected[angle], angle);
  }
});
