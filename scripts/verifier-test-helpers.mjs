// Shared by the verifier test suites, never served.
import { crc32 } from 'node:zlib';

const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';

/** The `<payload>.<signature>` token of a compact URL. */
export const tokenOf = (url) => url.split('#c=')[1];

/** `s` with the base64url character at `i` swapped for the one whose 6-bit value is XOR 32. */
export function flip(s, i) {
  const value = ALPHABET.indexOf(s[i]);
  if (value < 0) throw new Error(`not base64url at ${i}: ${s[i]}`);
  return s.slice(0, i) + ALPHABET[value ^ 32] + s.slice(i + 1);
}

export const concat = (...parts) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
};

const ascii = (s) => new TextEncoder().encode(s);
const u32 = (n) => {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n);
  return b;
};

/** PNG chunks after the signature: type and whole byte range, length field through CRC. */
export function pngChunks(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const chunks = [];
  for (let at = 8; at < bytes.length; ) {
    const end = at + 12 + view.getUint32(at);
    chunks.push({ type: String.fromCharCode(...bytes.subarray(at + 4, at + 8)), start: at, end });
    at = end;
  }
  return chunks;
}

/** One PNG chunk, length field through a valid CRC. */
export const pngChunk = (type, data) => concat(u32(data.length), ascii(type), data, u32(crc32(concat(ascii(type), data))));

/** The PNG with its first chunk of `type` replaced by `chunk`, a whole chunk. */
export function replaceChunk(bytes, type, chunk) {
  const c = pngChunks(bytes).find((x) => x.type === type);
  return concat(bytes.subarray(0, c.start), chunk, bytes.subarray(c.end));
}

/** The PNG with its first chunk of `type` moved right after the first `after` chunk, or first when `after` is null. */
export function moveChunk(bytes, type, after) {
  const chunks = pngChunks(bytes);
  const moved = chunks.find((c) => c.type === type);
  const rest = chunks.filter((c) => c !== moved);
  rest.splice(after === null ? 0 : rest.findIndex((c) => c.type === after) + 1, 0, moved);
  return concat(bytes.subarray(0, 8), ...rest.map((c) => bytes.subarray(c.start, c.end)));
}

/** The card's `iTXt` chunk around `text`, its header bytes overridable to break one rule at a time. */
export const cardChunk = (text, { flag = 0, method = 0, lang = '', translated = '' } = {}) => pngChunk('iTXt', concat(
  ascii('perf-sentinel-card\0'), Uint8Array.of(flag, method), ascii(`${lang}\0${translated}\0`), ascii(text)));

/** An envelope text with its keys in another order, or with a space after the first `:`. */
export function reorderEnvelope(text) {
  const o = JSON.parse(text);
  return `{"payload":"${o.payload}","content_sig":"${o.content_sig}","signature":"${o.signature}"}`;
}
export const spaceEnvelope = (text) => text.replace(':', ': ');

/** The PNG without its chunks of `type`. */
export function stripChunk(bytes, type) {
  const kept = pngChunks(bytes).filter((c) => c.type !== type).map((c) => bytes.subarray(c.start, c.end));
  return concat(bytes.subarray(0, 8), ...kept);
}

/** A JPEG XL box with a 32-bit size. */
export const jxlBox = (type, payload) => concat(u32(8 + payload.length), ascii(type), payload);

/** Top-level boxes of a container whose boxes all carry a 32-bit size. */
export function jxlParts(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const parts = {};
  for (let at = 0; at < bytes.length; ) {
    const end = at + view.getUint32(at);
    parts[String.fromCharCode(...bytes.subarray(at + 4, at + 8))] = bytes.subarray(at, end);
    at = end;
  }
  return parts;
}
