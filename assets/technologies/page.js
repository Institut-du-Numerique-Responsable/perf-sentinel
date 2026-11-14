// Behaviour of the technologies page. Every row and its detail ship in the HTML.
// An inline script in <head> sets html.js, under which page.css shows a detail only
// when its row carries the open class. This script owns that state and the
// button's aria-expanded, then adds the search, the production filter
// (#production) and the scroll rail. ES2018 syntax at most: a browser that runs modules
// but cannot parse this file still gets every detail open, see HEAD in build-technologies.mjs.
import { COPY } from './copy.js';

const t = COPY[document.documentElement.lang === 'fr' ? 'fr' : 'en'];

const q = document.getElementById('tp-q');
const prod = document.getElementById('tp-prod');
const none = document.getElementById('tp-none');
const noneProd = document.getElementById('tp-none-prod');
const status = document.getElementById('tp-status');
const families = [...document.querySelectorAll('section.fam')];
const rows = [...document.querySelectorAll('tr.row')];
const expanded = (row) => row.querySelector('button').getAttribute('aria-expanded') === 'true';

function setOpen(row, open) {
  row.querySelector('button').setAttribute('aria-expanded', String(open));
  row.classList.toggle('open', open);
}
for (const row of rows) setOpen(row, false);

// The whole row toggles; Enter and Space reach the same path through the button's click.
document.querySelector('.fams').addEventListener('click', (e) => {
  const row = e.target.closest('tr.row');
  if (row && !e.target.closest('a')) setOpen(row, !expanded(row));
});

// say: announce the result in the live region, after a search or a filter change, not on load.
function filter(say) {
  const s = q.value.trim().toLowerCase();
  const onlyProd = prod.getAttribute('aria-pressed') === 'true';
  let any = 0;
  let found = 0;
  for (const fam of families) {
    let n = 0;
    for (const tbody of fam.querySelectorAll('tbody')) {
      let m = 0;
      for (const row of tbody.querySelectorAll('tr.row')) {
        const hit = row.dataset.h.includes(s);
        found += hit;
        row.hidden = !(hit && (!onlyProd || row.hasAttribute('data-prod')));
        m += !row.hidden;
      }
      const grp = tbody.querySelector('tr.grp');
      if (grp) grp.hidden = !m;
      n += m;
    }
    fam.hidden = !n;
    any += n;
  }
  // "Not recognized" comes from the search alone: the filter only narrows what exists.
  none.hidden = found > 0;
  if (!found) none.querySelector('h2').textContent = t.notFound.replace('{q}', () => q.value.trim());
  noneProd.hidden = !(found && !any);
  if (say) status.textContent = !found ? none.querySelector('h2').textContent : any ? t.shown(any) : t.noneInProduction;
  onScroll();
}

function setProduction(on) {
  prod.setAttribute('aria-pressed', String(on));
  history.replaceState(null, '', on ? '#production' : location.pathname + location.search);
  filter(true);
}

q.addEventListener('input', () => filter(true));
prod.addEventListener('click', () => setProduction(prod.getAttribute('aria-pressed') !== 'true'));
addEventListener('hashchange', () => { if (location.hash === '#production') setProduction(true); });
document.addEventListener('keydown', (e) => {
  if (e.key === '/' && !e.target.closest('input, textarea, [contenteditable]')) {
    e.preventDefault();
    q.focus();
  }
});

// syncDots of the home page: the last family whose top crossed 40% of the window,
// the last visible family at the bottom of the page, hidden families skipped.
const rail = [...document.querySelectorAll('.tp-rail a')];
function syncRail() {
  const shown = families.filter((f) => !f.hidden);
  const line = innerHeight * 0.4;
  let cur = shown[0];
  for (const f of shown) if (f.getBoundingClientRect().top <= line) cur = f;
  if (innerHeight + scrollY >= document.documentElement.scrollHeight - 2) cur = shown[shown.length - 1] || cur;
  for (const a of rail) {
    if (cur && a.getAttribute('href') === '#' + cur.id) a.setAttribute('aria-current', 'true');
    else a.removeAttribute('aria-current');
  }
}
let raf = 0;
function onScroll() {
  cancelAnimationFrame(raf);
  raf = requestAnimationFrame(syncRail);
}
addEventListener('scroll', onScroll, { passive: true });
addEventListener('resize', onScroll);

if (location.hash === '#production') prod.setAttribute('aria-pressed', 'true');
filter();
// Read by the inline script of <head> on load: without it, html.js goes and every detail opens.
document.documentElement.dataset.tpReady = '1';
