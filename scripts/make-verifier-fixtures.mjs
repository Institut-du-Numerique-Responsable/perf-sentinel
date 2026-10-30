// Makes the share card fixtures the verifier tests read, with the engine itself:
// one throwaway key, story in the four formats, slow/curve/collective/nodomain as
// PNG, and swapped.<ext> (a twin card carrying the story's envelope) that
// verify-card must read as modified. Writes scripts/fixtures/verifier/fixtures.json.
// The private key stays in a temporary directory, deleted on exit.
// Run: PERF_SENTINEL_BIN=/path/to/perf-sentinel node scripts/make-verifier-fixtures.mjs
import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { crc32 } from 'node:zlib';

const BIN = process.env.PERF_SENTINEL_BIN || '/Users/robintrassard/RustroverProjects/perf-sentinel/target/release/perf-sentinel';
const DIR = join(dirname(fileURLToPath(import.meta.url)), 'fixtures', 'verifier');
const FORMATS = ['png', 'pdf', 'svg', 'jxl'];
const PNG_ONLY = ['slow', 'curve', 'collective', 'nodomain'];

function fail(msg) {
  throw new Error(msg);
}

function run(args) {
  const r = spawnSync(BIN, args, { encoding: 'utf8' });
  if (r.error) fail(`${BIN}: ${r.error.message}`);
  return { status: r.status, out: r.stdout.trim(), line: r.stdout.trim() ? JSON.parse(r.stdout.trim()) : null };
}

function render(input, key, out, format) {
  const r = run(['card', 'render', '--input', join(DIR, `input-${input}.json`), '--signing-key', key, '--out-dir', out, '--format', format]);
  if (r.status !== 0 || r.line?.result !== 'rendered' || r.line.files.length !== 1) fail(`render ${input} ${format}: ${r.out}`);
  return { file: join(out, r.line.files[0]), compact_url: r.line.compact_url };
}

function verifyOriginal(path) {
  const r = run(['verify-card', path]);
  if (r.status !== 0 || r.line?.integrity !== 'ok' || r.line.content !== 'intact') fail(`verify-card ${path}: ${r.out}`);
  return r.line;
}

function verifySwapped(path) {
  const r = run(['verify-card', path]);
  if (r.status !== 1 || r.line?.content !== 'modified' || !r.out.includes('content_modified')) fail(`verify-card ${path}: ${r.out}`);
}

// The envelope text's one occurrence; a compressed iTXt or a second copy has none or two.
function envelopeAt(bytes, what) {
  const start = bytes.indexOf('{"content_sig":"');
  if (start < 0 || bytes.indexOf('{"content_sig":"', start + 1) >= 0) fail(`${what}: envelope absent or present twice`);
  const end = bytes.indexOf('"}', start) + 2;
  JSON.parse(bytes.subarray(start, end).toString('utf8'));
  return { start, end };
}

// Rewrites the CRC of the PNG chunk whose data holds `offset`.
function fixPngCrc(png, offset) {
  for (let at = 8; at < png.length; ) {
    const len = png.readUInt32BE(at);
    const dataEnd = at + 8 + len;
    if (offset >= at + 8 && offset < dataEnd) {
      png.writeUInt32BE(crc32(png.subarray(at + 4, dataEnd)), dataEnd);
      return;
    }
    at = dataEnd + 4;
  }
  fail('png: envelope chunk not found');
}

function swapped(story, twin, format) {
  const s = readFileSync(story);
  const t = readFileSync(twin);
  const se = envelopeAt(s, `story.${format}`);
  const te = envelopeAt(t, `twin.${format}`);
  if (se.end - se.start !== te.end - te.start) fail(`${format}: story and twin envelopes differ in length`);
  s.copy(t, te.start, se.start, se.end);
  if (format === 'png') fixPngCrc(t, te.start);
  const path = join(DIR, `swapped.${format}`);
  writeFileSync(path, t);
  return path;
}

const tmp = mkdtempSync(join(tmpdir(), 'ps-card-'));
try {
  const keygen = run(['card', 'keygen', '--out-dir', tmp]);
  if (keygen.status !== 0) fail(`keygen: ${keygen.out}`);
  const keys = readdirSync(tmp).filter((f) => f.endsWith('.pk8'));
  if (keys.length !== 1) fail(`keygen wrote ${keys.length} .pk8 files`);
  const key = join(tmp, keys[0]);
  const out = join(tmp, 'out');
  mkdirSync(out);

  const cards = { story: {} };
  const copy = (angle, format) => {
    const r = render(angle, key, out, format);
    const path = join(DIR, `${angle}.${format}`);
    copyFileSync(r.file, path);
    const line = verifyOriginal(path);
    return { name: `${angle}.${format}`, compact_url: r.compact_url, figures: line.figures };
  };
  const story = FORMATS.map((format) => copy('story', format));
  FORMATS.forEach((format, i) => (cards.story[format] = story[i].name));
  cards.story.compact_url = story[0].compact_url;
  cards.story.figures = story[0].figures;
  for (const angle of PNG_ONLY) {
    const c = copy(angle, 'png');
    cards[angle] = { png: c.name, compact_url: c.compact_url, figures: c.figures };
  }

  const swappedFiles = {};
  for (const format of ['png', 'svg', 'jxl', 'pdf']) {
    const twin = join(tmp, `twin.${format}`);
    copyFileSync(render('twin', key, out, format).file, twin);
    verifySwapped(swapped(join(DIR, `story.${format}`), twin, format));
    swappedFiles[format] = `swapped.${format}`;
  }

  const fixtures = {
    fingerprint: keygen.line.fingerprint,
    public_key: keygen.line.public_key,
    date: '2026-10-15',
    domain: 'cards.example.org',
    cards,
    swapped: swappedFiles,
  };
  writeFileSync(join(DIR, 'fixtures.json'), JSON.stringify(fixtures, null, 2) + '\n');
  console.log(`wrote ${Object.keys(cards).length} cards, 4 swapped files and fixtures.json`);
} catch (e) {
  console.error(e.message);
  process.exitCode = 1;
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
