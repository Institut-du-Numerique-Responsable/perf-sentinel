// The /verifier page: reads the card from the URL fragment or a dropped original file,
// then shows one state in #verifier-result. Every payload string goes through text nodes.
import { COPY } from './copy.js';
import { parseFragment, parseEnvelope, openCard, formatValue, leafPaths, periodName, equivalenceCount } from './card.js';
import { readPngCard, readPdfCard, readSvgCard, readJxlCard } from './files.js';
import { isValidDomain } from './dns.js';

const lang = document.documentElement.lang === 'fr' ? 'fr' : 'en';
const T = COPY[lang];
const MAX_FILE = 16 * 1024 * 1024;

const $ = (s) => document.querySelector(s);
const result = $('#verifier-result');
const dropZone = $('#verifier-drop');
const input = $('#verifier-file');
const phone = $('#verifier-phone');
const limits = $('#verifier-limits');
const langSwitch = $('[data-lang-switch]');
const langBase = langSwitch?.getAttribute('href');

const isObject = (o) => typeof o === 'object' && o !== null && !Array.isArray(o);
const filled = (s) => typeof s === 'string' && s !== '';
const fill = (s, vars) => s.replace(/\{(\w+)\}/g, (m, k) => (Object.hasOwn(vars, k) ? vars[k] : m));
const get = (o, path) => path.split('.').reduce((v, k) => (isObject(v) ? v[k] : undefined), o);

function el(tag, attrs, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs ?? {})) if (v !== undefined && v !== false) e.setAttribute(k, v === true ? '' : v);
  e.append(...children.filter((c) => c !== null && c !== undefined && c !== false));
  return e;
}
const icon = (name) => el('span', { 'data-ico': name, 'aria-hidden': 'true' });
const limitsButton = () => el('button', { class: 'vf-lnk', type: 'button', 'data-open-limits': true }, T.limitsLink);

function show(state, ...nodes) {
  result.dataset.state = state;
  result.replaceChildren(...nodes);
  const loaded = state !== 'empty';
  dropZone.toggleAttribute('data-compact', loaded);
  phone.hidden = loaded;
}

function head(headline, iconName, ...extra) {
  return el('div', { class: 'vf-hd' }, el('div', null, headline, ...extra), iconName && icon(iconName));
}
const headline = (...text) => el('p', { class: 'vf-hl', id: 'verifier-headline' }, ...text);

// state: [copy key, icon, limits control, line under the headline]
const REFUSALS = {
  'invalid-link': ['invalidLink', 'link-2-off', true],
  tampered: ['tampered', 'shield-alert', true, () => el('p', { class: 'vf-next', id: 'verifier-next' }, T.tamperedNext)],
  unsupported: ['unsupported', 'circle-alert'],
  'cannot-verify': ['cannotVerify', 'circle-alert'],
  'no-card-data': ['noCardData', 'image-off', true, () => el('p', { class: 'vf-phone' }, T.phone)],
  'file-too-large': ['fileTooLarge', 'file-x'],
  'file-type': ['fileType', 'file-x'],
};

function refuse(state) {
  const [key, ico, withLimits, line] = REFUSALS[state];
  show(state, head(headline(T[key]), ico, line?.(), withLimits && limitsButton()));
}

// ---- level 1: the signed values ----

const STEP_MONTHS = { month: 1, quarter: 3, semester: 6, year: 12 };

/** The `from` day of the period `steps` periods away, or null for a kind it does not know. */
function shiftPeriod(kind, from, steps) {
  const [y, m, d] = from.split('-').map(Number);
  if (kind === 'week') return new Date(Date.UTC(y, m - 1, d + 7 * steps)).toISOString().slice(0, 10);
  if (!STEP_MONTHS[kind]) return null;
  return new Date(Date.UTC(y, m - 1 + steps * STEP_MONTHS[kind], 1)).toISOString().slice(0, 10);
}

function namePeriod(card, steps = 0) {
  const { kind, from } = card.period;
  const day = shiftPeriod(kind, from, steps);
  return day ? periodName(kind, day, lang) : '';
}

const ORDER = [
  'angle', 'period', 'figures.cited_ops', 'figures.cited_ms', 'figures.waste_permille', 'figures.last_waste_permille',
  'figures.previous_waste_permille', 'figures.avoidable_ops', 'figures.confirmed_fixes_count', 'figures.services_count',
  'figures.previous_fixes_count', 'figures.fixes_by_family_count', 'figures.before_fix_ops', 'before_fix_period',
  'figures.threshold_ops', 'figures.threshold_ms', 'lang',
  'free.title', 'free.team', 'free.label',
  'scope.environment', 'scope.sources', 'scope.covered_days', 'scope.restart_days', 'scope.sampled_permille',
  'story.finding_type', 'story.service', 'story.endpoint', 'story.observed_at', 'story.done_at',
  'figures.co2_low_ug', 'equivalence.unit', 'equivalence.per_unit_ug', 'equivalence.id', 'equivalence.source', 'equivalence.vintage',
  'org.name', 'org.domain', 'key', 'public_key', 'generated_at_ms', 'generated_on', 'v',
];
const rank = (path) => (ORDER.includes(path) ? ORDER.indexOf(path) : ORDER.length);
// One row for these objects, as the approved mockup shows them.
const WHOLE = ['period', 'before_fix_period', 'figures.fixes_by_family_count'];
const MONO = new Set(['key', 'public_key', 'story.service', 'story.endpoint', 'equivalence.id']);

function groupOf(path) {
  const top = path.split('.')[0];
  if (path === 'figures.co2_low_ug' || top === 'equivalence') return 'co2';
  if (top === 'free') return 'free';
  if (top === 'scope') return 'cover';
  if (top === 'story') return 'fix';
  if (['org', 'key', 'public_key', 'generated_at_ms', 'generated_on', 'v'].includes(top)) return 'sig';
  return 'card';
}

function labelOf(path, card, compact) {
  const last = path.split('.').at(-1);
  const figures = isObject(card.figures) ? card.figures : {};
  const typed = T.typedFields;
  if (!compact && isObject(card.story)) {
    const type = typeof card.story.finding_type === 'string' ? card.story.finding_type : '';
    if (path === 'figures.cited_ops' && type.startsWith('n_plus_one_')) return typed.nPlusOneCitedOps;
    if (path === 'figures.cited_ms' && type.startsWith('slow_')) return typed.slowCitedMs;
    if (path === 'figures.threshold_ms' || path === 'figures.threshold_ops') return typed.threshold;
    if (path === 'story.done_at' && ('threshold_ms' in figures || 'threshold_ops' in figures)) return typed.thresholdDoneAt;
  }
  const extra = get(figures, 'fixes_by_family_count.extra_calls');
  if (path === 'figures.before_fix_ops' && card.angle === 'collective' && Number.isSafeInteger(extra)) {
    return fill(typed.collectiveBeforeFixOps, { n: formatValue('figures.n', extra, lang) });
  }
  if (!Object.hasOwn(T.fields, last)) return path;
  return fill(T.fields[last], { period: namePeriod(card, last.startsWith('previous_') ? -1 : 0) });
}

/** The period a CO2 figure was measured on: the one before the fix when the card has it. */
const co2Period = (card) => fill(T.inPeriod, { period: formatValue('period', isObject(card.before_fix_period) ? card.before_fix_period : card.period, lang) });

function unitWord(card, n) {
  const forms = T.units[card.equivalence.unit];
  if (!Array.isArray(forms)) return String(card.equivalence.unit);
  return forms[(lang === 'fr' ? n < 2 : n === 1) ? 0 : 1];
}

const list = (rows, plain) => el('ul', { class: 'vf-rates', 'data-plain': plain }, ...rows.map(([k, v]) => el('li', null, el('span', null, k), el('b', null, v))));
const isCount = (v) => Number.isSafeInteger(v) && v >= 0;

function valueOf(path, v, card) {
  const per = get(card, 'equivalence.per_unit_ug');
  const co2 = get(card, 'figures.co2_low_ug');
  if (path === 'figures.fixes_by_family_count' && isObject(v)) {
    const families = Object.keys(T.fields);
    return list(Object.entries(v).sort(([a], [b]) => families.indexOf(a) - families.indexOf(b)).map(([k, n]) => [Object.hasOwn(T.fields, k) ? T.fields[k] : k, formatValue(`${path}.${k}`, n, lang)]), true);
  }
  if (path === 'figures.waste_permille' && Array.isArray(v) && v.every(isCount)) {
    return list(v.map((p, i) => [namePeriod(card, i - (v.length - 1)), formatValue(path, p, lang)]));
  }
  if (path === 'figures.co2_low_ug' && isCount(v)) return formatValue(path, v, lang) + co2Period(card);
  if (path === 'equivalence.unit' && isCount(co2) && isCount(per) && per > 0) {
    const n = equivalenceCount(co2, per);
    return `${T.atLeast} ${n} ${unitWord(card, n)}${co2Period(card)}`;
  }
  if (path === 'equivalence.per_unit_ug' && isCount(v) && v > 0) {
    const [unit, symbol] = v >= 1e9 ? [1e9, 'kg'] : [1e6, 'g'];
    const mass = `${new Intl.NumberFormat(lang, { maximumFractionDigits: 2, roundingMode: 'trunc' }).format(v / unit)} ${symbol}`;
    return fill(T.factor, { unit: unitWord(card, 1), mass });
  }
  return formatValue(path, v, lang);
}

function classOf(path, v) {
  if (MONO.has(path)) return 'vf-m';
  if (path.startsWith('free.')) return 'vf-free';
  if (path === 'figures.waste_permille' || path === 'figures.fixes_by_family_count') return 'vf-big-list';
  if (path.startsWith('figures.') && typeof v === 'number') {
    if (path === 'figures.co2_low_ug') return undefined;
    return path === 'figures.previous_fixes_count' ? 'vf-n' : 'vf-n vf-big';
  }
  return undefined;
}

function row(path, card, compact) {
  const v = get(card, path);
  let shown;
  try {
    shown = valueOf(path, v, card);
  } catch {
    shown = String(v); // a self-signed card can hold values no formatter expects
  }
  return el('div', { 'data-field': path }, el('dt', null, labelOf(path, card, compact)), el('dd', { class: classOf(path, v) }, shown));
}

function details(card, compact) {
  const paths = [...new Set(leafPaths(card).map((p) => WHOLE.find((w) => p.startsWith(w + '.')) ?? p))];
  paths.sort((a, b) => rank(a) - rank(b));
  const groups = { card: [], free: [], cover: [], fix: [], co2: [], sig: [] };
  for (const p of paths) groups[groupOf(p)].push(row(p, card, compact));
  const section = (name, note) => groups[name].length > 0 && [
    el('h2', { class: 'vf-gh' }, T.groups[name]), note, el('dl', { class: 'vf-dl' }, ...groups[name]),
  ];
  return el('div', { class: 'vf-det', id: 'verifier-details' }, ...[
    section('card'),
    compact && [el('p', { class: 'vf-cnote', id: 'verifier-compact-note' }, T.compactNote)],
    section('free', el('p', { class: 'vf-gnote' }, T.freeNote)),
    section('cover'), section('fix'), section('co2'), section('sig'),
  ].filter(Boolean).flat());
}

function orgLine(org) {
  const vars = { org: org.name, domain: org.domain };
  if (filled(org.name) && filled(org.domain)) return fill(T.level1Org, vars);
  if (filled(org.name)) return fill(T.level1OrgOnly, vars);
  if (filled(org.domain)) return fill(T.level1Domain, vars);
  return null;
}

function level1(card, compact) {
  const org = isObject(card.org) ? card.org : {};
  const line = orgLine(org);
  const [first, second] = T.level1.split(/(?<=\.) /);
  const dns = typeof org.domain === 'string' && isValidDomain(org.domain)
    && el('div', { class: 'vf-dns' }, el('button', { class: 'vf-btn', id: 'verifier-dns', type: 'button' }, el('span', null, fill(T.dnsButton, { domain: org.domain }))));
  show('level1',
    head(headline(el('span', null, first), ' ', el('span', null, second)), null, line && el('p', { class: 'vf-org', id: 'verifier-org' }, line)),
    dns,
    details(card, compact),
    el('div', { class: 'vf-ft' }, el('a', { href: T.methodHref, 'data-plain': true }, T.method), limitsButton()));
}

// ---- inputs ----

let latest = 0;

async function check(pair, kind, n) {
  let r;
  try {
    r = await openCard(pair, kind);
  } catch {
    r = { state: 'cannot-verify' };
  }
  if (n !== latest) return;
  if (r.state === 'ok') level1(r.card, kind === 'compact');
  else refuse(r.state);
}

function fromHash() {
  langSwitch?.setAttribute('href', langBase + location.hash);
  const n = ++latest;
  if (!location.hash) return show('empty');
  const pair = parseFragment(location.hash);
  return pair ? check(pair, 'compact', n) : refuse('invalid-link');
}

const ascii = (s) => Array.from(s, (c) => c.charCodeAt(0));
const at = (b, sig, offset = 0) => sig.every((x, i) => b[offset + i] === x);
const READERS = { png: readPngCard, pdf: readPdfCard, svg: readSvgCard, jxl: readJxlCard };

/** The format from the first bytes; `reencoded` for what a social network gives back. */
function sniff(b) {
  if (at(b, [0x89, 0x50, 0x4e, 0x47])) return 'png';
  if (at(b, ascii('%PDF-'))) return 'pdf';
  if (at(b, [0, 0, 0, 0x0c, 0x4a, 0x58, 0x4c, 0x20, 0x0d, 0x0a, 0x87, 0x0a]) || at(b, [0xff, 0x0a])) return 'jxl';
  if (at(b, [0xff, 0xd8, 0xff]) || (at(b, ascii('RIFF')) && at(b, ascii('WEBP'), 8))) return 'reencoded';
  let i = at(b, [0xef, 0xbb, 0xbf]) ? 3 : 0;
  while ([0x09, 0x0a, 0x0c, 0x0d, 0x20].includes(b[i])) i += 1;
  return at(b, ascii('<?xml'), i) || at(b, ascii('<svg'), i) ? 'svg' : null;
}

async function fromFile(file) {
  if (!file) return;
  const n = ++latest;
  if (file.size > MAX_FILE) return refuse('file-too-large');
  const bytes = new Uint8Array(await file.arrayBuffer());
  if (n !== latest) return;
  const format = sniff(bytes);
  if (!format) return refuse('file-type');
  let pair = null;
  if (format !== 'reencoded') {
    try {
      pair = parseEnvelope(await READERS[format](bytes));
    } catch {
      pair = null;
    }
  }
  if (n !== latest) return;
  return pair ? check(pair, 'full', n) : refuse('no-card-data');
}

input.addEventListener('change', () => {
  fromFile(input.files[0]);
  input.value = '';
});
dropZone.addEventListener('dragenter', (e) => {
  e.preventDefault();
  dropZone.setAttribute('data-over', '');
});
dropZone.addEventListener('dragover', (e) => e.preventDefault());
dropZone.addEventListener('dragleave', (e) => {
  if (!dropZone.contains(e.relatedTarget)) dropZone.removeAttribute('data-over');
});
dropZone.addEventListener('drop', (e) => {
  e.preventDefault();
  dropZone.removeAttribute('data-over');
  fromFile(e.dataTransfer?.files?.[0]);
});
// A file dropped beside the zone must not make the browser open it.
for (const type of ['dragover', 'drop']) window.addEventListener(type, (e) => e.preventDefault());

document.addEventListener('click', (e) => {
  if (!e.target.closest?.('[data-open-limits]')) return;
  limits.open = true;
  limits.scrollIntoView({ behavior: 'smooth', block: 'start' });
});

window.addEventListener('hashchange', fromHash);
fromHash();
