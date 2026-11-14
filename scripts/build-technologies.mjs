// Generates the technologies pages, technologies.html and fr/technologies.html,
// from the engine's site-data, after checking its lab proofs against the lab:
//   node scripts/build-technologies.mjs <engineDir> <labDir> [outDir]
// outDir defaults to the site root. Exits 1 and writes nothing when a proof is
// missing. Every row is in the HTML: the page reads without JavaScript, with
// every detail open. With JavaScript, page.css folds the details under html.js
// and assets/technologies/page.js adds the toggles, the search and the filters.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { COPY } from '../assets/technologies/copy.js';
import { escapeHtml as esc, labErrors, level, loadSiteData, localize } from './technologies-model.mjs';
import { page } from './site-chrome.mjs';

const SITE = join(dirname(fileURLToPath(import.meta.url)), '..');
const CONTRIBUTING = 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/blob/main/CONTRIBUTING.md#what-to-contribute-by-difficulty';
const LAB_ROOT = 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel-simulation-lab';
const LAB = `${LAB_ROOT}/tree/main/`;
const fill = (text, values) => text.replace(/\{(\w+)\}/g, (_, k) => values[k]);

// Drawings of the mock-up: level signs at the ink of the text, the ringed ones for production.
const SIGN = {
  full: '<circle cx="8" cy="8" r="6.2" fill="currentColor"/>',
  supported: '<circle cx="8" cy="8" r="6.2" fill="none"/><path d="M8 1.8a6.2 6.2 0 0 0 0 12.4z" fill="currentColor"/>',
  generic: '<circle cx="8" cy="8" r="6.2" fill="none"/>',
  none: '<circle cx="8" cy="8" r="2" fill="currentColor"/>',
};
const RING = {
  full: '<circle cx="8" cy="8" r="3.6" fill="currentColor"/>',
  supported: '<circle cx="8" cy="8" r="3.7" fill="none"/><path d="M8 4.3a3.7 3.7 0 0 0 0 7.4z" fill="currentColor"/>',
  generic: '<circle cx="8" cy="8" r="3.7" fill="none"/>',
  none: '<circle cx="8" cy="8" r="1.3" fill="currentColor"/>',
};
const sign = (k, ringed) => `<svg class="lv${ringed ? ' pr' : ''}" viewBox="0 0 16 16" aria-hidden="true">${ringed ? RING[k] + '<circle cx="8" cy="8" r="7.1" fill="none"/>' : SIGN[k]}</svg>`;
const mark = (v, t) => v
  ? `<svg class="mk y" viewBox="0 0 16 16" aria-hidden="true"><path d="M3.2 8.4l3.1 3.1 6.5-7"/></svg><span class="sr">${t.yes}</span>`
  : `<svg class="mk n" viewBox="0 0 16 16" aria-hidden="true"><path d="M4.5 8h7"/></svg><span class="sr">${t.no}</span>`;
const PLUS = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3.5v9M3.5 8h9"/></svg>';
const CHEVRON = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 3.5l4.5 4.5L6 12.5"/></svg>';
const SEARCH = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.6-3.6"/></svg>';
const DIFFICULTY = { low: 'df-f', medium: 'df-m', high: 'df-e' };

// A language logo, with its dark twin when assets/logos has one. Decorative: the title follows.
function logo(name) {
  if (!name) return '';
  const src = (n) => `/assets/logos/${esc(n)}.svg`;
  if (!existsSync(join(SITE, 'assets', 'logos', `${name}-dark.svg`))) return `<img src="${src(name)}" alt="">`;
  return `<img data-logo="light" src="${src(name)}" alt=""><img data-logo="dark" src="${src(name + '-dark')}" alt="">`;
}

const month = (yyyyMm, t) => {
  const [y, m] = yyyyMm.split('-');
  return `${t.months[Number(m) - 1]} ${y}`;
};

function since(row, teams) {
  return row.production.map((id) => {
    const team = teams.find((x) => x.id === id);
    if (!team) throw new Error(`row ${row.id}: production team ${id} is not in teams`);
    return team.since;
  }).sort()[0];
}

function rowHtml(family, row, group, teams, t) {
  const k = level(row);
  const prod = row.production?.length > 0;
  const cols = family.criteria.length + 3 + (family.extra_column ? 1 : 0);
  const id = `d-${family.id}-${row.id}`;
  const haystack = [row.name, row.detail, ...(row.aliases ?? []), group?.title].filter(Boolean).join(' ').toLowerCase();
  const crit = `<div class="crit">${family.criteria.map((c, i) => `<span>${mark(row.criteria[i], t)}${esc(c)}</span>`).join('')}</div>`;
  const note = k === 'full'
    ? fill(t.allMet, { n: family.criteria.length }) + (row.note ? ' ' + esc(row.note) : '')
    : esc(row.note ?? '');
  // A complete row has nothing left to contribute. An unproven row of a family that
  // has a lab criterion points at the lab itself, where its proof would go.
  const labHref = row.lab.length ? LAB + esc(row.lab[0]) : k !== 'full' && family.lab_criterion != null ? LAB_ROOT : null;
  const links = (k !== 'full' ? `<a href="${CONTRIBUTING}">${t.contribute}</a>` : '') +
    (labHref ? `<a href="${labHref}">${t.lab}</a>` : '') +
    `<a href="${t.referencesHref}">${t.report}</a>`;
  const used = prod
    ? `<p class="pp">${sign(k, true)}${fill(t.usedSince, { since: month(since(row, teams), t), teams: t.teams(row.production.length) })}</p>`
    : '';
  return `<tr class="row" data-h="${esc(haystack)}"${prod ? ' data-prod' : ''}>` +
    `<th scope="row" class="name"><button type="button" aria-controls="${id}"><span class="n">${esc(row.name)}</span>${row.detail && !family.extra_column ? `<small>${esc(row.detail)}</small>` : ''}</button></th>` +
    (family.extra_column ? `<td class="x">${esc(row.detail ?? '')}</td>` : '') +
    `<td class="lvc">${sign(k, prod)}${t.levels[k][0]}${prod ? `<span class="sr">${t.inProduction}</span>` : ''}</td>` +
    row.criteria.map((v) => `<td class="c">${mark(v, t)}</td>`).join('') +
    `<td class="chev">${CHEVRON}</td></tr>` +
    `<tr class="det" id="${id}"><td colspan="${cols}">${crit}<p class="note">${note}</p>${used}<p class="lk">${links}</p></td></tr>`;
}

function familyHtml(family, teams, t) {
  const cols = family.criteria.length + 3 + (family.extra_column ? 1 : 0);
  const head = `<thead><tr><th scope="col">${esc(family.row_header)}</th>` +
    (family.extra_column ? `<th scope="col" class="x">${esc(family.extra_column)}</th>` : '') +
    `<th scope="col" class="lvh">${t.level}</th>` +
    family.criteria.map((c) => `<th scope="col" class="c">${esc(c)}</th>`).join('') +
    `<td></td></tr></thead>`;
  const body = family.groups
    ? family.groups.map((g) => `<tbody><tr class="grp"><th colspan="${cols}" scope="rowgroup"><span>${logo(g.logo)}${esc(g.title)}</span></th></tr>` +
      family.rows.filter((r) => r.group === g.id).map((r) => rowHtml(family, r, g, teams, t)).join('') + '</tbody>').join('')
    : '<tbody>' + family.rows.map((r) => rowHtml(family, r, null, teams, t)).join('') + '</tbody>';
  const add = family.add.map((a) => `<a href="${CONTRIBUTING}">${PLUS}${esc(a.label)}</a><span>${t.difficulty} <b class="${DIFFICULTY[a.difficulty]}">${t.difficulties[a.difficulty]}</b></span>`).join('');
  return `<section class="fam" id="${esc(family.id)}"><h2 id="h-${esc(family.id)}"><span class="sw" aria-hidden="true"></span>${esc(family.title)}</h2>` +
    `<table class="${family.groups ? '' : 'flat'}${family.extra_column ? ' tx' : ''}" aria-labelledby="h-${esc(family.id)}">${head}${body}</table>` +
    (family.after ? `<p class="after">${esc(family.after)}</p>` : '') +
    (add ? `<p class="add">${add}</p>` : '') + '</section>';
}

/** The HTML of <main>: data is loadSiteData's result, provenance { engine, lab, built: Date }. */
export function technologiesMain(data, lang, provenance) {
  const t = COPY[lang];
  const { teams, families } = localize(data.technologies, data.french, lang);
  const d = provenance.built;
  const legend = Object.keys(SIGN).map((k) => `<div><b>${sign(k)}${t.levels[k][0]}</b><span>${t.levels[k][1]}</span></div>`).join('') +
    `<p>${sign('full', true)}<span>${t.ringed}</span></p>`;
  return `<section class="intro"><h1>${t.h1}</h1><p>${t.intro}</p>` +
    `<div class="tools"><div class="search">${SEARCH}<input id="tp-q" type="search" placeholder="${t.placeholder}" aria-label="${t.searchLabel}" autocomplete="off"><kbd aria-hidden="true">/</kbd></div>` +
    `<button id="tp-prod" type="button" aria-pressed="false">${sign('full', true)}${t.production}</button></div></section>` +
    `<section class="legend" aria-label="${t.legendLabel}">${legend}</section>` +
    `<nav class="tp-rail" aria-label="${t.railLabel}">${families.map((f) => `<a href="#${esc(f.id)}"><span class="sw"></span><b>${esc(f.title)}</b></a>`).join('')}</nav>` +
    `<div class="fams">${families.map((f) => familyHtml(f, teams, t)).join('')}` +
    `<section class="none" id="tp-none" hidden><h2></h2><p>${t.notFoundText}</p><a href="${CONTRIBUTING}">${t.contribute}</a></section></div>` +
    `<p class="foot">${fill(t.provenance, { engine: esc(provenance.engine), lab: esc(provenance.lab), date: `${d.getDate()} ${t.months[d.getMonth()]} ${d.getFullYear()}` })}</p>` +
    `<script type="module" src="/assets/technologies/page.js"></script>`;
}

// The js class lands before the body paints, so page.css folds the details without a flash.
const HEAD = '<link rel="stylesheet" href="/assets/technologies/page.css"><script>document.documentElement.classList.add(\'js\')</script>';
// /technologies follows the stored choice, else the browser language; /fr/technologies never redirects.
const REDIRECT = `<script>(function(){var l=null;try{l=localStorage.getItem('ps-lang')}catch(e){}if(l!=='fr'&&l!=='en')l=(navigator.language||'').toLowerCase().indexOf('fr')===0?'fr':'en';if(l==='fr')location.replace('/fr/technologies'+location.hash);})();</script>`;

function main([engineDir, labDir, outDir = SITE]) {
  if (!engineDir || !labDir) {
    console.error('usage: node scripts/build-technologies.mjs <engineDir> <labDir> [outDir]');
    process.exit(2);
  }
  const data = loadSiteData(engineDir);
  const errors = labErrors(data.technologies, labDir);
  if (errors.length) {
    console.error(`build-technologies: no page written\n${errors.join('\n')}`);
    process.exit(1);
  }
  const engine = readFileSync(join(engineDir, 'Cargo.toml'), 'utf8').match(/\[workspace\.package\][^[]*?^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!engine) throw new Error(`no [workspace.package].version in ${join(engineDir, 'Cargo.toml')}`);
  const lab = execFileSync('git', ['-C', labDir, 'rev-parse', '--short', 'HEAD'], { encoding: 'utf8' }).trim();
  const provenance = { engine, lab, built: new Date() };
  mkdirSync(join(outDir, 'fr'), { recursive: true });
  writeFileSync(join(outDir, 'technologies.html'), page('en', '/fr/technologies', COPY.en.title, COPY.en.desc, technologiesMain(data, 'en', provenance), { head: HEAD + REDIRECT, mainClass: 'tp' }));
  writeFileSync(join(outDir, 'fr', 'technologies.html'), page('fr', '/technologies', COPY.fr.title, COPY.fr.desc, technologiesMain(data, 'fr', provenance), { head: HEAD, mainClass: 'tp' }));
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) main(process.argv.slice(2));
