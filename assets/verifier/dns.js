// A card's domain over DNS-over-HTTPS and the binding rule on its key dates.
// Mirrors the engine's report/card/binding.rs (C3 frozen).

export const DOH = { cloudflare: 'https://cloudflare-dns.com/dns-query', google: 'https://dns.google/resolve' };

const TXT = 16;
const CNAME = 5;
const NOERROR = 0;
const NXDOMAIN = 3;
// The longest CNAME chain followed inside one answer, as the engine's MAX_CNAME_HOPS.
const MAX_CNAME_HOPS = 8;
const TIMEOUT_MS = 5000;
const DAY = /^\d{4}-\d{2}-\d{2}$/;
const LABEL = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

const isObject = (o) => typeof o === 'object' && o !== null && !Array.isArray(o);
const isString = (s) => typeof s === 'string';

/** Plan 2's public host name rule: every domain the engine signs passes. */
export function isValidDomain(d) {
  if (!isString(d) || d.length > 253) return false;
  const labels = d.split('.');
  return labels.length >= 2 && labels.every((l) => LABEL.test(l)) && !/^\d+$/.test(labels.at(-1));
}

// ASCII case only, as the engine's eq_ignore_ascii_case; one trailing dot dropped
// (Google writes it, Cloudflare does not).
const canonicalName = (n) => n.replace(/\.$/, '').replace(/[A-Z]/g, (c) => c.toLowerCase());
const sameName = (got, want) => isString(got) && canonicalName(got) === canonicalName(want);

/** `want`, then each CNAME target the answer's own records lead to from it. */
function followCnames(answer, want) {
  const chain = [want];
  for (let hop = 0; hop < MAX_CNAME_HOPS; hop++) {
    const next = answer.find((r) => r.type === CNAME && sameName(r.name, chain.at(-1)));
    if (!next) break;
    chain.push(next.data);
  }
  return chain;
}

/** Whether every question names `name` and every TXT or CNAME record is linked to it. */
function answersFor(reply, name) {
  if (!isObject(reply)) return false;
  const { Question = [], Answer = [] } = reply;
  if (!Array.isArray(Question) || !Array.isArray(Answer) || !Answer.every(isObject)) return false;
  const records = Answer.filter((r) => r.type === TXT || r.type === CNAME);
  if (!records.every((r) => isString(r.name) && isString(r.data))) return false;
  const linked = followCnames(records, name);
  return (
    Question.every((q) => isObject(q) && sameName(q.name, name)) &&
    records.every((r) => linked.some((want) => sameName(r.name, want)))
  );
}

/** The TXT data of one resolver's answer for `name`, or null when it does not count. */
async function ask(url, init, name, fetchImpl) {
  try {
    const res = await fetchImpl(url, init);
    if (res.status !== 200) return null;
    const reply = await res.json();
    if (!answersFor(reply, name)) return null;
    if (reply.Status === NXDOMAIN) return [];
    if (reply.Status === NOERROR) return (reply.Answer ?? []).filter((r) => r.type === TXT).map((r) => r.data);
  } catch {
    // A network error, a timeout or a body that is not JSON: the resolver failed.
  }
  return null;
}

/** The `_perf-sentinel.<domain>` TXT records, from Cloudflare, else Google. */
export async function lookupTxt(domain, fetchImpl = fetch) {
  if (!isValidDomain(domain)) return { resolver: null };
  const name = `_perf-sentinel.${domain}`;
  const query = `?name=${encodeURIComponent(name)}&type=TXT`;
  const cloudflare = await ask(
    DOH.cloudflare + query,
    { headers: { accept: 'application/dns-json' }, signal: AbortSignal.timeout(TIMEOUT_MS) },
    name,
    fetchImpl,
  );
  if (cloudflare) return { resolver: 'cloudflare', records: cloudflare };
  const google = await ask(DOH.google + query, { signal: AbortSignal.timeout(TIMEOUT_MS) }, name, fetchImpl);
  return google ? { resolver: 'google', records: google } : { resolver: null };
}

/**
 * `v=ps1; k=<fingerprint>; from=YYYY-MM-DD[; to=…][; revoked=…]`, bare, in one level
 * of quotes or split in quoted strings. Unknown parameters are ignored; any other
 * record, a bad date or a parameter given twice is null, as the engine's parse_txt_record.
 */
export function parseTxtRecord(data) {
  if (!isString(data)) return null;
  let text = data.trim();
  if (text.length >= 2 && text.startsWith('"') && text.endsWith('"')) {
    // `a" "b` splits on its quotes into a, the gap, b: keep the strings.
    text = text.slice(1, -1).split('"').filter((_, i) => i % 2 === 0).join('');
  }
  const fields = text.split(';').map((f) => f.trim()).filter((f) => f !== '');
  if (fields.shift() !== 'v=ps1') return null;
  const record = {};
  for (const field of fields) {
    const at = field.indexOf('=');
    if (at < 0) return null;
    const key = field.slice(0, at).trim();
    if (!['k', 'from', 'to', 'revoked'].includes(key)) continue;
    if (Object.hasOwn(record, key)) return null;
    record[key] = field.slice(at + 1).trim();
  }
  if (!record.k || !record.from) return null;
  return ['from', 'to', 'revoked'].every((d) => record[d] === undefined || DAY.test(record[d])) ? record : null;
}

const earliest = (a, b) => (a === undefined || (b !== undefined && b < a) ? b : a);

/**
 * The binding of key `fp` for a card dated `date`: the records for `fp` merge into the
 * strictest window (the engine's merge_lookups and restrict), and any revocation stops
 * the card whatever its date, since the signer writes that date.
 */
export function bindingState(records, fp, date) {
  let merged;
  for (const r of records.map(parseTxtRecord)) {
    if (r?.k !== fp) continue;
    merged = merged
      ? { k: fp, from: r.from > merged.from ? r.from : merged.from, to: earliest(merged.to, r.to), revoked: earliest(merged.revoked, r.revoked) }
      : r;
  }
  if (!merged) return { state: 'not-published' };
  const record = Object.fromEntries(Object.entries(merged).filter(([, v]) => v !== undefined));
  if (record.revoked) return { state: 'revoked', record };
  const inWindow = record.from <= date && (record.to === undefined || date <= record.to);
  return { state: inWindow ? 'bound' : 'out-of-validity', record };
}
