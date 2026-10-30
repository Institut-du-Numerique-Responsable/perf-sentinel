// Shared by the verifier test suites, never served.

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
