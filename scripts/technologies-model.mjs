// Model of the technologies pages: loads the hand-written data of the engine
// repo (site-data/), derives a level per row, localizes it, and checks its lab
// proofs against a checkout of the simulation lab. Pure: no page markup here.
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const readJson = (path) => {
  try {
    return JSON.parse(readFileSync(path, 'utf8'));
  } catch (e) {
    throw new Error(`cannot read ${path}: ${e.message}`);
  }
};

export function loadSiteData(engineDir) {
  const file = (name) => readJson(join(engineDir, 'site-data', name));
  return {
    technologies: file('technologies.json'),
    french: file('technologies-fr.json'),
    references: file('references.json'),
  };
}

// Never stored in the data: derived from the flags and the criteria of a row.
export function level(row) {
  if (row.unsupported) return 'none';
  if (row.fallback) return 'generic';
  return row.criteria.every(Boolean) ? 'full' : 'supported';
}

const FAMILY_TEXTS = ['title', 'row_header', 'extra_column', 'after', 'criteria'];
const ROW_TEXTS = ['name', 'detail', 'note'];
const pick = (from, keys) => Object.fromEntries(keys.filter((k) => from[k] != null).map((k) => [k, from[k]]));

// The French file holds only what differs: `add` is the list of labels, `groups`
// maps a group id to its title. Whatever it omits stays English.
export function localize(technologies, french, lang) {
  if (lang === 'en') return technologies;
  const families = technologies.families.map((family) => {
    const fr = french.families?.[family.id] ?? {};
    return {
      ...family,
      ...pick(fr, FAMILY_TEXTS),
      ...(family.add && { add: family.add.map((a, i) => ({ ...a, label: fr.add?.[i] ?? a.label })) }),
      ...(family.groups && { groups: family.groups.map((g) => ({ ...g, title: fr.groups?.[g.id] ?? g.title })) }),
      rows: family.rows.map((row) => ({ ...row, ...pick(french.rows?.[row.id] ?? {}, ROW_TEXTS) })),
    };
  });
  return { ...technologies, families };
}

// The recipe of verify-all-scenarios: from its target line to the next target.
// Comment lines drop out, they name scenarios that do not run.
function verifyAllRecipe(makefile) {
  const lines = makefile.split('\n');
  const start = lines.findIndex((l) => /^verify-all-scenarios\s*:/.test(l));
  if (start < 0) return '';
  const end = lines.findIndex((l, i) => i > start && /^[^\s#][^:=]*:(?!=)/.test(l));
  return lines.slice(start, end < 0 ? undefined : end).filter((l) => !/^\s*@?#/.test(l)).join('\n');
}

// Whole word, a hyphen being part of it: `hub` is not in `hub-incidents`.
const listed = (recipe, name) =>
  new RegExp(`(?<![\\w-])${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?![\\w-])`).test(recipe);

const isStack = (entry) =>
  entry.isDirectory() && !/^(\.|shared-)|\.tests$/.test(entry.name);

export function labErrors(technologies, labDir) {
  const makefile = join(labDir, 'Makefile');
  if (!existsSync(makefile)) return [`lab checkout not found at ${labDir}`];
  const recipe = verifyAllRecipe(readFileSync(makefile, 'utf8'));
  const errors = [];
  const cited = new Set();
  for (const family of technologies.families) {
    for (const row of family.rows) {
      for (const path of row.lab) {
        const where = `${family.id}/${row.id}`;
        const [top, name] = path.split('/');
        if (top === 'services') cited.add(name);
        if (!existsSync(join(labDir, path))) {
          errors.push(`${where}: lab path ${path} does not exist in ${labDir}`);
        } else if (top === 'scenarios' && !listed(recipe, name)) {
          errors.push(`${where}: scenario ${path} is not in verify-all-scenarios of ${makefile}`);
        }
      }
    }
  }
  const services = join(labDir, 'services');
  const stacks = existsSync(services) ? readdirSync(services, { withFileTypes: true }).filter(isStack) : [];
  for (const { name } of stacks) {
    if (!cited.has(name)) errors.push(`services/${name} carries a stack that no row cites in lab`);
  }
  return errors;
}

export function teamsSummary({ teams }) {
  return { count: teams.length, since: teams.map((t) => t.since).sort()[0] ?? null };
}

const ESCAPES = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
export const escapeHtml = (text) => String(text).replace(/[&<>"']/g, (c) => ESCAPES[c]);
