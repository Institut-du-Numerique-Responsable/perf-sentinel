import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import {
  loadSiteData, level, localize, labErrors, teamsSummary, escapeHtml,
} from './technologies-model.mjs';

// Builds a folder under os.tmpdir() from { 'relative/path': 'content' }, a path
// ending in '/' being an empty directory. Removed when the test ends.
function tmpTree(t, files) {
  const root = mkdtempSync(join(tmpdir(), 'technologies-model-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const [path, content] of Object.entries(files)) {
    if (path.endsWith('/')) mkdirSync(join(root, path), { recursive: true });
    else {
      mkdirSync(dirname(join(root, path)), { recursive: true });
      writeFileSync(join(root, path), content);
    }
  }
  return root;
}

const MAKEFILE = [
  'verify-all-scenarios: seed',
  '\t@# ghost is only named in this comment',
  '\t@for s in alpha beta-two; do \\',
  '\t  echo "==> verify-$$s"; \\',
  '\tdone',
  '',
  'verify-other: ## not part of the aggregate',
  '\t@for s in gamma; do echo $$s; done',
  '',
].join('\n');

const tech = (...lab) => ({ teams: [], families: [{ id: 'fam', rows: [{ id: 'row', lab }] }] });

test('level follows unsupported, then fallback, then criteria', () => {
  assert.equal(level({ unsupported: true, fallback: true, criteria: [true, true] }), 'none');
  assert.equal(level({ fallback: true, criteria: [true, true] }), 'generic');
  assert.equal(level({ criteria: [true, true, true] }), 'full');
  assert.equal(level({ criteria: [true, false, true] }), 'supported');
});

test('localize falls back to English when French omits a field', () => {
  const technologies = {
    teams: [{ id: 't1', since: '2026-06' }],
    families: [{
      id: 'fam', title: 'Frameworks', row_header: 'Technology', extra_column: null,
      criteria: ['Recognized', 'Proven'], after: null,
      add: [{ label: 'Add one', contributing_row: 'Get one', difficulty: 'low' }],
      groups: [{ id: 'g', title: 'Java', logo: 'java' }],
      rows: [
        { id: 'r1', name: 'Spring', detail: 'JPA', note: 'English note', criteria: [true], lab: [] },
        { id: 'r2', name: 'Quarkus', criteria: [true], lab: [] },
      ],
    }],
  };
  const french = {
    families: { fam: { title: 'Frameworks et ORM', criteria: ['Reconnue', 'Éprouvée'], add: ['Ajouter'], groups: { g: 'Java FR' } } },
    rows: { r1: { name: 'Spring FR', note: 'Note française' }, r2: {} },
  };
  const before = structuredClone(technologies);
  const fr = localize(technologies, french, 'fr');
  const [fam] = fr.families;
  assert.equal(fam.title, 'Frameworks et ORM');
  assert.equal(fam.row_header, 'Technology');
  assert.equal(fam.extra_column, null);
  assert.deepEqual(fam.criteria, ['Reconnue', 'Éprouvée']);
  assert.deepEqual(fam.add, [{ label: 'Ajouter', contributing_row: 'Get one', difficulty: 'low' }]);
  assert.deepEqual(fam.groups, [{ id: 'g', title: 'Java FR', logo: 'java' }]);
  assert.deepEqual(fam.rows[0], { id: 'r1', name: 'Spring FR', detail: 'JPA', note: 'Note française', criteria: [true], lab: [] });
  assert.deepEqual(fam.rows[1], technologies.families[0].rows[1]);
  assert.deepEqual(fr.teams, technologies.teams);
  assert.deepEqual(technologies, before, 'the English data is not mutated');
  assert.equal(localize(technologies, french, 'en'), technologies);
});

test('labErrors is empty when every proof exists', (t) => {
  const lab = tmpTree(t, {
    Makefile: MAKEFILE,
    'scenarios/alpha/verify.sh': '',
    'manifests/m.yaml': '',
    'services/svc-a/Dockerfile': '',
    'services/shared-config/x': '',
    'services/svc.tests/x': '',
    'services/.mvn/x': '',
    'services/mvnw': '',
  });
  assert.deepEqual(labErrors(tech('services/svc-a', 'scenarios/alpha', 'manifests/m.yaml'), lab), []);
});

test('labErrors names a missing proof path', (t) => {
  const lab = tmpTree(t, { Makefile: MAKEFILE, 'services/svc-a/Dockerfile': '' });
  const errors = labErrors(tech('services/svc-a', 'scenarios/alpha', 'manifests/m.yaml'), lab);
  assert.equal(errors.length, 2);
  assert.match(errors[0], /scenarios\/alpha/);
  assert.match(errors[1], /manifests\/m\.yaml/);
});

test('labErrors names a scenario absent from verify-all-scenarios', (t) => {
  const lab = tmpTree(t, {
    Makefile: MAKEFILE,
    'scenarios/alpha/': '', 'scenarios/beta/': '', 'scenarios/ghost/': '', 'scenarios/gamma/': '',
  });
  const errors = labErrors(tech('scenarios/alpha', 'scenarios/beta', 'scenarios/ghost', 'scenarios/gamma'), lab);
  assert.equal(errors.length, 3, 'only alpha is listed in the target');
  assert.match(errors[0], /scenarios\/beta.*verify-all-scenarios/);
  assert.match(errors[1], /scenarios\/ghost.*verify-all-scenarios/);
  assert.match(errors[2], /scenarios\/gamma.*verify-all-scenarios/);
});

test('labErrors names a stack service no row cites', (t) => {
  const lab = tmpTree(t, {
    Makefile: MAKEFILE,
    'services/svc-a/': '', 'services/svc-b/': '',
    'services/shared-config/': '', 'services/svc.tests/': '', 'services/.mvn/': '', 'services/mvnw': '',
  });
  const errors = labErrors(tech('services/svc-a'), lab);
  assert.equal(errors.length, 1);
  assert.match(errors[0], /services\/svc-b/);
});

test('labErrors reports a missing lab checkout once', (t) => {
  const lab = tmpTree(t, { 'services/svc-a/': '' });
  const rows = tech('services/svc-a', 'scenarios/alpha');
  const absent = join(lab, 'nowhere');
  assert.deepEqual(labErrors(rows, absent), [`lab checkout not found at ${absent}`]);
  assert.deepEqual(labErrors(rows, lab), [`lab checkout not found at ${lab}`], 'no Makefile');
});

test('teamsSummary returns the earliest month', () => {
  const teams = [{ id: 'b', since: '2026-09' }, { id: 'a', since: '2026-06' }, { id: 'c', since: '2026-11' }];
  assert.deepEqual(teamsSummary({ teams }), { count: 3, since: '2026-06' });
  assert.deepEqual(teamsSummary({ teams: [] }), { count: 0, since: null });
});

test('escapeHtml neutralises markup', () => {
  assert.equal(escapeHtml('<b>&"'), '&lt;b&gt;&amp;&quot;');
  assert.equal(escapeHtml("it's"), 'it&#39;s');
});

test('loadSiteData names the missing file', (t) => {
  const json = (o) => JSON.stringify(o);
  const engine = tmpTree(t, {
    'site-data/technologies.json': json({ teams: [], families: [] }),
    'site-data/references.json': json({ references: [] }),
  });
  assert.throws(() => loadSiteData(engine), /technologies-fr\.json/);
  writeFileSync(join(engine, 'site-data/technologies-fr.json'), json({ families: {}, rows: {} }));
  assert.deepEqual(loadSiteData(engine), {
    technologies: { teams: [], families: [] },
    french: { families: {}, rows: {} },
    references: { references: [] },
  });
});

const { ENGINE_DIR, LAB_DIR } = process.env;
test('the real data passes against the real lab', { skip: !(ENGINE_DIR && LAB_DIR) && 'ENGINE_DIR or LAB_DIR is not set' }, () => {
  const { technologies } = loadSiteData(ENGINE_DIR);
  assert.deepEqual(labErrors(technologies, LAB_DIR), []);
});
