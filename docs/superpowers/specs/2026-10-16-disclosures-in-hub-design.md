# Periodic disclosures in the Hub

## Why

`perf-sentinel disclose` writes a quarterly or yearly energy and carbon report
(`perf-sentinel-report/v1.7`) with a canonical `content_hash`, and
`disclose --emit-attestation` plus `cosign sign-blob` sign it. Today the three files
leave the CI as job artifacts and travel by hand. Nobody outside the platform team can read
a JSON report, nothing tells a recipient whether the copy they hold is the published one, and
there is no place where an organisation's past disclosures sit together.

The Hub becomes that place: it receives what the CI produced and signed, checks what it can,
renders a PDF a non-technical reader can follow, keeps the files, and hands them on through a
restricted page, an expiring share link and a signed webhook.

## Goal and success

An operator's CI publishes a signed G2 report to the Hub at the end of its disclosure job. A
Hub admin sees it on a Disclosures screen, downloads the four files, creates a share link and
sends the PDF's address to an auditor. The auditor opens the PDF without an account, and
`perf-sentinel verify-hash --url https://hub/share/<token>/report.json` answers TRUSTED from
their own machine. A webhook receiver gets a signed "published" event. Success is a lab
scenario doing that with a hash-only report (PARTIAL, since the lab has no Sigstore identity),
plus the same run with the report's figures edited, which the Hub refuses, and the engine's
`disclosure-signing.yml` workflow proving TRUSTED through a real keyless signature on a PDF.

## Decisions taken

From the 14 October proposals (artifact "Divulgations dans le Hub") and the 16 October answers.

- **The Hub publishes, it never produces.** `disclose` and the signature stay in the CI or with
  the operator. The Hub has no access to the daemon archives, no Sigstore identity and no
  cosign, and `generated_by` has no "hub" value.
- **The CI publishes, with an import key.** A POST at the end of the job that produces and
  signs, on the model of `/api/import/findings`. One shared key,
  `Hub:Disclosures:ImportKey`. The signature is the real proof of origin, the key only gates
  the door, and a report published by mistake is withdrawn.
- **Two Hub-level permissions.** `disclosures.read` (see the screen, the list, the detail,
  download the files) and `disclosures.manage` (create and revoke share links, retry a
  webhook, withdraw a disclosure, render the PDF again). Granted like `findings.read`, by role
  and group, on `*` only.
- **Three ways out.** Download from the restricted screen, an expiring share link that serves
  files only, and a webhook declared in configuration.
- **The share link serves files only, no HTML page.** The PDF already carries the summary
  (page 1) and the verification steps (page 5), so a page would duplicate it while adding an
  anonymous HTML surface to secure and maintain. The lasting point of comparison is the
  organisation's own transparency page, which the PDF's QR code opens.
- **Internal reports behind an option, off by default.** `Hub:Disclosures:AllowInternal`.
  "Internal" means `report_metadata.confidentiality_level: internal`, whatever the
  applications' granularity (G1 or G2). Once on, an internal report is visible to every holder
  of `disclosures.read`, and never shared by link nor pushed by webhook. Only `public` reports
  (which the validator already restricts to G2) leave the Hub.
- **A PDF/A-3b rendered by the engine**, in English only, carrying the three source files as
  attachments.
- **Withdraw, never delete.** A wrong disclosure is withdrawn with a reason and replaced. Its
  files stay, its links die, a "withdrawn" webhook goes out.
- **Files in SQLite BLOBs**, not on disk, so the existing `backup` (`VACUUM INTO`) covers them.
- **No SMTP client.** Email goes through the operator's gateway, which accepts a webhook.

Already shipped in 0.28.0 and not part of this work: the attestation subject is the canonical
`content_hash`, `verify-hash` checks that binding (TRUSTED needs it), and `disclose --intent
official` refuses an archive that does not reach the first day of the period.

## Anti-goals

- The Hub never says "verified" about a signature. It checks the content hash and the
  attestation binding with its embedded engine, and says so. The signature is declared by the
  report and checked by whoever runs `verify-hash`.
- No anonymous HTML. Nothing under `/share/` renders a page, follows a redirect or reflects
  input.
- No SQLite write per anonymous request.
- No outbound call the configuration does not name: webhooks only, each declared.
- A Hub without disclosures configured behaves byte for byte as today, `/api/status` included,
  apart from the new permissions appearing in the permission list.

## Out of scope

Daemon-triggered disclosures (`[reporting] disclose_output_path`), `perf-sentinel sign`, a
public "is this document still in force" route, `--intent audited`, and re-signing anything in
the Hub.

## Engine (perf-sentinel 0.31.0)

### `disclose-pdf`

A new subcommand behind a new cargo feature `pdf` (`pdf = ["dep:krilla", "dep:fast_qr"]`),
in the CLI's default features like `tui`, so the release binary and the image carry it.

```
perf-sentinel disclose-pdf --report report.json [--attestation attestation.intoto.jsonl]
                           [--bundle bundle.sig] [--expected-identity ID --expected-issuer URL]
                           --output report.pdf
```

- **Checks the schema version first.** `perf-sentinel-report/v1.0` to `v1.7` are accepted,
  and a section an older version lacks prints "not reported". `PeriodicReport` drops unknown
  fields, so a newer report would fail the hash for the wrong reason: the version is read
  from the raw JSON before anything else.
- **Exit codes**, a contract the Hub relies on, with one JSON line on stdout naming a reason
  from a closed set:
  - 0: rendered.
  - 1: refused, the document is not what it claims. Reasons `content_hash_mismatch` (the hash
    does not recompute) and `binding_mismatch` (`check_attestation_binding` answers
    `Mismatch`). Nothing is written. A rendered PDF therefore stands for both checks.
  - 3: input error, the file is not a readable report (`malformed_report`, `missing_file`).
  - 5: cannot render, though nothing says the document is wrong. Reasons
    `unsupported_schema_version` (newer than this engine) and `unsupported_characters` (text
    the bundled fonts cannot draw, listed on stderr: a missing glyph would break PDF/A, and
    replacing characters on an official document is not acceptable).
- `verify-hash` gains the same schema version check, exit 5 with the same reason, instead of
  a misleading hash failure on a newer report.
- **PDF/A-3b, A4, English.** `report.json` attached with relationship `Source`, the
  attestation and the bundle with `Supplement`, byte for byte as given.
- **Deterministic.** Same inputs, same bytes: document dates come from
  `report_metadata.generated_at`, never from the clock, and attachment order is fixed. A test
  renders twice and compares.
- **Five pages**, as in the 14 October mock-ups:
  1. Cover: organisation, period, three figures (energy, carbon, avoidable share), the declared
     integrity level, the document code, and a QR code to
     `notes.reference_urls["transparency"]` when the report has one (nothing otherwise, never a
     Hub or share-link URL). `disclose` copies `reference_urls` from the org config, so the key
     is documented there as the organisation's own transparency page.
  2. At a glance: the carbon range (the ~2× band), the canonical and operational readings of
     avoidable waste, coverage against the 75 % official threshold, the ESRS E1 crosswalk.
  3. Applications: energy share of each, declared against measured scope.
  4. Method and limits: detectors, and every disclaimer copied verbatim.
  5. Verify this document: a "who checked what" table, the attachments with their SHA-256,
     three steps without a terminal, and the `verify-hash` command with the expected identity
     and issuer filled in when given.
- **The document code** is the report's `content_hash`, printed in full on pages 1 and 5 and
  grouped by four characters so it can be read aloud and compared on a transferred copy.
- **Libraries.** krilla and fast_qr, pure Rust. The 14 October spike used krilla 0.8.2 and
  fast_qr 0.14.0: PDF/A-3b validated by veraPDF, deterministic, about 30 KB per document,
  binary from 0.34 MB to 2.67 MB before fonts. Fonts: DM Sans and JetBrains Mono as static
  TTFs (SIL OFL, licence files shipped). Take the latest versions at implementation time and
  re-check the size.
- **Line breaking** is a small tested line splitter over the font's advances, as in the spike,
  with page breaks and truncation of the hash in monospace.

### `verify-hash --report report.pdf`

`verify-hash` accepts a PDF: it extracts `report.json`, the attestation and the bundle from
the attachments and runs the usual checks on them, so a recipient who only holds the PDF can
verify it. A PDF without an attached `report.json` is an input error.

- Any well-formed PDF is accepted, not only the renderer's own output: a copy re-saved by a
  viewer or a document system (object streams, xref streams, recompression) must still verify.
  That needs a PDF reader, so a parsing dependency (lopdf or equivalent, latest version at
  implementation time) behind the `pdf` feature. A build without `pdf` says so and exits 3.
- The PDF is untrusted input: the file goes through the existing 64 MiB capped read, each
  attachment is capped at the size limits the JSON paths already use, nesting and object
  counts are bounded, and a fuzz target joins `fuzz/` for the extraction.

### CI

A veraPDF job (PDF/A-3b profile) on pull requests touching the `pdf` paths, on a rendered
fixture. Fixtures for the renderer come from the schema's official G2 example.

### Docs

`REPORTING.md` (publishing to the Hub, `disclose-pdf`, verifying a PDF), `SUPPLY-CHAIN.md`,
the disclosure-signing workflow example extended with the POST to the Hub, and the FR
mirrors. The CHANGELOG notes the new subcommand and feature, and the binary size increase.

## Hub (PerfSentinelHub 0.7.0)

0.6.0 (correlation history) ships first. 0.7.0 repins the engine to 0.31.0, which the
render needs.

### Configuration

```
Hub:Disclosures:ImportKey            shared key, 32+ characters, no control characters
Hub:Disclosures:AllowInternal        false by default
Hub:Disclosures:ExpectedIdentity     signer identity printed in the verify command
Hub:Disclosures:ExpectedIssuer       OIDC issuer printed in the verify command
Hub:Disclosures:ShareLinkDays        7 by default, 30 at most
Hub:Disclosures:Webhooks[]           Name, Url, Secret, Confidentiality, IncludeShareLink,
                                     ShareLinkDays
```

Validation at startup:
- The import key must not equal a source's `ImportApiKey` nor a machine-token secret.
- `AllowInternal` requires access control on: an open Hub would show G1 detail to anyone.
- A webhook needs an absolute https URL (http only to loopback), a secret of 32+ characters,
  and a confidentiality of `public` or `internal`. `internal` is refused while G1 never leaves
  the Hub. The field exists so the rule is stated in one place.
- `disclosures.read` and `disclosures.manage` are refused on a project grant: grant on `*`.

Disclosures are on when `ImportKey` is set. Without it the route answers 404, the tab stays
hidden, and nothing else changes.

### Permissions

`AccessPermissions` gains `DisclosuresRead = "disclosures.read"` and
`DisclosuresManage = "disclosures.manage"`, both in `All`, so only the built-in admin role
holds them by default. reader and member do not gain them. `MachineTokenPermissions` stays
`[findings.read]`. A custom role can carry them, granted on `*` only, like `admin`. Without
access control (open Hub), the screen is visible to whoever reaches the Hub, as every other
screen is, which is why G1 needs access control on.

### Publication: `POST /api/import/disclosures`

- Anonymous route with `X-API-Key` against `ImportKey`, compared as SHA-256 in constant time
  (`IsAuthorized`).
- `multipart/form-data` with parts `report` (required), `attestation` and `bundle` (both
  optional: an unsigned official report is accepted and shown as such). 8 MiB per request,
  checked before reading.
- One publication at a time: its own gate of one, 503 + `Retry-After: 1` when busy. The
  request stays synchronous, capped at 60 seconds.
- Steps: parse the report envelope (schema version, `report_uuid`, confidentiality,
  organisation, period), refuse a G1 when `AllowInternal` is off (422), write the parts to a
  private temporary directory, run `disclose-pdf` through `EngineProcess` with the expected
  identity and issuer, store everything, delete the temporary directory.
- Outcomes of the render, read from the exit code and the reason line, never from stderr
  (the Hub's rule that engine stderr never reaches a response stands):
  - 0, rendered: stored with `content_hash` and binding marked checked by engine X.Y.Z.
  - 1, refused: the publication is refused, 422 with the reason (`content_hash_mismatch`,
    `binding_mismatch`). Edited figures never reach the Hub.
  - 3, malformed: 422 `malformed_report`, nothing stored.
  - 5, or an engine without `disclose-pdf` (the engine probe reads `--help`): the files are
    stored without a PDF and the screen says "not checked here" with the reason. A
    `disclosures.manage` holder can render again once the engine is upgraded.
- Idempotence and signing after the fact. `content_hash` leaves out the signature fields
  (`POST_SIGN_FIELDS`), so the same report before and after signing has the same hash.
  - Same `report_uuid`, same hash, same bytes: 200, nothing changes.
  - Same `report_uuid` and hash, the stored copy has no attestation and the new one does: the
    new copy replaces the stored one (each copy is kept exactly as received), the PDF is
    rendered again, the screen notes when the signature was added. No new webhook event: the
    "published" event already named the same `content_hash`.
  - Any other difference under the same `report_uuid`: 409. A new report gets a new uuid.
- Response: the disclosure id and the checks' results, as JSON.
- Files are stored exactly as received, never re-serialised.

### Storage: schema V12

- `disclosures`: id, report_uuid (unique), organisation, period type, from, to, intent,
  confidentiality, content_hash, declared integrity level, the three received BLOBs, the PDF
  BLOB (nullable), engine version, hash and binding results, published_at, withdrawn_at,
  withdrawn_reason, withdrawn_by.
- `disclosure_share_links`: id, disclosure_id, token_hash (SHA-256, unique), created_by,
  created_at, expires_at, revoked_at, revoked_by, downloads, last_used_at.
- `disclosure_deliveries`: one row per webhook and event: webhook name (never its URL), event,
  state, attempts, last attempt, next attempt, last status.

### Reading and the screen

- `GET /api/disclosures` (list, newest period first, withdrawn included and marked) and
  `GET /api/disclosures/{id}` (detail with checks, links and deliveries), behind
  `disclosures.read`. File downloads under `/api/disclosures/{id}/files/{name}`, names fixed:
  `report.json`, `attestation.intoto.jsonl`, `bundle.sig`, `report.pdf`, `SHA256SUMS`.
- Actions behind `disclosures.manage`: create and revoke a share link, withdraw with a
  reason, retry a delivery, render again.
- Launcher: a Disclosures tab, shown only to a holder of `disclosures.read` (or on an open
  Hub with disclosures on). Registered like Correlations (`PSL.screenOf`, `render()`,
  `onRoute()`). From the mock-ups:
  - a period rail (published, withdrawn, missing quarters at a glance).
  - one card per disclosure with four figures, coverage drawn against the 75 % notch (orange
    under it), publication facts, downloads and sharing.
  - a detail view with the carbon range on a log scale, the applications, the "who checked
    what" table, the verify command, the files, share links and deliveries.
  - Declared facts are drawn dashed and the Hub's own checks solid, following the Hub's
    existing "declared, not measured" convention.
- Downloads go through `fetch` then a Blob, so an expired session leads back to sign-in rather
  than saving a 401 body as `report.json`. The launcher has no such helper yet.

### Share links: `/share/{token}/<file>`

- Created by a `disclosures.manage` holder, on public reports only, not on a withdrawn disclosure. Token
  `pshare_` plus 32 random bytes in base64url, shown once, stored as SHA-256 like terminal
  tokens. Default `ShareLinkDays`, 30 days at most, revocable.
- Serves exactly the five fixed file names. Anything else, the bare `/share/{token}/`
  included, answers 404. The same 404 for an unknown, expired, revoked or withdrawn token.
- `verify-hash --url https://hub/share/<token>/report.json` works unchanged: it fetches the
  attestation and the bundle beside it.
- Headers: `Referrer-Policy: no-referrer`, `Cache-Control: no-store`,
  `X-Content-Type-Options: nosniff`, `X-Robots-Tag: noindex`, and the right `Content-Type`
  and `Content-Disposition` per file.
- Downloads are counted in memory and flushed to the link's row once a minute. The client
  address is recorded only when `TrustedProxyNetworks` is set.
- The token ends up in the Ingress access logs. The Helm ConfigMap sets the ASP.NET hosting
  log level to Warning and the docs say so.

### Webhooks

- One POST per declared webhook on "published" and "withdrawn", for disclosures whose
  confidentiality the webhook accepts (`public` only, see validation).
- A dedicated named `HttpClient`, no redirect, no cookie, `Hub:HttpTimeout`.
- `X-PerfSentinel-Timestamp` and `X-PerfSentinel-Signature: sha256=<hex>`, an HMAC-SHA256 of
  `timestamp + "." + body` with the webhook's secret. Receivers reject an old timestamp.
- Body: event, disclosure id, organisation, period, `content_hash`, intent, declared integrity
  level, and a share link created for this delivery when `IncludeShareLink` is set (its own
  `ShareLinkDays`).
- Retries with backoff from the `disclosure_deliveries` row, so a restart resumes them. The
  screen shows each webhook's name, state and attempts, never its URL: Slack and Teams URLs
  carry their secret.

### Metrics and docs

- `perf_sentinel_hub_disclosure_import_rejected_total{reason}` on the closed set of import
  reasons plus `render_refused` and `internal_refused`, and
  `perf_sentinel_hub_disclosure_deliveries_failed_total`. Label values stay a closed set.
- Docs: `API.md`, `CONFIGURATION.md`, `ACCESS-CONTROL.md`, `OPERATIONS.md` (metrics,
  backup size), `LAUNCHER.md`, the `SECURITY-OVERVIEW` route inventory (two new anonymous
  routes), FR mirrors. Screenshots regenerated for the new screen only.

## Lab

`verify-hash` only checks keyless Sigstore signatures, and the lab has no Sigstore identity,
so the lab proves everything but the signature: its reports stay hash-only (as every lab
disclosure scenario already does) and the expected verdict is PARTIAL. TRUSTED is proven
outside the lab by the engine's `disclosure-signing.yml` workflow, extended to render the PDF
and run `verify-hash --report report.pdf` on it with the job's own identity. `verify-hash
--url` accepts https only, so the lab Hub is reached through TLS with a lab CA passed in
`SSL_CERT_FILE`.

A `hub-disclosures` scenario on the local images:
1. `disclose` on a seeded archive with `--emit-attestation`, POST to the Hub: 200, PDF
   rendered, hash and binding checked.
2. Share link created, `verify-hash --url` on it: PARTIAL, content hash and binding Ok,
   signature not provided. `verify-hash --report` on the downloaded PDF: the same.
3. The same report with one figure edited and the hash re-baked: 422, nothing stored.
4. An internal report with `AllowInternal` off: 422.
5. A webhook receiver in the lab checks the HMAC of "published", then of "withdrawn" after a
   withdrawal, and the link answers 404 after it.

## Delivery

- Engine first: one branch and PR for `disclose-pdf` and `verify-hash` on a PDF, released as
  0.31.0.
- Hub after its 0.6.0: one branch and PR for publication, reading, the screen, share links and
  webhooks, repinned to engine 0.31.0, released as 0.7.0 with its lab ledger line.

## Left to the builder

- The exact page layout of the PDF within the five pages above, and the screen's visual
  details, starting from the 14 October mock-ups and run through impeccable.
- The webhook backoff curve and its attempt cap.
- How the render-again action reports progress.
- Names of internal types, tables' column types and indexes.

## Calls I made (not discussed)

- The share link serves files only (you left this one to me).
- "Internal" is the report's `confidentiality_level`, not the G1 granularity.
- 8 MiB per publication request.
- The document code is the full `content_hash`, grouped by four.
- `AllowInternal` refused on an open Hub.
- A webhook's `Confidentiality` field exists but only `public` is accepted.
- `disclose-pdf` exit codes 0, 1, 3 and 5 with a closed set of reasons, so the Hub never
  relays engine stderr.
- Text outside the fonts refuses the render (stored without a PDF), no fallback font, no
  substitution.
- A signed copy replaces an unsigned one of the same report, any other change is a 409.
- `verify-hash` reads any well-formed PDF through a parsing dependency behind `pdf`, with a
  fuzz target.
- The lab proves PARTIAL, TRUSTED is proven by the signing workflow in CI.
- One Hub PR rather than two.
