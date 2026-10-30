// Share card tokens: strict base64url, Ed25519 signatures, compact payloads and
// the values the page prints. Mirrors the engine's report/card (C3 frozen).
import { COPY } from './copy.js';
import { cardContent } from './files.js';

export const NOBLE_URL = '/assets/vendor/noble-ed25519-3.2.0.js';
export const NOBLE_SHA384 = 'sha384-Mn3Pw6kF6BnK6mrt9jgociul6uWPooS/UN1RxwkhSN6lFlyQF33h3c9Dx+TKoIG3';

const MS_PER_DAY = 86_400_000;
// 9999-12-31, so every epoch day below prints as YYYY-MM-DD.
const MAX_DAY = 2_932_896;

const isObject = (o) => typeof o === 'object' && o !== null && !Array.isArray(o);
const isString = (s) => typeof s === 'string';
const isDay = (d) => Number.isSafeInteger(d) && d >= 0 && d <= MAX_DAY;
const isoOfDay = (d) => new Date(d * MS_PER_DAY).toISOString().slice(0, 10);
const lookup = (table, key) => (Object.hasOwn(table, key) ? table[key] : undefined);

// ---- base64url ----

export function b64urlEncode(bytes) {
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** Strict and canonical: one text per byte string, as the engine's `b64url_decode`. */
export function b64urlDecode(s) {
  if (!isString(s) || !/^[A-Za-z0-9_-]*$/.test(s) || s.length % 4 === 1) {
    throw new Error('not canonical base64url');
  }
  const bin = atob(s.replace(/-/g, '+').replace(/_/g, '/'));
  const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
  // Re-encoding catches non-zero trailing bits, which atob ignores.
  if (b64urlEncode(bytes) !== s) throw new Error('not canonical base64url');
  return bytes;
}

const tryDecode = (s) => {
  try {
    return b64urlDecode(s);
  } catch {
    return null;
  }
};

// ---- carriers ----

export function parseToken(text) {
  if (!isString(text)) return null;
  const parts = text.split('.');
  if (parts.length !== 2) return null;
  const payload = tryDecode(parts[0]);
  const sig = tryDecode(parts[1]);
  return payload && sig?.length === 64 ? { payload, sig } : null;
}

export function parseEnvelope(text) {
  if (!isString(text)) return null;
  let o;
  try {
    o = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isObject(o) || Object.keys(o).sort().join() !== 'content_sig,payload,signature') return null;
  if (![o.content_sig, o.payload, o.signature].every(isString)) return null;
  // Byte-canonical: reordered keys, whitespace or an escape give another text.
  if (text !== `{"content_sig":"${o.content_sig}","payload":"${o.payload}","signature":"${o.signature}"}`) return null;
  const payload = tryDecode(o.payload);
  const sig = tryDecode(o.signature);
  const contentSig = tryDecode(o.content_sig);
  return payload && sig?.length === 64 && contentSig?.length === 64 ? { payload, sig, contentSig } : null;
}

export function parseFragment(hash) {
  if (!isString(hash) || hash.length > 4096 || !hash.startsWith('#c=')) return null;
  return parseToken(hash.slice(3));
}

export async function fingerprint(pub) {
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', pub));
  return 'ps1:' + b64urlEncode(digest.subarray(0, 16));
}

// ---- signatures ----

export class EngineUnavailable extends Error {}

export async function defaultLoadNoble() {
  try {
    const res = await fetch(NOBLE_URL);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const bytes = new Uint8Array(await res.arrayBuffer());
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-384', bytes));
    if ('sha384-' + btoa(String.fromCharCode(...digest)) !== NOBLE_SHA384) throw new Error('noble hash mismatch');
    const url = URL.createObjectURL(new Blob([bytes], { type: 'text/javascript' }));
    try {
      return await import(url);
    } finally {
      URL.revokeObjectURL(url);
    }
  } catch (e) {
    throw new EngineUnavailable('the Ed25519 fallback could not load', { cause: e });
  }
}

/** WebCrypto first; noble only when the browser has no Ed25519. A WebCrypto `false` is final. */
export async function verifyEd25519(pub, sig, msg, loadNoble = defaultLoadNoble) {
  let key;
  try {
    key = await crypto.subtle.importKey('raw', pub, { name: 'Ed25519' }, false, ['verify']);
  } catch {
    let noble;
    try {
      noble = await loadNoble();
    } catch (e) {
      throw e instanceof EngineUnavailable ? e : new EngineUnavailable('the Ed25519 fallback could not load', { cause: e });
    }
    try {
      return await noble.verifyAsync(sig, msg, pub, { zip215: false });
    } catch {
      return false;
    }
  }
  return crypto.subtle.verify({ name: 'Ed25519' }, key, sig, msg);
}

// ---- payloads ----

const ANGLES = { s: 'story', c: 'curve', k: 'collective' };
const KINDS = { w: 'week', m: 'month', q: 'quarter', h: 'semester', y: 'year' };
const FIGURES = {
  story: { o: 'cited_ops', m: 'cited_ms', b: 'before_fix_ops', t: 'threshold_ops', u: 'threshold_ms' },
  curve: { w: 'last_waste_permille', x: 'previous_waste_permille' },
  collective: { n: 'confirmed_fixes_count', e: 'services_count' },
};
// The engine's compact `f` key sets per angle, sorted.
const FIGURE_SETS = { story: ['o', 'b,o', 'o,t', 'm', 'm,u'], curve: ['w,x'], collective: ['e,n'] };
const COMPACT_KEYS = new Set(['v', 'a', 'p', 'f', 'r', 'd', 'k', 'g']);

/** The compact payload in the full payload's shape, `null` on anything it does not know. */
export function expandCompact(c) {
  if (!isObject(c) || !Object.keys(c).every((k) => COMPACT_KEYS.has(k)) || c.v !== 1) return null;
  const angle = lookup(ANGLES, c.a);
  const { p, f, r } = c;
  if (!angle || !Array.isArray(p) || p.length !== 3 || !isObject(f) || !isString(c.k) || !isDay(c.g)) return null;
  const kind = lookup(KINDS, p[0]);
  if (!kind || !isDay(p[1]) || !isDay(p[2]) || p[1] > p[2]) return null;
  const keys = Object.keys(f).sort();
  if (!FIGURE_SETS[angle].includes(keys.join()) || !keys.every((k) => Number.isSafeInteger(f[k]) && f[k] >= 0)) return null;
  const card = {
    v: 1,
    angle,
    period: { kind, from: isoOfDay(p[1]), to: isoOfDay(p[2]) },
    figures: Object.fromEntries(keys.map((k) => [FIGURES[angle][k], f[k]])),
    public_key: c.k,
    generated_on: isoOfDay(c.g),
  };
  if (r !== undefined) {
    if (angle === 'curve' || !Array.isArray(r) || r.length !== 2 || !isDay(r[0]) || !isDay(r[1])) return null;
    if (r[0] > r[1] || r[1] > p[2]) return null;
    card.before_fix_period = { from: isoOfDay(r[0]), to: isoOfDay(r[1]) };
  }
  if (c.d !== undefined) {
    if (!isString(c.d) || c.d === '') return null;
    card.org = { domain: c.d };
  }
  return card;
}

// The keys the page reads, checked after the signature: anyone can sign any JSON with their own key.
const isIsoDay = (s) => isString(s) && /^\d{4}-\d{2}-\d{2}$/.test(s);

function readableFull(card) {
  const { period, org, generated_at_ms: at, before_fix_period: before } = card;
  return isString(card.angle)
    && isObject(period) && isString(period.kind) && isIsoDay(period.from) && isIsoDay(period.to)
    && (before === undefined || (isObject(before) && isIsoDay(before.from) && isIsoDay(before.to)))
    && isObject(org) && isString(org.name) && org.name !== ''
    && (org.domain === undefined || isString(org.domain))
    && Number.isSafeInteger(at) && at >= 0 && at <= 8.64e15;
}

const hex = (bytes) => Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
const sha256Hex = async (bytes) => hex(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)));

/** What `content_sig` signs: context, format, SHA-256 of the payload, SHA-256 of the content. */
export async function contentMessage(format, payload, content) {
  return new TextEncoder().encode(`perf-sentinel-card-content-v1\n${format}\n${await sha256Hex(payload)}\n${await sha256Hex(content)}`);
}

/** `file` is `{format, bytes}` for a dropped file: its content is checked against `contentSig`. */
export async function openCard(pair, kind, loadNoble = defaultLoadNoble, file = undefined) {
  let parsed;
  try {
    parsed = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(pair.payload));
  } catch {
    return { state: 'tampered' };
  }
  if (!isObject(parsed)) return { state: 'tampered' };
  if (Object.hasOwn(parsed, 'v') && parsed.v !== 1) return { state: 'unsupported' };
  const pub = tryDecode(kind === 'compact' ? parsed.k : parsed.public_key);
  if (pub?.length !== 32) return { state: 'tampered' };
  try {
    if (!(await verifyEd25519(pub, pair.sig, pair.payload, loadNoble))) return { state: 'tampered' };
  } catch (e) {
    if (e instanceof EngineUnavailable) return { state: 'cannot-verify' };
    throw e;
  }
  const fp = await fingerprint(pub);
  if (kind === 'compact') {
    const card = expandCompact(parsed);
    return card ? { state: 'ok', card, fingerprint: fp, date: card.generated_on, content: 'not_checked' } : { state: 'unsupported' };
  }
  if (parsed.key !== fp) return { state: 'unsupported' };
  if (file) {
    const content = cardContent(file.format, file.bytes, b64urlEncode(pair.contentSig));
    try {
      if (!content || !(await verifyEd25519(pub, pair.contentSig, await contentMessage(file.format, pair.payload, content), loadNoble))) {
        return { state: 'modified' };
      }
    } catch (e) {
      if (e instanceof EngineUnavailable) return { state: 'cannot-verify' };
      throw e;
    }
  }
  if (!readableFull(parsed)) return { state: 'unsupported' };
  return {
    state: 'ok', card: parsed, fingerprint: fp, date: new Date(parsed.generated_at_ms).toISOString().slice(0, 10),
    content: file ? 'intact' : 'not_checked',
  };
}

// The figures the QR code signs, per angle, in the order the page shows them (the engine's `compact_figures`).
const BIG_FIGURES = {
  story: ['cited_ops', 'cited_ms', 'before_fix_ops', 'threshold_ops', 'threshold_ms'],
  curve: ['last_waste_permille', 'previous_waste_permille'],
  collective: ['confirmed_fixes_count', 'services_count'],
};

/** The signed figures to compare with the image, the same object for a file and its link. */
export function bigFigures(card) {
  const figures = isObject(card.figures) ? card.figures : {};
  const waste = Array.isArray(figures.waste_permille) ? figures.waste_permille : [];
  const all = { last_waste_permille: waste.at(-1), previous_waste_permille: waste.at(-2), ...figures };
  const names = Object.hasOwn(BIG_FIGURES, card.angle) ? BIG_FIGURES[card.angle] : [];
  const out = { period: card.period };
  if (isObject(card.before_fix_period)) out.before_fix_period = card.before_fix_period;
  out.values = names.filter((n) => Number.isSafeInteger(all[n]) && all[n] >= 0).map((n) => [n, all[n]]);
  return out;
}

// ---- values, as the card prints them ----

/** Two significant figures, rounded down: the engine's `printed_co2_ug` and `floor_two`. */
export function printedCo2Ug(ug) {
  let scale = 1;
  while (ug / scale >= 100) scale *= 10;
  return Math.floor(ug / scale) * scale;
}

export const equivalenceCount = (co2Ug, perUnitUg) => Math.floor(printedCo2Ug(co2Ug) / perUnitUg);

const num = (lang, n, options) => new Intl.NumberFormat(lang, options).format(n);
const TWO_DOWN = { maximumSignificantDigits: 2, roundingMode: 'trunc' };
const ymd = (iso) => iso.split('-').map(Number);

export function formatDay(iso, lang) {
  const [y, m, d] = ymd(iso);
  const c = COPY[lang];
  return `${d === 1 ? c.firstOfMonth : d} ${c.months[m - 1]} ${y}`;
}

export function periodName(kind, from, lang) {
  const c = COPY[lang];
  const [y, m] = ymd(from);
  switch (kind) {
    case 'week': return c.week.replace('{day}', formatDay(from, lang));
    case 'month': return `${c.months[m - 1]} ${y}`;
    case 'quarter': return c.quarter.replace('{n}', Math.floor((m - 1) / 3) + 1).replace('{y}', y);
    case 'semester': return c.halves[m > 6 ? 1 : 0].replace('{y}', y);
    case 'year': return String(y);
    default: return from;
  }
}

// `before_fix_period` carries no kind: it is the card's, readable from the span.
function spanKind({ from, to }) {
  if ((Date.parse(to) - Date.parse(from)) / MS_PER_DAY === 6) return 'week';
  const [fy, fm] = ymd(from);
  const [ty, tm] = ymd(to);
  return { 1: 'month', 3: 'quarter', 6: 'semester', 12: 'year' }[ty * 12 + tm - (fy * 12 + fm) + 1];
}

function count(n, lang) {
  if (n < 10_000) return num(lang, n);
  const { thousand, million, billion } = COPY[lang].scale;
  const [scale, forms] = n >= 1e9 ? [1e9, billion] : n >= 1e6 ? [1e6, million] : [1e3, thousand];
  const floored = printedCo2Ug(n);
  // French takes the singular below 2, English for exactly one.
  const singular = lang === 'fr' ? floored < 2 * scale : floored === scale;
  return `${num(lang, floored / scale, TWO_DOWN)} ${forms[singular ? 0 : 1]}`;
}

function mass(ug, lang) {
  const printed = printedCo2Ug(ug);
  const [unit, symbol] = printed >= 1e12 ? [1e12, 't'] : printed >= 1e9 ? [1e9, 'kg'] : [1e6, 'g'];
  return `${COPY[lang].atLeast} ${num(lang, printed / unit, TWO_DOWN)} ${symbol}`;
}

const seconds = (ms, lang) => (ms < 100
  ? `${num(lang, ms)} ms`
  : `${num(lang, Math.floor(ms / 100) / 10, { maximumFractionDigits: 1 })} s`);

const permille = (p, lang) => (p === 0
  ? COPY[lang].underTenthPercent
  : num(lang, p / 1000, { style: 'percent', maximumFractionDigits: 1 }));

function dateTime(ms, lang) {
  const iso = new Date(ms).toISOString();
  const [hh, mm] = [iso.slice(11, 13), iso.slice(14, 16)];
  const time = COPY[lang].utcTime.replace('{HH}', hh).replace('{H}', Number(hh)).replace('{mm}', mm);
  return `${formatDay(iso.slice(0, 10), lang)}, ${time}`;
}

const ENUMS = { 'period.kind': 'kinds', angle: 'angles', lang: 'langs', 'story.finding_type': 'findingTypes' };
const DAYS = new Set(['story.observed_at', 'story.done_at', 'generated_on']);

/** A signed value in words, never rounded up and never more precise than the card. */
export function formatValue(key, value, lang) {
  if (Array.isArray(value)) return value.map((v) => formatValue(key, v, lang)).join(', ');
  if (key === 'period' || key === 'before_fix_period') {
    return periodName(value.kind ?? spanKind(value), value.from, lang);
  }
  const last = key.split('.').at(-1);
  if (typeof value === 'number') {
    if (last.endsWith('_at_ms')) return dateTime(value, lang);
    if (last.endsWith('_ug')) return mass(value, lang);
    if (last.endsWith('_ops') || last.endsWith('_count')) return count(value, lang);
    if (last.endsWith('_ms')) return seconds(value, lang) + (key === 'figures.cited_ms' ? ` ${COPY[lang].orMore}` : '');
    if (last.endsWith('_permille')) return permille(value, lang);
    return num(lang, value);
  }
  if (DAYS.has(key)) return formatDay(value, lang);
  if (Object.hasOwn(ENUMS, key)) return lookup(COPY[lang][ENUMS[key]], value) ?? String(value);
  return String(value);
}

/** Dotted paths of every leaf, an array counting as one leaf. */
export function leafPaths(card, prefix = '') {
  return Object.entries(card).flatMap(([k, v]) => (isObject(v) ? leafPaths(v, `${prefix}${k}.`) : [prefix + k]));
}
