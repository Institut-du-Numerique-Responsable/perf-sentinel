// Readers of the card envelope inside original PNG, JPEG XL, SVG and PDF files.
// Byte walks only, no decoder: each mirrors the engine's reader (plan 2 Tasks 12,
// 13 and 15), so the page refuses whatever `verify-card` refuses.

const MAX_ENVELOPE = 64 * 1024;
const KEYWORD = 'perf-sentinel-card';

const viewOf = (b) => new DataView(b.buffer, b.byteOffset, b.byteLength);
// windows-1252 maps every byte to one UTF-16 unit, so string indices stay byte offsets.
const latin1 = (b) => new TextDecoder('latin1').decode(b);
const utf8 = (b) => {
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(b);
  } catch {
    return null;
  }
};
const startsWith = (b, prefix, at = 0) => prefix.every((x, i) => b[at + i] === x);

export async function inflateCapped(bytes, cap = 1 << 20) {
  try {
    const reader = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('deflate')).getReader();
    const out = new Uint8Array(cap);
    let size = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return out.slice(0, size);
      if (size + value.length > cap) {
        await reader.cancel();
        return null;
      }
      out.set(value, size);
      size += value.length;
    }
  } catch {
    return null;
  }
}

// ---- PNG ----

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const KEYWORD_BYTES = [...KEYWORD].map((c) => c.charCodeAt(0));
// After the keyword's NUL: compression flag, method, empty language tag, empty translated keyword.
const TEXT_AT = KEYWORD.length + 5;

function crc32(bytes) {
  let crc = ~0;
  for (const b of bytes) {
    crc ^= b;
    for (let k = 0; k < 8; k++) crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
  }
  return ~crc >>> 0;
}

function chunkAt(bytes, view, at) {
  if (at + 8 > bytes.length) return null;
  const length = view.getUint32(at);
  if (length > 0x7fffffff || at + 12 + length > bytes.length) return null;
  return { type: latin1(bytes.subarray(at + 4, at + 8)), start: at, end: at + 12 + length };
}

function isCardChunk(bytes, c) {
  const data = bytes.subarray(c.start + 8, c.end - 4);
  return c.type === 'iTXt' && startsWith(data, KEYWORD_BYTES) && data[KEYWORD.length] === 0;
}

/** The card's `iTXt` chunk (length field through CRC) and its envelope text, by the PNG card walk. */
export function pngEnvelopeRange(bytes) {
  if (!startsWith(bytes, PNG_SIGNATURE)) return null;
  const view = viewOf(bytes);
  const ihdr = chunkAt(bytes, view, 8);
  if (!ihdr || ihdr.type !== 'IHDR' || ihdr.end - ihdr.start !== 25) return null;
  let card = null;
  let count = 0;
  let seenIdat = false;
  for (let at = ihdr.end; ; ) {
    const c = chunkAt(bytes, view, at);
    if (!c) return null;
    if (c.type === 'IEND') break;
    if (c.type === 'IDAT') seenIdat = true;
    else if (isCardChunk(bytes, c)) {
      count += 1;
      card = seenIdat ? null : c;
    }
    at = c.end;
  }
  if (count !== 1 || !card) return null;
  const body = bytes.subarray(card.start + 4, card.end - 4);
  if (crc32(body) !== view.getUint32(card.end - 4)) return null;
  const header = body.subarray(4 + KEYWORD.length + 1, 4 + TEXT_AT);
  if (header.length !== 4 || header.some((b) => b !== 0)) return null;
  const text = [card.start + 8 + TEXT_AT, card.end - 4];
  if (text[1] - text[0] > MAX_ENVELOPE || utf8(bytes.subarray(...text)) === null) return null;
  return { chunk: [card.start, card.end], text };
}

export function readPngCard(bytes) {
  const range = pngEnvelopeRange(bytes);
  return range && utf8(bytes.subarray(...range.text));
}

// ---- JPEG XL ----

const JXL_SIGNATURE = [0, 0, 0, 0x0c, 0x4a, 0x58, 0x4c, 0x20, 0x0d, 0x0a, 0x87, 0x0a];

/** Every top-level box, header included, each size checked before any slicing. */
export function jxlBoxes(bytes) {
  if (!startsWith(bytes, JXL_SIGNATURE)) return null;
  const view = viewOf(bytes);
  const boxes = [{ type: 'JXL ', start: 0, end: 12 }];
  for (let at = 12; at < bytes.length; ) {
    const rest = bytes.length - at;
    if (rest < 8) return null;
    const size32 = view.getUint32(at);
    let header = 8;
    let size = size32;
    if (size32 === 0) size = rest;
    else if (size32 === 1) {
      if (rest < 16) return null;
      header = 16;
      size = view.getUint32(at + 8) * 2 ** 32 + view.getUint32(at + 12);
      if (size > Number.MAX_SAFE_INTEGER) return null;
    }
    if (size < header || size > rest) return null;
    boxes.push({ type: latin1(bytes.subarray(at + 4, at + 8)), start: at, end: at + size });
    at += size;
  }
  return boxes;
}

/** The only `pscd` box, where the engine writes it: third, at offset 32, 32-bit size, `jxlc` next. */
export function jxlEnvelopeRange(bytes) {
  const boxes = jxlBoxes(bytes);
  if (!boxes || boxes.filter((b) => b.type === 'pscd').length !== 1) return null;
  const pscd = boxes[2];
  if (boxes.slice(0, 4).map((b) => b.type).join() !== 'JXL ,ftyp,pscd,jxlc' || pscd.start !== 32) return null;
  const size32 = viewOf(bytes).getUint32(32);
  if (size32 === 0 || size32 === 1) return null;
  return { box: [pscd.start, pscd.end], payload: [pscd.start + 8, pscd.end] };
}

export function readJxlCard(bytes) {
  if (bytes[0] === 0xff && bytes[1] === 0x0a) return null; // a bare codestream
  const range = jxlEnvelopeRange(bytes);
  if (!range || range.payload[1] - range.payload[0] > MAX_ENVELOPE) return null;
  return utf8(bytes.subarray(...range.payload));
}

// ---- SVG ----

const META_OPEN = `<metadata id="${KEYWORD}">`;
const META_CLOSE = '</metadata>';
const TITLE_CLOSE = '</title>';

/** The `<metadata>` element, present once and right after the first `</title>` and its LF. */
export function svgEnvelopeRange(bytes) {
  if (utf8(bytes) === null) return null;
  const text = latin1(bytes);
  let i = text.startsWith('ï»¿') ? 3 : 0;
  while (i < text.length && ' \t\n\f\r'.includes(text[i])) i += 1;
  if (!text.startsWith('<?xml', i) && !text.startsWith('<svg', i)) return null;
  const at = text.indexOf(META_OPEN);
  if (at < 0 || text.includes(META_OPEN, at + 1)) return null;
  const close = text.indexOf(META_CLOSE, at);
  const title = text.indexOf(TITLE_CLOSE);
  if (close < 0 || title < 0 || title + TITLE_CLOSE.length + 1 !== at || text[at - 1] !== '\n') return null;
  return { element: [at, close + META_CLOSE.length], text: [at + META_OPEN.length, close] };
}

export function readSvgCard(bytes) {
  const range = svgEnvelopeRange(bytes);
  if (!range || range.text[1] - range.text[0] > MAX_ENVELOPE) return null;
  const envelope = latin1(bytes.subarray(...range.text));
  return /^[A-Za-z0-9{}":,_-]*$/.test(envelope) ? envelope : null;
}

// ---- PDF ----

const escapeRegExp = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** The embedded file named `name`: filespec, its `/EF /F` object, a direct `/Length`, Flate or no filter. */
export async function readPdfAttachment(bytes, name) {
  const text = latin1(bytes);
  const spec = new RegExp(`/F\\s*\\(${escapeRegExp(name)}\\)`).exec(text);
  if (!spec) return null;
  const ef = /\/EF\s*<<\s*\/F\s+(\d+)\s+0\s+R/g;
  ef.lastIndex = spec.index;
  const ref = ef.exec(text);
  if (!ref) return null;
  // Anchored on a boundary: a plain '9 0 obj' search also hits '19 0 obj'.
  const head = new RegExp(`(?:^|[\\r\\n\\s])${ref[1]}\\s+0\\s+obj\\b`).exec(text);
  if (!head) return null;
  const dictAt = head.index + head[0].length;
  const streamAt = text.indexOf('stream', dictAt);
  if (streamAt < 0) return null;
  const dict = text.slice(dictAt, streamAt);
  const length = /\/Length\s+(\d+)\s*(?=[/>])/.exec(dict); // direct only, never `n 0 R`
  const eol = text.startsWith('stream\r\n', streamAt) ? 8 : text.startsWith('stream\n', streamAt) ? 7 : 0;
  if (!length || !eol) return null;
  const start = streamAt + eol;
  const end = start + Number(length[1]);
  if (end > bytes.length) return null;
  const data = bytes.slice(start, end);
  const filter = /\/Filter\s*(\[[^\]]*\]|\/[A-Za-z0-9]+)/.exec(dict);
  if (!filter) return data;
  return filter[1].replace(/[[\]\s]/g, '') === '/FlateDecode' ? inflateCapped(data) : null;
}

export async function readPdfCard(bytes) {
  const attachment = await readPdfAttachment(bytes, `${KEYWORD}.json`);
  return attachment && utf8(attachment);
}

// ---- the bytes `content_sig` signs ----

const SLOT_MARKER = [...'{"content_sig":"'].map((c) => c.charCodeAt(0));
const SLOT_END = [...'","payload":"'].map((c) => c.charCodeAt(0));
const SIG_LENGTH = 86;

/** Offset of the value after the only `{"content_sig":"` in a PDF, when it holds `sigText`. */
function pdfSlot(bytes, sigText) {
  if (sigText.length !== SIG_LENGTH) return null;
  let at = -1;
  for (let i = 0; i + SLOT_MARKER.length <= bytes.length; i++) {
    if (bytes[i] !== SLOT_MARKER[0] || !startsWith(bytes, SLOT_MARKER, i)) continue;
    if (at >= 0) return null;
    at = i + SLOT_MARKER.length;
  }
  if (at < 0 || !startsWith(bytes, [...sigText].map((c) => c.charCodeAt(0)), at)) return null;
  return startsWith(bytes, SLOT_END, at + SIG_LENGTH) ? at : null;
}

/** The file minus its envelope carrier, or the PDF with its slot reset to `A`s: the engine's `card_content`. */
export function cardContent(format, bytes, contentSigText) {
  if (format === 'pdf') {
    const at = pdfSlot(bytes, contentSigText);
    if (at === null) return null;
    const content = bytes.slice();
    content.fill(0x41, at, at + SIG_LENGTH);
    return content;
  }
  let range;
  if (format === 'png') range = pngEnvelopeRange(bytes)?.chunk;
  else if (format === 'svg') range = svgEnvelopeRange(bytes)?.element;
  else if (format === 'jxl') range = jxlEnvelopeRange(bytes)?.box;
  if (!range) return null;
  const content = new Uint8Array(bytes.length - (range[1] - range[0]));
  content.set(bytes.subarray(0, range[0]));
  content.set(bytes.subarray(range[1]), range[0]);
  return content;
}
