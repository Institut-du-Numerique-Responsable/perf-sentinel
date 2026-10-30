# Share cards

A share card prints what Perf Sentinel measured about a waste pattern, such as its volume before a fix and the CO2 priced on that volume, next to an everyday equivalent such as meals or kilometres by train. It is signed, so anyone can check that a key signed these figures and that nobody changed them afterwards. Level 2 of the check ties that key to the signer's domain.

PerfSentinelHub builds the card's figures from its own records and calls the engine to draw and sign it. The engine never reads the Hub's database: it receives a `card.json`, applies every rule on this page, and refuses a card whose figures would claim more than they show. This page is the contract between the two (C2 for the commands, C3 for the payloads), and the reference the verify page at `perf-sentinel.dev/verifier` is built on.

## What a card shows

A card has one of three angles:

| Angle | `angle` | What it says |
|---|---|---|
| Story | `story` | One finding, how big it was before its fix, and since when it is gone. |
| Curve | `curve` | The share of the work that was avoidable, period by period, 2 to 12 bars. |
| Collective | `collective` | How many fixes were confirmed by measurement over a period, on how many services, by family. |

Each card renders in English or French (`lang`) and in one of four formats:

- `card.png`, a single 1080 x 1350 image, the format to post.
- `card.svg`, the same image as an SVG, its text drawn as outlines (see [The SVG](#the-svg)).
- `card.jxl`, the same pixels as a lossless JPEG XL, an archival export LinkedIn does not accept (see [The JPEG XL](#the-jpeg-xl)).
- `card.pdf`, a four-page carousel (the hook, why it matters, the figures with their equivalence, the proof with the method), PDF/A-3b.

All four carry the full signed payload. The QR code on the image and on the carousel's last page holds the verify URL with the compact payload.

## Commands (C2)

Every command prints one JSON line on stdout, which is the contract. stderr carries a human-readable detail and is never parsed.

### `card keygen`

```bash
perf-sentinel card keygen --out-dir <DIR>
```

Generates an Ed25519 key, writes it to `<DIR>/<fingerprint>.pk8` (PKCS#8 v2, mode 0600, never through a symlink, never over an existing file) and prints:

```json
{"fingerprint":"ps1:BjFPvCKhcizo9T39wJxfhA","public_key":"ZFvd2BCZpTwhC04R9ptRGgO7rGWHkq5gtL4Zb1oXdDc"}
```

`public_key` is the raw 32-byte key in base64url, the value to publish. `card keygen` exists on Unix only.

### `card render`

```bash
perf-sentinel card render --input card.json --signing-key <fingerprint>.pk8 --out-dir <DIR> --format png|pdf|svg|jxl
```

Reads `card.json` (64 KiB at most) and the key (1 KiB at most, a regular file, never a symlink, and on Unix readable by its owner only), signs the full and compact payloads, and writes `<DIR>/card.png`, `<DIR>/card.pdf`, `<DIR>/card.svg` or `<DIR>/card.jxl`. Nothing is written unless the card renders. The same input and key give the same bytes. It prints:

```json
{"result":"rendered","files":["card.png"],"fingerprint":"ps1:...","compact_url":"https://perf-sentinel.dev/verifier#c=..."}
```

The private key never travels as a command-line argument: the Hub passes the path of a file in its private temporary directory.

### `verify-card`

```bash
perf-sentinel verify-card <card.png|card.pdf|card.svg|card.jxl|URL> [--keys <published.json>] [--online [--doh <URL>]]
```

Reads a card file (16 MiB at most, a PNG, a PDF, a JPEG XL or an SVG by its first bytes, a JPEG XL starting with its signature box or with `FF 0A`, an SVG starting with `<?xml` or `<svg` after an optional UTF-8 BOM and whitespace) or a verify URL (any target starting with `https://` or `http://`), checks the signature, then the key's binding to the card's domain. It works offline by default. `--keys` reads a local copy of the domain's `/.well-known/perf-sentinel-key` (64 KiB at most). `--online` reads the published keys over the network, see [`--online`](#--online). `--keys` and `--online` exclude each other, and `--doh` needs `--online`. It prints:

```json
{"integrity":"ok","level":2,"domain":"acme.fr","key":"current","reason":"key_published"}
```

| Key | Values |
|---|---|
| `integrity` | `ok`, or `tampered` when the signature does not hold over the payload or the payload's `key` is not its public key's fingerprint. |
| `level` | `1`: the signature holds, nothing ties the key to the domain. `2`: the domain publishes this key for the day the card was signed. |
| `domain` | The card's `org.domain`, or `null`. |
| `key` | `current`, `retired`, `revoked` or `unknown`, what the published list says of the key. |
| `reason` | Why, from the closed list below. |

The signature is checked first, over the exact bytes the card carries. Only an intact payload is then read against the rules: a bad signature is `tampered` whatever the bytes say, and a valid signature over a payload that breaks a rule is `malformed_input`.

The verdict covers the signed payload, not the pixels. Someone can copy a genuine card's `iTXt` chunk, PDF attachment, SVG `<metadata>` element, JPEG XL `pscd` box or QR code onto an edited image, and `verify-card` still prints `"integrity":"ok"`. Compare the payload's figures with the image: the verify page shows them.

### Exit codes and reasons

An error prints `{"result":"<family>","reason":"<reason>"}`, plus `"field"` on `unsupported_characters` when the text comes from `free.title`, `free.team`, `free.label` or `org.name`. Any other text the fonts cannot draw (scope, service, endpoint) gives `unsupported_characters` without `field`.

| Exit | `result` | `card keygen` and `card render` reasons |
|---|---|---|
| 0 | | Done. |
| 1 | `refused` | `period_not_calendar`, `period_not_closed`, `incomplete_period`, `volume_not_avoidable`, `threshold_not_applicable`, `unknown_equivalence`, `equivalence_out_of_scale`, `unsupported_language`. The card would claim what its figures do not show. |
| 3 | `input_error` | `missing_file`, `file_too_large`, `malformed_input`, `signing_key_permissions`, `signing_key_unreadable`, `key_exists`, `output_unwritable`. |
| 5 | `unrenderable` | `compact_too_large`, `unsupported_characters`, `unsupported_version`, `png_encode_failed`, `pdf_export_failed`, `svg_encode_failed`, `jxl_encode_failed`. |

| Exit | `verify-card` line and reasons |
|---|---|
| 0 | `integrity` `ok`. Level 2: `key_published`. Level 1: `no_key_list`, `key_not_published`. |
| 1 | `integrity` `tampered` (`signature_invalid`, `fingerprint_mismatch`), or a key the list does not vouch for (`key_revoked`, `signed_outside_validity`, `public_key_mismatch`), level 1. |
| 3 | `{"result":"input_error"}`: `missing_file`, `file_too_large`, `malformed_input`, `no_card_payload`, `duplicate_card_payload`, `no_domain`. |
| 4 | `--online` could not read the domain's keys: the verify line at level 1 with `binding_check_failed` or `binding_partial`, or `no_http_client` on a build without the HTTP features. |
| 5 | `{"result":"unrenderable","reason":"unsupported_version"}`: the card's format is newer than this engine. |

A payload whose last base64url character is changed to a non-canonical one (trailing bits not zero) is `malformed_input`, exit 3, not `tampered`: both reject the card. An envelope over 64 KiB inside a PDF is `malformed_input`, a PDF over 4 MiB `file_too_large`.

## Payloads (C3)

### `card.json`, the render input

The Hub writes it, the engine reads it. Unknown keys are refused, every number is an integer from 0 to 2^53 − 1 (9 007 199 254 740 991, the largest a browser reads back exactly), and dates are `YYYY-MM-DD`.

| Key | Required | Content and bounds |
|---|---|---|
| `angle` | yes | `story`, `curve` or `collective`. |
| `lang` | yes | `en` or `fr`. Any other value is `malformed_input`. |
| `period` | yes | `{"kind","from","to"}`. `kind` is `week` (Monday to Sunday), `month`, `quarter`, `semester` (January to June, July to December) or `year`. `from` and `to` bound exactly one calendar period of that kind (`period_not_calendar`), which ends before the UTC day of `generated_at_ms` (`period_not_closed`). |
| `scope` | yes | `{"environment","sources","covered_days","restart_days","sampled_permille"}`. `environment` is optional. `sources` is not empty. `covered_days` equals the period's day count (`incomplete_period`), `restart_days` is at most `covered_days`, `sampled_permille` is 0 to 1000. `environment` and `sources`, joined with `", "`, take at most 120 characters. |
| `figures` | yes | Integers whose name carries the unit, read by angle, see below. |
| `story` | on a story only | `{"finding_type","service","endpoint","observed_at","done_at"}`. `observed_at` is optional. `endpoint` takes at most 200 characters. `done_at` falls in or before the period, `observed_at` on or before `done_at`. |
| `before_fix_period` | when the card prints one | `{"from","to"}`, one calendar period of `period.kind`, see [the before-fix period](#the-before-fix-period). |
| `equivalence` | no | An `id` of the equivalence table. Absent, the engine picks one. |
| `free` | yes, `{}` when empty | `{"title","team","label"}`, each optional: title 48 characters, team 48, label 80 (a story only). |
| `org` | yes | `{"name","domain"}`. `name` takes at most 60 characters. `domain` is optional, a public DNS name: lowercase ASCII, at most 253, two labels or more of 1 to 63 `[a-z0-9-]` with no edge hyphen, the last one not all digits. |
| `generated_at_ms` | yes | When the Hub built the card, in epoch milliseconds. |

Every text is trimmed (a value that differs from its trim is refused, never trimmed silently), not empty, and holds no control, bidirectional or invisible character.

`figures` by angle:

| Angle | Keys |
|---|---|
| Story, N+1 and redundant types | `cited_ops`, `before_fix_ops` (optional, the volume in the before-fix period), `co2_low_ug` (optional, only with `before_fix_ops`). No threshold (`threshold_not_applicable`). |
| Story, `slow_*` types | `cited_ms`, `threshold_ms` (optional). |
| Story, `excessive_fanout`, `chatty_service`, `pool_saturation`, `serialized_calls` | `cited_ops`, `threshold_ops` (optional). |
| Curve | `waste_permille` (2 to 12 values of at most 1000, oldest first, the card's period last), `avoidable_ops` and `co2_low_ug` (optional) for the card's period. |
| Collective | `confirmed_fixes_count` (at least 1), `previous_fixes_count` (optional, absent when the Hub cannot count the previous period honestly), `services_count` (1 to `confirmed_fixes_count`), `fixes_by_family_count` (`extra_calls`, `slow_calls`, `call_architecture`, adding up to `confirmed_fixes_count`), `before_fix_ops` (optional, needs an `extra_calls` fix), `co2_low_ug` (optional, only with `before_fix_ops`). |

Only N+1 and redundant findings have an avoidable volume: `before_fix_ops` on any other story type is `volume_not_avoidable`. On a collective, `before_fix_ops` sums the volume before the N+1 and redundant fixes only, and the card names that subset.

The three fixtures of `tests/fixtures/card/` are complete examples: `story.json` (an N+1 with its volume and CO2), `curve.json` (three months) and `collective.json` (seven fixes, four of them N+1 or redundant).

```json
{
  "angle": "story",
  "lang": "en",
  "period": { "kind": "month", "from": "2026-09-01", "to": "2026-09-30" },
  "scope": { "environment": "production", "sources": ["prod-eu"], "covered_days": 30, "restart_days": 0, "sampled_permille": 1000 },
  "figures": { "cited_ops": 41, "before_fix_ops": 1937000, "co2_low_ug": 12400000000 },
  "story": { "finding_type": "n_plus_one_sql", "service": "shop", "endpoint": "GET /cart/{id}", "observed_at": "2026-08-20", "done_at": "2026-08-28" },
  "before_fix_period": { "from": "2026-07-01", "to": "2026-07-31" },
  "free": {},
  "org": { "name": "Acme", "domain": "acme.fr" },
  "generated_at_ms": 1790856000000
}
```

### The before-fix period

The card names the period its "before the fix" figures come from, and both payloads sign it. It is present exactly when the card prints one, and must be:

- on a story with `before_fix_ops`: the calendar period before the one holding `done_at`,
- on a story with a threshold: the period holding `observed_at`, which is then required (the trace its figure comes from),
- on a collective with `before_fix_ops`: the period before the card's.

Anything else is `malformed_input`.

### The full payload

The full payload is what the PNG and the PDF carry. It holds `card.json` with three changes: `v` (1), the resolved `equivalence` object, and the key.

| Key | Content |
|---|---|
| `v` | The format version, `1`. A higher value is `unsupported_version`. |
| `angle`, `lang`, `period`, `scope`, `figures`, `story`, `before_fix_period`, `free`, `org`, `generated_at_ms` | As in `card.json`. |
| `equivalence` | `{"id","per_unit_ug","unit","source","vintage"}`, absent when the card prints no equivalence. |
| `key` | The fingerprint of the signing key. |
| `public_key` | The raw 32-byte Ed25519 public key, base64url. |

The verifier applies the `card.json` rules to a signed payload too, since anyone can make a key. The equivalence is checked on its own terms (non-empty strings, a factor above 0, a count of at least 2 on the printed CO2), never against the embedded table, so a card stays verifiable after a Base Carbone refresh.

The story fixture signs as:

```json
{"angle":"story","before_fix_period":{"from":"2026-07-01","to":"2026-07-31"},"equivalence":{"id":"meal","per_unit_ug":2040000000,"source":"ADEME, Base Carbone","unit":"meal","vintage":"V23.6 (2025-07-03)"},"figures":{"before_fix_ops":1937000,"cited_ops":41,"co2_low_ug":12400000000},"free":{},"generated_at_ms":1790856000000,"key":"ps1:BjFPvCKhcizo9T39wJxfhA","lang":"en","org":{"domain":"acme.fr","name":"Acme"},"period":{"from":"2026-09-01","kind":"month","to":"2026-09-30"},"public_key":"ZFvd2BCZpTwhC04R9ptRGgO7rGWHkq5gtL4Zb1oXdDc","scope":{"covered_days":30,"environment":"production","restart_days":0,"sampled_permille":1000,"sources":["prod-eu"]},"story":{"done_at":"2026-08-28","endpoint":"GET /cart/{id}","finding_type":"n_plus_one_sql","observed_at":"2026-08-20","service":"shop"},"v":1}
```

### The compact payload

The compact payload is what the QR code carries: the angle, the period, the figures the card prints large, the before-fix period, the domain, the public key and the day. Days are epoch days, the days since 1970-01-01 UTC.

| Key | Content |
|---|---|
| `v` | `1`. |
| `a` | The angle: `s` story, `c` curve, `k` collective. |
| `p` | `[kind, from, to]`: the kind letter, then the period's first and last epoch days. Kinds: `w` week, `m` month, `q` quarter, `h` semester, `y` year. |
| `f` | The figures, see below. |
| `r` | `[from, to]`, the epoch days of the before-fix period, absent when the card prints none. Never on a curve. |
| `d` | `org.domain`, absent when the card names none. |
| `k` | The public key, base64url, as `public_key`. |
| `g` | The epoch day of `generated_at_ms`, the day the card was signed. |

| Angle | `f` keys |
|---|---|
| Story | `o` `cited_ops`, `m` `cited_ms`, `b` `before_fix_ops`, `t` `threshold_ops`, `u` `threshold_ms`. One of the sets `o`, `b o`, `o t`, `m`, `m u`. |
| Curve | `w` the card's period's `waste_permille`, `x` the period before the card's. |
| Collective | `n` `confirmed_fixes_count`, `e` `services_count`. |

The story fixture's compact payload:

```json
{"a":"s","d":"acme.fr","f":{"b":1937000,"o":41},"g":20727,"k":"ZFvd2BCZpTwhC04R9ptRGgO7rGWHkq5gtL4Zb1oXdDc","p":["m",20697,20726],"r":[20635,20665],"v":1}
```

### Envelopes

- **File envelope**: `{"payload":"<base64url of the canonical full payload>","signature":"<base64url of the 64-byte Ed25519 signature>"}`, no other key. The PNG carries it in an `iTXt` chunk with the keyword `perf-sentinel-card`, ahead of the image data. The PDF carries it as the attachment `perf-sentinel-card.json`. The SVG carries it as the text of `<metadata id="perf-sentinel-card">`, written as is. The JPEG XL carries it as the payload of a top-level `pscd` box, ahead of the codestream. The envelope takes at most 64 KiB. A second `perf-sentinel-card` chunk ahead of the image data, a second attachment of that name, a second such element, or a second top-level `pscd` box, is `duplicate_card_payload`.
- **Compact URL**: `https://perf-sentinel.dev/verifier#c=<base64url of the canonical compact payload>.<base64url of its signature>`. The token travels in the fragment, which a browser never sends to the server. A reader takes what follows the first `#c=` and splits it on its last `.`.

### Encoding rules

- **Canonical bytes.** A payload is signed as JSON with its keys sorted at every depth, no whitespace, strings in UTF-8 as written (only `"`, `\` and control characters are escaped, and texts hold no control character). A verifier checks the signature over the bytes the card carries and never re-serializes them.
- **Strict base64url.** The alphabet `A-Z a-z 0-9 - _`, no padding, a length that is not 1 modulo 4, and trailing bits at zero, so one changed character never decodes to the same bytes.
- **Fingerprint.** `ps1:` then the base64url of the first 16 bytes of the SHA-256 of the raw 32-byte public key: 26 characters.
- **Integers only.** Every number is an integer from 0 to 2^53 − 1, the unit in the key's name (`_ug`, `_ops`, `_ms`, `_permille`, `_count`, `_days`). A float, a negative number or a larger integer is `malformed_input`.
- **Signature.** Ed25519 (RFC 8032) over the canonical bytes. It is deterministic: one payload and one key give one signature.

### Resolution of C3

The addendum of 2026-10-30 left some points open, which this engine settles:

- The story fields sit under a top-level `story` key, next to `figures`, not inside it.
- `lang` (`en` or `fr`) is signed in the full payload.
- `org.domain` is optional. A card without it can only reach level 1.
- `before_fix_period` is signed in the full payload, and as `r` in the compact one (addendum item 8), so the verify page can name the month before the fix.
- A curve compares the card's period with the one right before it (`x`).
- A file carries the JSON envelope, the URL carries the `<payload>.<signature>` token.

## Key publication

An organization ties its keys to its domain in two places, either or both. The Hub prints both forms, ready to copy.

**DNS.** One TXT record per key under `_perf-sentinel.<domain>`:

```text
_perf-sentinel.acme.fr. TXT "v=ps1; k=ps1:BjFPvCKhcizo9T39wJxfhA; from=2026-09-01"
```

The record is `v=ps1; k=<fingerprint>; from=YYYY-MM-DD[; to=YYYY-MM-DD][; revoked=YYYY-MM-DD]`. It must parse exactly: a record whose first field is not `v=ps1`, with a field missing its `=`, a bad date or a parameter given twice is ignored, so a typo reads as "not published". Unknown parameters are ignored. A record split into several quoted strings is joined.

**Well-known.** `https://<domain>/.well-known/perf-sentinel-key`:

```json
{"keys":[
  {"fingerprint":"ps1:BjFPvCKhcizo9T39wJxfhA","public_key":"ZFvd2BCZpTwhC04R9ptRGgO7rGWHkq5gtL4Zb1oXdDc","from":"2026-09-01","to":null,"revoked":null}
]}
```

`public_key`, `to` and `revoked` may be absent or `null`. Unknown keys and a fingerprint listed twice are refused (`malformed_input`), so a list must name each key once. `--keys` reads the same JSON from a file.

**Rotation and revocation.** A key is valid from `from` to `to`, both included, compared with the UTC day the card was signed (`generated_at_ms`, or `g`). That day is the signer's own claim, written by whoever holds the key.

| The list says | Card signed | Result |
|---|---|---|
| Nothing (no list read) | | Level 1, `unknown`, `no_key_list` |
| Not this fingerprint | | Level 1, `unknown`, `key_not_published` |
| Another `public_key` for this fingerprint | | Level 1, `unknown`, `public_key_mismatch`, exit 1 |
| `revoked` | any day | Level 1, `revoked`, `key_revoked`, exit 1 |
| Valid, no `to` | before `from` | Level 1, `current`, `signed_outside_validity`, exit 1 |
| Valid, with `to` | outside `from`..`to` | Level 1, `retired`, `signed_outside_validity`, exit 1 |
| Valid, no `to` | inside its validity | Level 2, `current`, `key_published` |
| Valid, with `to` | inside its validity | Level 2, `retired`, `key_published` |

To rotate, publish the new key with its `from` and set `to` on the old one: cards signed with the old key stay at level 2, `retired`. That window rests on the signer's own date, so a stolen key that is only retired can still date a card inside it. To withdraw a key that may have leaked, set `revoked`: every card it signed drops to level 1, `key_revoked`, whatever date the card states, because a stolen key could date a card before any revocation day. Honest cards that key signed lose level 2 too, and no third-party timestamp tells them apart. Revoke a key that may have leaked, retire one that did not. The `revoked` day is the first day the key may have leaked, not the day it was withdrawn: it changes no verdict and only tells a reader since when the key may have been exposed. When the DNS record and the well-known file both list a key, every limit either sets applies, so `revoked` wins over `to`, and `to` over no limit.

## `--online`

`verify-card --online` reads the card's domain (`org.domain`, or `d`), so a card without one gives `no_domain`. It asks two places at once, through the engine's shared HTTP client:

1. The TXT records of `_perf-sentinel.<domain>`, from a DNS-over-HTTPS JSON service (`application/dns-json`): `https://cloudflare-dns.com/dns-query` by default, or the `https://` URL `--doh` names. The engine looks up only a domain that passes the card's own domain rule, and reads an answer only when its question and its TXT records name `_perf-sentinel.<domain>`, case and one trailing dot aside. A TXT record reached through CNAME records present in the same answer counts: the engine follows that chain inside the answer, up to 8 hops. An answer whose names cannot be linked to `_perf-sentinel.<domain>` that way reads "could not check".
2. `https://<domain>/.well-known/perf-sentinel-key`, on the domain itself, with no port and no user part.

Each lookup has 5 s and reads 64 KiB at most. No redirect is followed: a domain whose apex redirects to `www` fails the well-known lookup, so publish the file on the apex, or rely on the DNS record. A 404, an NXDOMAIN or no `v=ps1` record means "publishes nothing". A timeout, an error status, a redirect or a body that is not the expected shape means "could not check". The two answers merge by fingerprint. When a lookup could not check and the other found no key, the result is `binding_check_failed` at level 1, exit 4: "could not check" never reads as "not published", since the missing answer might have revoked the key. When one lookup could not check and the other found the key, the result is `binding_partial` at level 1, exit 4, for the same reason: the unread source might be the one that revokes the key. An NXDOMAIN or a 404 is an answer, not a failure, so a domain that publishes in one place only still reaches level 2.

These are the only network calls the engine makes for a card, and only when `--online` is passed. Without it, `verify-card` stays offline.

## Equivalences

The equivalences come from the ADEME Base Carbone. The table ships embedded in the binary, which never fetches it at runtime.

<!-- equivalences:begin -->
Embedded vintage: Base Carbone `V23.6 (2025-07-03)`.

| `id` | Base Carbone id | Base Carbone element | Factor | `per_unit_ug` | `unit` | `label` | Validity |
|---|---:|---|---|---:|---|---|---|
| `tgv_km` | 43256 | TGV, 2022 | 0.00293 kgCO2e/passager.km | 2_930_000 | `passenger_km` | km by high-speed train | avr-24 |
| `car_km` | 43791 | Voiture, Motorisation moyenne | 0.256 kgCO2e/km | 256_000_000 | `vehicle_km` | km driven by an average car | 2026 |
| `meal` | 20682 | Repas | 2.04 kgCO2e/repas | 2_040_000_000 | `meal` | average meals | déc-21 |
| `smartphone` | 27013 | Smartphone, de plus de 5,5 pouces | 39.1 kgCO2e/unité | 39_100_000_000 | `unit` | smartphones | 20/12/2021 |
<!-- equivalences:end -->

Base Carbone keeps a row `Valide générique` after its validity period has lapsed, so the last column shows how old each factor is.

### Selection rules

1. Each row is a Base Carbone `Elément` line, `Valide générique`, located in `France continentale`, with the unit the script expects for it.
2. The unit is one everyday countable thing: a passenger kilometre, a kilometre, a meal, a unit. Never per euro, per kg or per kWh.
3. Sorted by factor, each row is at most 100 times the previous one, so a CO2 figure always has a row that gives a readable count.
4. When the label does not tell variants apart, the highest factor wins, so the count stays an "at least". The smartphone row is the over 5.5 inch model (39.1 kg), not the "classique" one (16.5 kg).
5. No flight, and no comparison implying a cost the measurement does not show.

`scripts/refresh-equivalences.py` enforces rules 1 and 2 and exits 1 when a row fails them. The tests enforce rule 3.

### How the count is computed

The count uses the CO2 as the card prints it, never the raw measurement. The printed CO2 keeps two significant figures, rounded down, and the count is a floor division of that figure by the factor. A saving of 12.4 kg prints "12 kg" and 5 average meals (12 ÷ 2.04 = 5.88), where the raw 12.4 kg would have given 6. Both roundings go down, so the card never claims more than was measured.

When the caller does not choose an equivalence, the card takes the largest factor that still counts at least 2. A chosen equivalence must count between 2 and 1000.

### Attribution and licence

Every card prints the factor it used, the source "ADEME, Base Carbone" and the embedded vintage. The Base Carbone is published under the Licence Ouverte 2.0 (Etalab), which allows reuse with attribution.

### Refresh

```bash
python3 scripts/refresh-equivalences.py
cargo fmt -p perf-sentinel-core
```

The script reads the dataset through the data.ademe.fr API, rewrites `crates/sentinel-core/src/score/equivalence_data.rs` and the table between the `equivalences:begin` and `equivalences:end` markers of this page and of its French mirror. Edit outside the markers only. The `refresh-datasets` workflow runs it twice a year and opens a pull request, and release step 2.5 audits `EQUIVALENCE_TABLE_VINTAGE` (see [`RELEASE-PROCEDURE.md`](./RELEASE-PROCEDURE.md)).

## Card wording

Every sentence a card prints is listed here, the English ones on this page and the French ones on the French page. The test `docs_list_every_story_sentence` fails when a story sentence changes in `crates/sentinel-core/src/report/card/copy.rs` and not here.

A story sentence has two forms. Without a threshold, `{lead}` is the label the user gave the action followed by `: `, or `For one action in the application, `. With a threshold, `{before}` is the before-fix period ("In August 2026"), and the label, when there is one, leads the sentence. `{n}` is the figure the story cites and `{threshold}` the alert threshold.

| Finding types | Without a threshold | With a threshold |
|---|---|---|
| `n_plus_one_sql`, `n_plus_one_http`, `n_plus_one_messaging` | `{lead}the application fetched information {n} times instead of once.` | None, these types have no threshold. |
| `redundant_sql`, `redundant_http` | `{lead}the application asked {n} times for the same information.` | None. |
| `slow_sql`, `slow_http`, `slow_messaging` | `{lead}the slowest runs of one step took {n} or more.` | `{before}, before the fix, the slowest runs of one step took {n} or more. It still runs and no longer triggers the slowness alert, set at {threshold}.` |
| `excessive_fanout` | `{lead}a single step set off {n} operations.` | `{before}, before the fix, a single step set off {n} operations. It still runs and no longer triggers the alert, set at {threshold} operations.` |
| `chatty_service` | `{lead}the application reached out to other parts of the system {n} times.` | `{before}, before the fix, the application reached out to other parts of the system {n} times. The action still runs and no longer triggers the alert, set at {threshold} times.` |
| `pool_saturation` | `{lead}{n} operations ran at the same moment, at the peak.` | `{before}, before the fix, {n} operations ran at the same moment, at the peak. The action still runs and no longer triggers the alert, set at {threshold} operations.` |
| `serialized_calls` | `{lead}the application took {n} steps one after the other.` | `{before}, before the fix, the application took {n} steps one after the other. The action still runs and no longer triggers the alert, set at {threshold} steps.` |

The Hub sends a threshold, and so gets the threshold form, only when it saw passes below the threshold and no detection on a day after the fix (contract C1). Otherwise the card takes the form without a threshold and says since when the finding is no longer detected.

The other lines:

| Where | Line |
|---|---|
| Story, under the sentence | `Seen on {date}.` (when `observed_at` is set), then `Not detected since {date}.` (without a threshold) |
| Story CO2 line | `{before}, before the fix: {n} unnecessary operations, at least {co2} CO2e (estimate)` |
| Curve CO2 line | `Avoidable in {period}: {n} operations, at least {co2} CO2e (estimate)` |
| Collective CO2 line | `{before}, before the fix of work done more than once: ...` with one N+1 or redundant fix, `{before}, before the {n} fixes of work done more than once: ...` with several |
| Equivalence | `Equal to at least {n} {unit}`, then `1 {unit} = {factor} CO2e. Source: {source}, {vintage}` |
| Collective | `{n} fixes confirmed by measurement, across {s} services`, with a trailing comma and then `against {p} the month before` (or week, quarter, half-year, year) when the Hub counted the previous period |
| Collective families | `What was fixed`: `Work done more than once`, `Slow steps`, `How the work was ordered` |
| Curve | `Share of the work that was avoidable`, `in {period}, against {x} in {previous period}`, `The lower, the better.`, and the legend `Share of the work that was avoidable, month by month` (or week by week, quarter by quarter, half-year by half-year, year by year) |
| Slow story bars | `Slowest runs, before the fix` and the dashed `Alert threshold` |
| Proof | `Fix confirmed by measurement` (story) or `Fixes confirmed by measurement` (collective), none on a curve, which names no fix |
| Scope | `Measured on {sources}, {environment}, {days} days`, then `, {n} days with a restart` and `, on {p} of traffic` when they apply |
| Organisation | `Organisation, as declared: {name}` |
| Footer | `Generated on {date}`, `Measured with Perf Sentinel, free software carried by the Institut du Numérique Responsable` |
| Verify | `Check it and read the method: perf-sentinel.dev/verifier` |
| Carousel, page 2 | `Every unnecessary operation costs server time, energy and sometimes waiting.` and `Measuring them shows where the application can do the same work with less.` |
| Carousel, page 4 | `Check these figures` and `Method:` followed by the link to this page |

In the `Measured with` line, the two names Perf Sentinel and Institut du Numérique Responsable print in bold, underlined, in the brand green `#107a56`, and each stays whole on one line. The words are the same as in the table.

Every figure is rounded down: counts are exact below 10 000, then two significant figures ("1.9 million"), durations in whole milliseconds below 100 ms, then seconds with one decimal, and a share under 0.1 % reads "under 0.1%". The units an equivalence counts are `average meal(s)`, `km by high-speed train`, `km driven by an average car` and `smartphone(s)`.

A story CO2 line covers the before-fix period only, a curve's covers the card's period, and a collective's covers the before-fix period of its N+1 and redundant fixes only. A curve's scope line counts the days of every period it shows, from the first bar to the last.

## The SVG

`card.svg` draws the same operations as `card.png`. Its text is the glyph outlines of the bundled DM Sans fonts, one `<path>` per piece of text, not a `<text>` element with an embedded font:

1. The SVG draws exactly what the layout measured. A viewer's own text engine applies kerning and its own metrics, so a line measured to fit could overflow, or wrap where the PNG does not.
2. The viewer needs no font. Several editors ignore an embedded `@font-face` and substitute a system font, which the layout never checked its glyphs against.
3. The bytes depend only on the card and the bundled fonts, never on a font the viewer has.
4. Embedding the two DM Sans files would add about 143 KiB before a single glyph, and subsetting them needs a crate the engine does not have.

The cost: the text cannot be selected or searched, and a screen reader reads the `<title>` as the image's name. The title is the card's `free.title`, else `Perf Sentinel card` (`Carte Perf Sentinel` in French), the same as the PDF metadata. DM Sans is under the SIL Open Font License, which allows both forms.

The file starts with a fixed header:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="1080" height="1350" viewBox="0 0 1080 1350" xml:lang="en" role="img">
<title>{title, XML-escaped}</title>
<metadata id="perf-sentinel-card">{envelope}</metadata>
```

One line per drawing operation follows, in the PNG's order, then `</svg>`. The only elements are `svg`, `title`, `metadata`, `defs`, `linearGradient`, `stop`, `path`, `rect` and `circle`: no script, no style, no link, no image, no `<text>`. Numbers are fixed point with at most two decimals, so the same card gives the same bytes on one build target.

The envelope only holds JSON punctuation (`{`, `}`, `"`, `:`, `,`) and base64url characters, so it needs no XML escaping. `card render` checks every byte before writing and stops with `svg_encode_failed` otherwise, which the canonical envelope never triggers. `verify-card` reads the envelope without an XML parser: the engine writes the element in one form only, and a parser would be a new dependency and more code exposed to a file a stranger sends. It looks for the exact string `<metadata id="perf-sentinel-card">` and takes its text up to the next `</metadata>`:

| The file | Reason |
|---|---|
| Over 16 MiB | `file_too_large` |
| Not UTF-8, or not starting with `<?xml` or `<svg` after an optional UTF-8 BOM and whitespace | `malformed_input` |
| No such element | `no_card_payload` |
| The element twice | `duplicate_card_payload` |
| No `</metadata>`, more than 64 KiB, or a byte outside the set above | `malformed_input` |

Like the PNG chunk, the envelope proves the payload, not the drawing around it.

Sizes measured on the test fixtures, envelope included:

| Fixture | English | French |
|---|---|---|
| Story | 319 KiB | 324 KiB |
| Curve | 297 KiB | 293 KiB |
| Collective | 325 KiB | 338 KiB |

## The JPEG XL

`card.jxl` holds the pixels of `card.png`, lossless: the engine rasterizes the card once and hands the same 8-bit RGB bytes to both encoders, so a decoded `card.jxl` equals a decoded `card.png`, byte for byte. The encoder is jixel (pure Rust, no dependency, default features and SIMD off), at speed `Medium` on one thread, so the bytes do not depend on the host's core count. A test decodes the file with jxl-rs, the decoder Chrome and Firefox ship, and with jxl-oxide, a second, independent decoder, and compares both with the PNG's pixels.

Sizes measured on the test fixtures (English, envelope included):

| Fixture | `card.jxl` | `card.png` | Ratio |
|---|---|---|---|
| Story | 44,719 B | 111,986 B | 0.40 |
| Curve | 41,383 B | 110,546 B | 0.37 |
| Collective | 42,011 B | 116,719 B | 0.36 |

JPEG XL is an archival or high-quality export. The PNG stays the format to post: LinkedIn does not accept JPEG XL ([its help page](https://www.linkedin.com/help/linkedin/answer/a564109) lists GIF, HEIF/HEIC on iOS, JPEG, PNG and WEBP for images), and browser support is recent and uneven: Chrome turns it on by default from version 155, Firefox from 158, Safari has partial support since 17, and Edge does not display it ([caniuse](https://caniuse.com/jpegxl), checked 2026-10-30).

jixel writes a bare codestream (it starts `FF 0A`) and has no API for a custom box, so the engine writes the container itself. The layout is fixed:

```text
00 00 00 0C  4A 58 4C 20  0D 0A 87 0A         signature box
00 00 00 14  ftyp  jxl   00 00 00 00  jxl     ftyp box
size(8 + n)  pscd  <n envelope bytes>         the card's box
size(8 + m)  jxlc  <m codestream bytes>       the codestream
```

Sizes are 32-bit big-endian and count the 8-byte header. Nothing follows `jxlc`. The `pscd` payload is the file envelope as is, the same bytes as the PNG's `iTXt` text. It comes before the codestream, so a reader finds it in the first few hundred bytes.

`pscd` is a private box type. libjxl's `decode.h` lets an application add a box whose type neither starts with `jxl` or `JXL` nor reuses a defined type. jxl-rs and jxl-oxide skip it, and the test above decodes the card with its `pscd` box. An `xml ` box would force an XMP packet with XML escaping, a second canonical form that metadata tools rewrite. An `Exif` box is a TIFF structure and the box pipelines strip most often. A `jumb` box (JUMBF, the C2PA container) is far heavier than a card needs.

`verify-card` reads the envelope by walking the top-level boxes, without decoding the codestream: a decoder would be a large dependency exposed to a file a stranger sends, for bytes the reader never needs. A box of size 1 has a 64-bit size after its type, a box of size 0 runs to the end of the file. Every box other than `pscd` is skipped by its length and never read, so a `pscd` box nested inside another box is not seen.

| The file | Reason |
|---|---|
| Over 16 MiB | `file_too_large` |
| A bare codestream (starts `FF 0A`) | `no_card_payload` |
| Not starting with the signature box | `malformed_input` |
| A box smaller than its header, running past the end of the file, or cut inside its header | `malformed_input` |
| No top-level `pscd` box | `no_card_payload` |
| Two top-level `pscd` boxes | `duplicate_card_payload` |
| A `pscd` payload over 64 KiB or not UTF-8 | `malformed_input` |

Like the PNG chunk, the envelope proves the payload, not the pixels.

## QR code

The QR code is version 12 at most, error correction L: the card lives on a screen, and the compact URL does not fit in version 12 at level M. Version 12-L holds 367 bytes. The prefix and the signature take 124 characters of the URL, which leaves about 182 bytes for the compact JSON. With the fixtures' figures, the longest domain that fits is 35 characters on a story with `r`, 42 on a collective and 58 on a curve. Ten-digit volumes bring the story down to 28 characters. A longer domain is `compact_too_large`, never truncated, so the Hub should check the length before it offers the domain.

## Method link

The carousel's last page prints `Method:` and the address of this page on GitHub (`.../docs/SHARE-CARD.md`, or `.../docs/FR/SHARE-CARD-FR.md` on a French card), broken only after a `/`. The image prints the verify page, `perf-sentinel.dev/verifier`, which links back here.

The limits of what a card proves are listed in [`LIMITATIONS.md`](./LIMITATIONS.md#share-cards).
