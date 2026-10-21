# Periodic public reporting

`perf-sentinel disclose` produces a single JSON document that aggregates findings collected over a calendar period (typically a quarter) into a form suitable for public transparency. The output is hash-verifiable, schema-versioned, and distinct from the per-batch `Report` JSON consumed by the HTML dashboard.

The subcommand is added in v0.6.x and supersedes earlier ad-hoc disclosure recipes.

## When to use which intent

| intent      | validation       | publishable | typical use                     |
|-------------|------------------|-------------|---------------------------------|
| `internal`  | none             | no          | development drafts, dry runs    |
| `official`  | strict           | yes         | quarterly transparency post     |
| `audited`   | reserved         | not yet     | reserved for a future release   |

`audited` is reserved for a future release. The JSON schema accepts the value for forward compatibility, but the CLI exits with code 2 ("audited intent is reserved for a future release, use 'internal' or 'official' instead") and the daemon refuses to start with `intent = "audited"` configured.

For `official` intent, the validator also rejects reports below 75% runtime-calibration coverage. The denominator is `runtime_windows_count + fallback_windows_count`: each scoring window the daemon archived in the requested period is classified as runtime (per-service energy attribution present) or fallback (proxy I/O share used as a substitute). A coverage below 75% means more than a quarter of the period's windows did not carry per-service attribution, so the proxy share starts dominating the totals and the "official" claim loses meaningful per-service coverage. The empirical rationale for the exact 75% bar (versus 50% or 90%) is documented in [docs/design/08-PERIODIC-DISCLOSURE.md](design/08-PERIODIC-DISCLOSURE.md#the-75-runtime-calibration-threshold).

### Temporal coverage and other warnings (v1.2)

`disclose` reports `aggregate.temporal_coverage`: the fraction of the declared period's calendar days that carried measurements (`observed_days / days_in_period`). When it falls below an informational threshold the CLI prints a stderr warning and appends a disclaimer to the report, but it never blocks an `official` report. Archiving is traffic-gated (a window with no traffic writes nothing), so a low figure can be a legitimately quiet period rather than a measurement gap. Treat it as a lower bound on activity, not a daemon-uptime guarantee. It exists so a reader can distinguish a continuously-measured quarter from one where the daemon ran only a handful of days, the in-binary signal closest to "the operator stopped measuring for part of the period". Total non-participation (never running the tool) leaves no report at all and is out of scope for any in-binary check, see [docs/design/08-PERIODIC-DISCLOSURE.md](design/08-PERIODIC-DISCLOSURE.md).

Two further checks tighten consistency. The validator hard-rejects a `days_covered` that does not equal `(to_date - from_date) + 1` (a disclose-produced report always satisfies this, so only a hand-edited file trips it) and a `requests_measured` that exceeds an operator-declared `total_requests_in_period`. For `official` intent, omitting `total_requests_in_period` from both the org config and `--total-requests-in-period` emits a warning, since `coverage_percentage` is then absent from the report.

### The archive must reach the period start (0.28.0)

`disclose` also checks that the archive reaches back to the period's first day: the oldest window across every input file, in or out of the period, must fall on that day or earlier. When the inputs hold chained lines, only lines whose hash chain verifies count, so a line without a hash put in front of a chained archive cannot stand in for pruned windows. An archive written before chaining has no verified line, and its oldest line still counts. The daemon archive rotates at `max_size_mb` and prunes to `max_files` by count, not by age, and a busy daemon writes one window every `trace_ttl_ms / 2` (15 seconds by default). Its default 12 files of 100 MiB can therefore hold less than a day: a lab measurement under synthetic load found window lines of 350 to 900 KiB, which filled them in 6 to 16 hours, and the pruned files leave no trace in the hash chain, which restarts with each file. When the archive starts later, an `official` disclosure exits 2 without writing a report, and an `internal` one warns. Both messages name the archive's first day, so `--period-type custom --from <that day>` discloses what the archive still covers. A quiet start does not trip the check as long as older lines precede the period. Since 0.28.0, the daily rollups described below hold a quarter at the default settings. Without them, size the archive for the period you disclose, or disclose shorter periods.

### Daily rollups hold a period (0.28.0)

With `[daemon.archive]` configured, the daemon also writes daily rollups: one NDJSON line per UTC day, in one file per month named `<stem>-daily-YYYY-MM.ndjson` in the directory of `[daemon.archive] path`. With `path = "/var/lib/perf-sentinel/reports.ndjson"`, October 2026 lands in `/var/lib/perf-sentinel/reports-daily-2026-10.ndjson`. A rollup line holds the aggregator's own partial state for that day's windows, the same fold `disclose` runs over window lines, so a day costs a few tens of KiB and a quarter a few MiB at most. Its fields are `format` (`"perf-sentinel-rollup/v1"`), `ts` (the day's first window), `day`, `last_ts`, `state`, `prev`, `seq`, `drops` and `hash`. `hash` is the same SHA-256 archive chain hash as on window lines, computed over the line without its `hash` field.

A day closes when the first window of another UTC day arrives, judged by that window's own timestamp and not by the wall clock, and on a graceful shutdown. A daemon restarted the same day therefore writes a second line for that day, and a clock stepped back across midnight writes a second line for the earlier day. The lines of one day add up, nothing is overwritten. Every 20 windows (about five minutes at the default window), the open day is also saved to `<stem>-daily.partial.json`, written to a temporary file then renamed, readable by its owner only. At startup the daemon appends that snapshot once as a line, unless a line on disk already starts at the same first window, then removes it. A snapshot behind a symlink is refused. A crash (SIGKILL, OOM kill) therefore costs the rollups at most their last 20 windows, never the day, and never counts a day twice. When the window lines are kept, `disclose` still counts those windows from them, see below. A line the directory refuses, on a full disk for instance, is kept in memory and retried at the next day close. At shutdown the open day goes to the snapshot instead.

Two `[daemon.archive]` keys come with them:

- `rollup_retention_days` (default `400`, a little over a year, must be `>= 1`). A monthly file is deleted whole once its last day is more than this many days before the newest closed day, or before today when a window stamped in the future closed a later day. The check runs each time a day closes.
- `windows` (default `true`). `false` stops the per-window lines and keeps only the rollups. The window lines are the per-window detail the [RUNBOOK](RUNBOOK.md) queries, so turn them off only if you do not need it.

`--input` accepts window files, rollup files or both, and a directory holding both. A rollup file is recognised by its content, the format tag on its first readable line, so a renamed or copied rollup file is still read as one. A directory never yields the `.json` snapshot. Rollup files are read first. A rollup line covers the windows between its first and last window (`ts` and `last_ts`) for the archive it sits beside, the window file with the same stem in the same directory. Window lines inside that span then feed only the chain check and the drop counter, so no window counts twice. Window lines outside every span still count: those archived before the upgrade on the upgrade day, and those a crash cost the rollup after its last snapshot. Hosts stay apart, so one host's rollups never hide another host's windows. A rollup file renamed away from its archive's stem is read as a source of its own, so do not pass it together with that archive's window lines.

Figures from rollups match figures from the same windows up to floating-point rounding, because a rollup sums its day first and the additions run in another order. The same period disclosed from rollups and from windows can therefore differ in its last digits, and so in its `content_hash`. Disclosing the same inputs twice stays reproducible. The published report schema does not change: rollups are an input format only.

The daemon writes rollups without applying `--strict-attribution`, which stays the reader's call. With the flag, `disclose` refuses a rollup that folded an unattributed window and names the earliest such window. A rollup line is read from a file, so merging it applies every guard the window path applies: the 4096-service cap, known anti-pattern names only, non-finite or negative figures zeroed, binary version and energy model tag validation. A rollup line also answers only for its own day.

All rollup lines of an archive form one chain that never resets. It runs across the monthly files and across restarts, the writer resuming from the last line of the newest file. A removed or edited day shows as a chain break. Retention deletes whole months from the front, and the reader adopts the oldest surviving line as the chain's start, so retention is not a break. In `integrity.trace_integrity_chain`, a verified rollup line counts the windows it holds as verified, and a broken rollup line counts one break. Each line also carries the daemon's cumulative drop counter, so `windows_dropped` is published from rollups as from window lines.

The reach check above takes the oldest verified `ts` across rollup and window lines. An official quarterly disclosure therefore passes it with the default settings once the daemon has been archiving since the quarter's first day, even after the window files of that day were pruned. To see how far back the rollups reach, read `disclosure_archive` in `/api/status` (`oldest_day`, `days`, `last_closed_day`, absent until a first day has closed), or the gauges `perf_sentinel_archive_oldest_day_timestamp_seconds`, `perf_sentinel_archive_rollup_days` and `perf_sentinel_archive_last_closed_day_timestamp_seconds`, which read `0` until then.

## Granularity

Perf Sentinel publishes reports at two granularity levels, controlled by `--confidentiality`. The validator refuses to publish a `confidentiality = public` disclosure that carries G1 entries, and vice versa.

- **G1** (Granularity level 1, "internal detail"). Activated by `--confidentiality internal`. Each `applications[*]` entry carries a full `anti_patterns: [...]` array breaking down every anti-pattern type detected on that service with occurrences, estimated waste energy, and waste carbon. Use for internal optimization decisions, not for public publication: the per-pattern detail exposes internal performance signals an operator may not want broadcast.
- **G2** (Granularity level 2, "public aggregate"). Activated by `--confidentiality public`. Each `applications[*]` entry carries the same service-level totals (energy, carbon, efficiency score) but replaces the array with a single `anti_patterns_detected_count` integer. Suitable for publication on an organization's transparency URL.

## CLI flags

`perf-sentinel disclose` accepts the following flags:

- `--intent <internal|official|audited>` (required). `audited` is reserved for a future release, the CLI refuses it today with exit code 2.
- `--confidentiality <internal|public>` (required). Drives G1 vs G2 granularity, see above.
- `--period-type <calendar-quarter|calendar-month|calendar-year|custom>` (required). Hints the period semantics for downstream consumers. `custom` uses `--from` and `--to` as-is and is the right choice for non-aligned windows (e.g. a 6-week pilot).
- `--from <YYYY-MM-DD>` and `--to <YYYY-MM-DD>` (required, inclusive). UTC calendar dates.
- `--input <PATH>` (required, repeatable). Each path can be a single `.ndjson` file, a directory whose `*.ndjson` files are unioned (sorted by name), or a shell-expanded glob. Perf Sentinel itself does not expand globs, so `--input archive/2026Q1/*.ndjson` works in a shell but fails when called via direct `exec` without shell expansion. In CI runners that exec the binary directly, prefer a directory or a single file.
- `--output <PATH>` (required). Where to write `perf-sentinel-report.json`.
- `--org-config <PATH>` (required for `intent = "official"`). The static organisation / methodology / scope TOML described in the org-config TOML section below.
- `--emit-attestation <PATH>` (optional). When set, also writes the in-toto v1 statement sidecar at this path. Needed for the signing workflow.
- `--strict-attribution` (optional). By default, Perf Sentinel buckets spans without a `service.name` attribution into a synthetic `_unattributed` service. This bucket contributes to aggregate totals but is excluded from per-service breakdowns. With `--strict-attribution`, the disclose call refuses to produce a report if any window carries unattributed spans, listing the offending timestamps in the error message. Use for an official disclosure when you want to assert that 100% of measured operations were properly attributed.
- `--total-requests-in-period <N>` (optional). Total requests across the in-scope applications over the period, the denominator of `coverage_percentage`. Takes precedence over `scope_manifest.total_requests_in_period` in the org config. The total is only known once the period closes, so passing it per run keeps the org config static. The `--tui` preview applies it too and repeats it in the command it prints.
- `--tui` (optional, needs the `tui` build feature). Opens a read-only preview instead of writing a report, see below. It relaxes `--intent`, `--confidentiality`, `--period-type`, `--from`, `--to`, and `--output` to optional, since you dial them in the interface. Conflicts with `--emit-attestation`.

## Interactive preview (`--tui`)

`perf-sentinel disclose --tui` opens a read-only preview instead of producing a report. It re-reads the same cold NDJSON archive as the canonical command, so the figures match exactly, but it never hashes or writes anything. Use it to dial in a period and confirm coverage before committing to the reproducible CLI or CI run.

```bash
perf-sentinel disclose --tui \
  --input /var/lib/perf-sentinel/reports.ndjson \
  --org-config /etc/perf-sentinel/org.toml
```

![disclose preview: calendar stepper widens the period, the summary updates, and the official validator reports its verdict](https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/perf-sentinel/main/docs/img/disclose/demo.gif)

Only `--input` and `--org-config` stay required. The period, intent, and confidentiality are set live:

- `g` cycles the granularity (month, quarter, year, custom). For the first three, `from` and `to` snap to calendar boundaries. `custom` lets you edit each edge by hand.
- `←` / `→` (or `h` / `l`) step the period one unit at a time. In `custom` they move the active edge by a day, `[` and `]` move it by a month, and `Tab` switches between the `from` and `to` edge.
- `i` toggles the intent (internal or official), `c` toggles the confidentiality (G1 internal or G2 public).
- The summary reports the window count, period coverage against the official threshold, measured and excluded services, totals (requests, carbon, energy, waste ratio), and the official-validator verdict when the intent is official.
- Under Totals, a `Database:` and a `Broker:` line give each host perimeter: the in-total host kWh and how many of the windows with a figure counted it (`1.2500 kWh in the totals (3 of 5 windows)`), or `outside the totals`. See [Host perimeter](#host-perimeter-v18).
- The footer prints the exact `disclose` command for the current settings. Copy it to produce the hashed report.

The preview needs an interactive terminal. Piping its output (no TTY) exits with a clear error rather than rendering.

## Avoidable waste: canonical versus operational (1.1+)

A report carries the total operational energy and carbon of the workload (span-derived, not tunable) plus two tiers of avoidable waste:

- **Canonical** (`aggregate.canonical_waste`). Computed at a fixed N+1 threshold pinned in the binary (`2`), regardless of the operator's `[detection] n_plus_one_min_occurrences`. This is the non-manipulable figure: raising your own threshold cannot shrink it. It is the headline avoidable number, and at the default operator threshold of `5` it is typically larger than what the operator's own dashboard shows.
- **Operational** (`aggregate.operational_waste`). Computed at the operator's configured threshold and recorded with that threshold.

Publishing both lets a reader compare the two and see how much avoidable waste a loose operator threshold would otherwise hide. For `intent = official`, the validator rejects a report whose `canonical_waste.n_plus_one_threshold` is not the binary's canonical value.

![disclose preview, month view: settings header, aggregated summary, and the equivalent command in the footer](https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/perf-sentinel/main/docs/img/disclose/preview.png)

![disclose preview, quarter view: the `g` stepper widens the period to the whole quarter](https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/perf-sentinel/main/docs/img/disclose/quarter.png)

![disclose preview, official intent: the validator reports why the report is not yet publishable](https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/perf-sentinel/main/docs/img/disclose/official.png)

## Host perimeter (v1.8)

`total_energy_kwh` and `total_carbon_kgco2eq` cover the application services, plus the database and broker hosts in the windows that counted them. A window counts a host when a measured (`alumet_rapl`), modeled (`cloud_specpower`) or declared (`database_specpower`, `broker_specpower`) figure carried a carbon conversion. The estimated figure is the application-side share of the SQL or publish spans and stays inside the application figures. A host figure archived before host energy entered the totals, or without carbon, stays beside them under `carbon_breakdown.*_kgco2eq_out_of_total`.

Every report states this perimeter three times:

- `notes.disclaimers` ends with one line per subsystem, database then broker, appended after the custom or default disclaimers. It says the hosts are inside the totals in all N windows that carried a figure or in K of N windows (naming the estimated windows and the ones kept beside the totals), with the models and placements seen over the period, or that the totals hold none of that subsystem's host energy. When a host is inside, the line adds that its carbon is in `total_carbon_kgco2eq` and in no application's figures.
- The PDF at-a-glance page names the database and broker hosts whose `in_totals_windows` is non-zero, the test `notes.disclaimers` uses, with their `in_totals_energy_kwh` (and `in K of N windows` when only some windows counted them), and says the others are outside the totals. The applications table adds a `Database hosts` and a `Broker hosts` row for the same hosts, so its rows add up to the Total row.
- `carbon_breakdown.database_kgco2eq` and `messaging_kgco2eq` carry the in-total host carbon, and `database_waste.in_totals_energy_kwh` and `messaging_waste.in_totals_energy_kwh` the in-total host energy.

For `intent = official`, the validator checks the perimeter: the two host carbon terms are finite and >= 0, the five breakdown terms add up to `total_carbon_kgco2eq` within 1e-6 relative (skipped when `carbon_methodologies` holds the legacy `sci_v1_numerator` tag), and the applications' `energy_kwh` plus both in-total host shares fit in `total_energy_kwh`.

## ESRS E1 crosswalk (v1.3)

Every report carries `methodology.standard_crosswalk`, an interpretive map from its figures to the EU climate-reporting standard ESRS E1 (Delegated Regulation (EU) 2023/5303):

- `aggregate.total_energy_kwh` feeds **E1-5** (energy consumption and mix), to be converted to MWh. Perf Sentinel does not split the figure by fossil, nuclear or renewable source. It includes database and broker host energy when the matching `in_totals_windows` is non-zero.
- the operational, database and broker carbon terms feed **E1-6 Scope 2** on a location-based basis. ESRS also requires a market-based Scope 2 figure, which SCI excludes, so this is a partial input.
- embodied carbon (the SCI `M` term, aggregate only) feeds **E1-6 Scope 3** (categories 1 and 2). ESRS admits estimates and proxy data for Scope 3.

This is a **mapping aid, not a certification**. It does not make a report a CSRD submission: the figures keep their 2x directional uncertainty bracket, the scope is IT compute only, and an audited inventory by a qualified body is still required. The same caveats ship in-band under `standard_crosswalk.caveats` and in `notes.disclaimers`.

## Inputs

**Daemon archives only.** A batch `analyze --format json` report cannot feed `disclose`, for three independent reasons: a directory input collects `*.ndjson` only, every line must deserialize as the `{"ts", "report"}` envelope below or as a daily rollup line, and a batch report carries no `disclosure_waste` (the canonical avoidable tiers are computed at archive time). Batch reports stay excluded because a public disclosure rests on continuous observation over the declared period, not on ad-hoc runs. Run `perf-sentinel watch` with archiving enabled for anything you intend to disclose.

A period that mixes archives written before canonical disclosure with newer ones passes `--intent official` validation, because the canonical threshold is the maximum across windows. The canonical tier then omits waste from the older windows, so `disclose` prints how many windows carried no canonical figure. Regenerate over post-upgrade windows for a complete figure.

The aggregator reads NDJSON files that the daemon archives one envelope per scoring window:

```json
{"ts":"2026-01-15T14:30:00Z","report":{ ...full Report... }}
```

Configure the daemon archive via:

```toml
[daemon.archive]
path = "/var/lib/perf-sentinel/reports.ndjson"
max_size_mb = 100
max_files = 12
windows = true
rollup_retention_days = 400
```

When the active file exceeds `max_size_mb`, Perf Sentinel renames it to `reports-<utc-timestamp>.ndjson` and starts a fresh file. Older rotated files beyond `max_files` are pruned by modification time. Window rotation never touches the daily rollup files, which `rollup_retention_days` prunes by age, see [Daily rollups hold a period](#daily-rollups-hold-a-period-0280).

Operators that already collect daemon stdout via a sidecar can pass the resulting file (or directory) to `--input` directly, as long as each line is one `{ts, report}` envelope.

## org-config TOML

The static organisation/methodology/scope fields live in a TOML file you check into your infrastructure repository alongside the rest of the Perf Sentinel config. A complete example sits in `docs/examples/perf-sentinel-org.toml`. The same file is referenced by `[reporting] org_config_path` when the daemon is asked to validate publishable disclosures at startup.

`methodology.calibration.specpower_table_version` is optional. Left out, `disclose` fills it with the date of the SPECpower table the running binary embeds, the only table it can use, so an upgrade that changes the table needs no edit. Declared, it must equal that date: `disclose --intent official` rejects a mismatch, and so does the daemon startup gate below, so the drift surfaces at deploy time rather than when the report is produced.

## Example: internal draft (G1)

```bash
perf-sentinel disclose \
  --intent internal \
  --confidentiality internal \
  --period-type calendar-quarter \
  --from 2026-01-01 --to 2026-03-31 \
  --input /var/lib/perf-sentinel/reports.ndjson \
  --output /tmp/perf-sentinel-report.json \
  --org-config /etc/perf-sentinel/org.toml
```

The output passes only the structural checks (no validator). `integrity.content_hash` is computed and stable but `integrity.binary_hash` is the SHA-256 of the locally running binary, not necessarily a published release.

## Example: official publication (G2)

```bash
perf-sentinel disclose \
  --intent official \
  --confidentiality public \
  --period-type calendar-quarter \
  --from 2026-01-01 --to 2026-03-31 \
  --input /var/lib/perf-sentinel/reports.ndjson \
  --output /var/www/transparency/perf-sentinel-report.json \
  --org-config /etc/perf-sentinel/org.toml
```

The validator runs over the full document. If any required field is missing or out of range, the CLI prints every offending field and exits 2. Fix the org-config (or the underlying data) and re-run.

The recommended publication path is the root of your transparency domain:

```
https://transparency.example.fr/perf-sentinel-report.json
```

The schema URL inside `notes.reference_urls.schema` advertises which schema version a consumer should fetch to validate the file.

## Daemon-driven gating

When the daemon is configured with `[reporting] intent = "official"`, it refuses to start if the org-config TOML is missing or fails the static-field validator. The error message lists every missing or invalid field so an operator fixes them all in one pass.

```toml
[reporting]
intent = "official"
confidentiality_level = "public"
org_config_path = "/etc/perf-sentinel/org.toml"
# Reserved for daemon-triggered periodic disclosures, not implemented
# yet: a no-op. Setting it today logs a warning at startup. Reports are
# produced exclusively via `perf-sentinel disclose --output`.
disclose_output_path = "/var/lib/perf-sentinel/last-disclosure.json"
disclose_period = "calendar-quarter"
```

`intent = "internal"` (or omitting the section) leaves the daemon in monitoring mode without the publishable-disclosure gate.

## Restricting who can publish an official disclosure

Producing an official disclosure is an outward-facing, hard-to-reverse
action: the file lands at a public transparency URL and is signed under
your organisation's identity. The CLI itself has no authorization layer.
Anyone who can run the binary with the org-config and the input data can
produce one. So the control belongs in the pipeline that publishes, not
in `perf-sentinel` itself, the same split as for the daemon write paths
(see [`docs/QUERY-API.md`](./QUERY-API.md#restricting-writes-in-production-reverse-proxy)).

In CI, gate the job that runs `disclose --intent official` behind a
**GitHub Environment with required reviewers**. A developer can still
open the PR or trigger the workflow, but the job pauses until a named
reviewer (an architect or a DevOps lead) approves.

Set it up once under `Settings -> Environments -> official-disclosure`:

- **Required reviewers**: the architects or DevOps team that signs off
  on official reports.
- **Deployment branches and tags**: restrict to your protected branch or
  release tags so the job cannot run from an arbitrary branch.

Then target that environment from the workflow:

```yaml
# .github/workflows/perf-sentinel-disclosure.yml
name: official disclosure

on:
  workflow_dispatch:
    inputs:
      from: { description: "period start (YYYY-MM-DD)", required: true }
      to:   { description: "period end (YYYY-MM-DD)",   required: true }

permissions:
  contents: read
  id-token: write   # Sigstore keyless signing via the workflow OIDC token

jobs:
  publish:
    runs-on: ubuntu-latest
    environment: official-disclosure   # required reviewers gate this job
    steps:
      - uses: actions/checkout@b4ffde65f46336ab88eb53be808477a3936bae11 # v4.1.1
      - name: Produce and sign the official disclosure
        run: |
          perf-sentinel disclose \
            --intent official \
            --confidentiality public \
            --period-type calendar-quarter \
            --from "${{ inputs.from }}" --to "${{ inputs.to }}" \
            --input /var/lib/perf-sentinel/reports.ndjson \
            --output perf-sentinel-report.json \
            --org-config /etc/perf-sentinel/org.toml \
            --emit-attestation attestation.intoto.jsonl
          # cosign sign-blob ... then publish, see "Signing your disclosure" below.
```

The `id-token: write` permission is a control in its own right: keyless
signing binds the disclosure to the workflow identity, recorded in
`integrity.signature.signer_identity`. A run from an unapproved path
produces a file whose signer identity does not match the workflow a
verifier expects, see [Identity verification](#identity-verification).

## Signing your disclosure

### Background: Sigstore primer

If you have not used Sigstore before, this short primer is a prerequisite for the commands that follow.

**Why Sigstore.** Sigstore is an open-source toolkit hosted by the Open Source Security Foundation (OpenSSF) and maintained by Google, Red Hat, Chainguard, GitHub and the Linux Foundation. It is the de-facto standard for verifiable artefact signatures in the cloud-native ecosystem (Kubernetes, Helm, npm provenance and PyPI attestations all rely on it). The choice for Perf Sentinel disclosures comes from three properties:

1. **Keyless signing**, no long-lived private key to manage or leak on the signer side.
2. **A public, tamper-evident log** (Rekor), so a third party can independently verify that a signature existed at a given point in time.
3. **Free, open-source, self-hostable**, no proprietary lock-in or per-signature billing.

**The three components.**

- **Cosign** is the client CLI you run locally. It opens an OIDC flow in your browser (or consumes a workflow token in CI), signs the file, and ships the signature to Sigstore.
- **Fulcio** is the certificate authority. It takes the OIDC token cosign obtained (proof of identity: email, GitHub workflow URL, ...) and issues a short-lived X.509 certificate (10 minutes) bound to that identity. Fulcio never sees the signer's private key.
- **Rekor** is the public transparency log. It records the signature next to the Fulcio certificate, returns an inclusion proof, and exposes the entry at a stable log index. Past entries cannot be silently rewritten.

**Who signs with which key.** Cosign generates a brand-new ephemeral keypair just before signing. Fulcio issues a 10-minute certificate that binds the *public* half of that keypair to the OIDC identity. Once the signature is uploaded to Rekor the keypair is discarded. Only the signature, the certificate, and the Rekor entry remain, which is what a verifier needs.

**The OIDC identity** is the subject of the Fulcio certificate (and ends up as `integrity.signature.signer_identity` + `signer_issuer` in your disclosure). For an individual signing with a Google account the identity is the email address and the issuer is `https://accounts.google.com`. For a GitHub Actions workflow the identity is the workflow URL and the issuer is `https://token.actions.githubusercontent.com`.

**Known limitation: OIDC issuer migration.** The issuer URL is recorded inside the certificate, therefore in Rekor and in the disclosure. If the organisation later migrates between identity providers (Google Workspace to self-hosted Keycloak, Entra ID to Okta, ...), past signatures remain valid but new signatures will carry a different `signer_issuer`. Verifiers that pin a specific issuer in their verification policy must be updated, otherwise they reject the new signatures as untrusted. Plan the pinning policy with provider migrations in mind.

**Related terms you will see in the workflow.** One-liners only, full definitions in the linked specs.

- **OIDC (OpenID Connect)** is an identity protocol layered on OAuth 2.0. In this workflow it is how cosign proves "this signer is `user@example.org`" to Fulcio. Cosign opens a browser tab, you log in to your IdP (Google, GitHub, ...), the IdP returns a signed token, cosign forwards it to Fulcio. [Spec](https://openid.net/specs/openid-connect-core-1_0.html).
- **in-toto v1 statement** is an open OpenSSF specification for software-supply-chain attestations. A JSON envelope that pairs an artefact hash with a typed *claim* about it. `--emit-attestation` produces such a statement where the artefact is your `report.json` and the claim type is `perf-sentinel-disclosure/v1`. Cosign signs the statement, not the report directly, so verifiers can chain the trust from the report hash through the in-toto statement, the cosign signature on that statement, and finally the Fulcio cert binding the signature to an OIDC identity. [Spec](https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md).
- **Bundle (`bundle.sig`)** is the file cosign writes at sign time. It packs the signature, the Fulcio certificate, and the Rekor inclusion proof into a single JSON. Publishing this bundle next to the report enables fully offline verification: a consumer validates the signature against Rekor's public key without having to re-query Rekor live.
- **SLSA provenance** is a separate OpenSSF framework (Supply-chain Levels for Software Artifacts) that describes *how* an artefact was built (which source commit, which builder, which workflow). perf-sentinel release binaries carry SLSA build provenance (Build Level 2) produced during the GitHub Actions release workflow. The disclosure's `integrity.binary_attestation` is filled with this provenance when present, and `report_metadata.integrity_level` graduates from `signed` to `signed-with-attestation`. [Spec](https://slsa.dev/spec/v1.0/).

The same primer is mirrored in [docs/SUPPLY-CHAIN.md](SUPPLY-CHAIN.md#background-sigstore-primer), which is the canonical location for the supply-chain stack across binary, Helm chart and disclosure signing.

### Workflow

`intent = "official"` disclosures should be signed via Sigstore so a
consumer can verify the file was published by your organisation and
has not been modified. The pipeline is opt-in: pass
`--emit-attestation <path>` to `disclose` to get a sidecar in-toto
v1 statement, then sign that statement with `cosign`.

```bash
# 1. Produce the report and the in-toto attestation.
perf-sentinel disclose \
    --intent official \
    --confidentiality public \
    --period-type calendar-quarter \
    --from 2026-01-01 --to 2026-03-31 \
    --input archive/2026Q1/*.ndjson \
    --output report.json \
    --emit-attestation attestation.intoto.jsonl \
    --org-config org.toml

# 2. Sign the attestation with cosign against Sigstore public. The
#    file produced at step 1 is already a complete in-toto v1
#    Statement, so we sign it directly with `cosign sign-blob`. The
#    OIDC issuer (browser flow or GitHub Actions token) records the
#    signer identity. The bundle includes the Rekor inclusion proof.
#    Do NOT use `cosign attest-blob --predicate attestation.intoto.jsonl`:
#    that command treats its input as a raw predicate and wraps it in
#    a fresh Statement, producing a double-wrapped permanent entry in
#    the Rekor public log.
cosign sign-blob \
    --bundle bundle.sig \
    --new-bundle-format \
    attestation.intoto.jsonl

# 3. Patch integrity.signature into report.json so verifiers can
#    locate the bundle and the Rekor entry (see "Editing
#    integrity.signature" below for the schema and a jq helper).
#    Then bump report_metadata.integrity_level from "hash-only" to
#    "signed" (or "signed-with-attestation" if the producing binary
#    carries SLSA provenance). A future `perf-sentinel sign`
#    subcommand will automate this step. The attestation names the
#    report's canonical content hash, which leaves both fields out, so
#    this edit does not break the tie between signature and figures.

# 4. Publish report.json, attestation.intoto.jsonl, bundle.sig at
#    your transparency URL.
```

### Editing integrity.signature

After step 2 succeeds, `report.json` still has `integrity.signature =
null`. A consumer running `verify-hash` would see "Signature: not
provided" and treat the report as PARTIAL. Step 3 fills the locator
fields so the consumer can find the bundle and verify it.

The seven fields and where each value comes from:

| Field             | Where to read                                                                                                                                                                                                    |
|-------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `format`          | constant `"sigstore-cosign-intoto-v1"` for this schema                                                                                                                                                           |
| `bundle_url`      | URL where you will publish `bundle.sig` at step 4                                                                                                                                                                |
| `signer_identity` | the OIDC subject in the signing certificate inside `bundle.sig` (`cosign sign-blob` 3.0+ does not always print it on stdout). Visible via `cosign verify-blob --certificate-identity-regexp '.*' ... 2>&1 \| grep identity` |
| `signer_issuer`   | same source as `signer_identity`, the OIDC issuer URL recorded next to it                                                                                                                                        |
| `rekor_url`       | the Rekor instance used (`https://rekor.sigstore.dev` for Sigstore public, or the value from `[reporting.sigstore] rekor_url` for a private instance)                                                            |
| `rekor_log_index` | `bundle.sig` at `.verificationMaterial.tlogEntries[0].logIndex` (`cosign sign-blob` 3.0+ no longer prints `tlog entry created with index: X`). Or fetch via `curl <rekor_url>/api/v1/log/entries?logIndex=X` to confirm |
| `signed_at`       | timestamp from the Rekor entry, ISO 8601 UTC                                                                                                                                                                     |

Example before / after on a fresh disclosure:

```json
// Before step 2 (state immediately after disclose --emit-attestation)
"integrity": {
  "content_hash": "sha256:abc123...",
  "binary_hash": "sha256:def456...",
  "binary_verification_url": "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/releases/tag/v0.7.0",
  "trace_integrity_chain": null,
  "signature": null,
  "binary_attestation": null
}
```

```json
// After step 3 (after cosign sign-blob succeeds and the operator pastes locators)
"integrity": {
  "content_hash": "sha256:abc123...",
  "binary_hash": "sha256:def456...",
  "binary_verification_url": "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/releases/tag/v0.7.0",
  "trace_integrity_chain": null,
  "signature": {
    "format": "sigstore-cosign-intoto-v1",
    "bundle_url": "https://transparency.example.fr/bundle.sig",
    "signer_identity": "robin.trassard@example.fr",
    "signer_issuer": "https://accounts.google.com",
    "rekor_url": "https://rekor.sigstore.dev",
    "rekor_log_index": 123456789,
    "signed_at": "2026-05-15T09:00:00Z"
  },
  "binary_attestation": null
}
```

And in `report_metadata`:

```diff
-  "integrity_level": "hash-only"
+  "integrity_level": "signed"
```

(Use `"signed-with-attestation"` instead of `"signed"` when the
producing binary also carries SLSA provenance.)

### What the hash proves, and where it stops

The `content_hash`, the cosign signature and the SLSA provenance bind
the **published document**. Since 0.9.25 the daemon also hash-chains
each archived window as it writes it. `disclose` walks that chain while
aggregating and publishes the result in
`integrity.trace_integrity_chain`: windows verified, windows written
before chaining existed, breaks found inside the period, and breaks
found in the same files outside it. Breaks are split this way because
one rolling archive can span years and a report answers only for its
own period. The chain closes the gap where a window could be edited
between measurement and publication with nothing to show for it. A
break does not stop the report. It is published as a count, so a
truncated archive still yields a partial disclosure.

Since v1.7 each archive line also carries the daemon's cumulative drop
counter. The same block publishes `windows_dropped`, the windows the
daemon produced but could not archive over the period. It also publishes
`drop_counter_resets`, how many times the counter went backwards (one
per daemon restart, each making the figure a lower bound over the gap it
spans). This is the signed twin of the
`perf_sentinel_archive_windows_dropped_total` metric: the scrape sees
losses live, the report accounts for them after the fact. It says
nothing about windows never produced (a stopped daemon leaves no counter
to read), which `aggregate.temporal_coverage` bounds instead. Both fields
are omitted on archives written before the counter existed, so an old
archive never reads as zero drops.

One edit the chain does not see is a clean cut of an archive's tail:
what remains is a shorter chain that verifies on its own terms, and no
field inside the file can contradict it. Detecting that needs a record
of the chain head kept somewhere the operator does not control, which is
what `integrity.cross_period_log` stays reserved for.

None of this proves that the measurement was sincere in the first
place. An operator who controls the daemon and its files can
regenerate a consistent chain, exactly as they can choose an
unfavourable coefficient. Those parameters are now published too, see
`carbon_methodologies`, `scoring_coefficients` and the embodied fields
in `methodology.calibration_inputs`, which makes them contestable
rather than invisible. The remaining step is anchoring the chain head
outside the operator's control, which is what
`integrity.cross_period_log` stays reserved for. The guarantee is
tamper-evident after the fact but self-declared at the source.

### Content hash stays valid

The `content_hash` does **not** need to be recomputed after step 3.
The canonical form used by `compute_content_hash` blanks four
fields before hashing: `integrity.content_hash`,
`integrity.signature`, `integrity.binary_attestation`, and
`report_metadata.integrity_level`. The list lives in
`POST_SIGN_FIELDS` (`crates/sentinel-core/src/report/periodic/hasher.rs`)
and the invariance is enforced by the test
`hash_is_invariant_under_post_sign_locator_addition`. So a consumer
re-running the hash on the post-step-3 report gets the same value
as the operator did at step 1.

**Do not recompute** `content_hash` after editing. Doing so
produces a fresh hash, breaks the canonical form, and a verifier
will see a hash mismatch.

### jq helper

**Caveat.** The snippet below is indicative, not canonical. It diverges
from the schema in [Editing integrity.signature](#editing-integritysignature)
on four fields:

- `signer_identity`: parses `Successfully signed by ...` from cosign
  stdout, which is the wording emitted by `cosign sign` for container
  images but not always by `cosign sign-blob` (3.0+ omits it). The
  canonical value is the OIDC subject embedded in the signing
  certificate (read it via `cosign verify-blob --certificate-identity-regexp '.*'`,
  or inspect `bundle.sig` directly with
  `jq -r '.verificationMaterial.certificate.rawBytes' bundle.sig | base64 -d | openssl x509 -inform DER -noout -ext subjectAltName`).
- `signer_issuer`: hardcoded to `https://accounts.google.com`. The
  canonical value is the OIDC issuer URL in the signing certificate,
  which the operator must align with their provider (Google, GitHub
  Actions, custom OIDC).
- `rekor_log_index`: parses `tlog entry created with index` from cosign
  stdout, which `cosign sign-blob` 3.0+ no longer emits, so `LOG_INDEX`
  is empty and the snippet falls through to `rekor_log_index: 0`. The
  canonical value is in `bundle.sig` itself at
  `.verificationMaterial.tlogEntries[0].logIndex`, no API call needed.
- `signed_at`: filled with the local clock at jq-run time. The canonical
  value is the `integratedTime` of the Rekor entry, fetched via
  `https://rekor.sigstore.dev/api/v1/log/entries?logIndex=<idx>` and
  formatted as ISO 8601 UTC.

For a publication intended for third-party audit, prefer reading these
four fields from the bundle and from the Rekor entry rather than from
local state. The helper remains useful for an internal dry-run where
the four values are inspected for plausibility, not provenance.

The pattern is repetitive and easy to script. Until
`perf-sentinel sign` lands, this jq workflow
captures the fields from cosign output and patches the report in
one shot:

```bash
# Sign and capture cosign output for parsing
cosign sign-blob \
    --bundle bundle.sig \
    --new-bundle-format \
    attestation.intoto.jsonl 2>&1 | tee cosign.log

# Extract tlog index from cosign output. Format:
# "tlog entry created with index: 123456789"
LOG_INDEX=$(grep "tlog entry created with index" cosign.log \
            | awk '{print $NF}')

# Extract signer identity from the cosign log. Format depends on
# issuer: for Google OIDC it's an email, for GitHub Actions it is
# the workflow URL.
SIGNER=$(grep "Successfully signed" cosign.log \
         | sed 's/.*by //' | tr -d '"')

# Pick the issuer that matches your OIDC provider.
ISSUER="https://accounts.google.com"  # or token.actions.githubusercontent.com

# Patch report.json with the seven locator fields and bump
# integrity_level. Adjust bundle_url to your transparency host.
jq --arg url "https://transparency.example.fr/bundle.sig" \
   --arg sig "$SIGNER" \
   --arg issuer "$ISSUER" \
   --arg idx "$LOG_INDEX" \
   --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
   '.integrity.signature = {
     format: "sigstore-cosign-intoto-v1",
     bundle_url: $url,
     signer_identity: $sig,
     signer_issuer: $issuer,
     rekor_url: "https://rekor.sigstore.dev",
     rekor_log_index: ($idx | tonumber),
     signed_at: $ts
   } | .report_metadata.integrity_level = "signed"' \
   report.json > report-signed.json && mv report-signed.json report.json
```

This is an interim workaround. `perf-sentinel sign` will replace
the bash + jq combo with a single subcommand once it ships.

Operators who run a private Rekor instance set
`[reporting.sigstore] rekor_url = "..."` in their Perf Sentinel
config and pass the same URL to `cosign --rekor-url`.
`verify-hash` rejects bundles signed with
`cosign sign-blob --no-tlog-upload`, because such bundles lack a
Rekor inclusion proof. Always sign without that flag for reports
intended for public transparency.

`verify-hash` itself reads `integrity.signature.rekor_url` from
the report being verified, so a consumer fetching a publicly
hosted disclosure does not need any local configuration: the
URL travels with the report. If you want to force a different
Rekor at verification time (e.g. cross-check a public-Rekor
claim against a private archive), invoke cosign directly with
its own `--rekor-url` flag rather than going through
`verify-hash`. The report stays the single source of truth for
which transparency log signed it.

See `docs/design/10-SIGSTORE-ATTESTATION.md` for the full
methodology, failure modes, and privacy considerations on Rekor
public.

## Rendering a PDF for readers (`disclose-pdf`)

`perf-sentinel disclose-pdf` renders a report as a PDF/A-3b a reader without a terminal can follow: A4 pages in English, five unless a section runs onto the next page, with the figures, the uncertainty band, the applications, the method and its disclaimers, and how to verify the document. An annex follows that prints every field the current schema defines, in the order it defines them and as the attached report.json carries them, with `not reported` for each field the report does not carry or carries empty, so what a report leaves out shows instead of disappearing. The report, its attestation and its bundle travel inside the PDF as attachments, byte for byte.

```bash
perf-sentinel disclose-pdf \
    --report report.json \
    --attestation attestation.intoto.jsonl \
    --bundle bundle.sig \
    --expected-identity release@example.fr \
    --expected-issuer https://accounts.google.com \
    --output report.pdf
```

It renders only when the `content_hash` recomputes and, with `--attestation`, when the attestation names that hash, so a rendered PDF stands for those two checks. It never checks the signature: the verify page prints the `verify-hash` command, with the identity and issuer passed here, for the reader to run. The document code is the report's `content_hash`, printed in full and grouped by four characters on the cover and the verify page, and shortened in the footer of every page. Add `transparency` to `[notes.reference_urls]` in the org config and the cover draws a QR code to the page where you publish your reports. Without it, no QR code is drawn. Since the annex prints every field, a character the embedded fonts lack anywhere in the report makes it unrenderable, not only one on the first five pages.

The command prints one JSON line on stdout and exits with a fixed code, a contract PerfSentinelHub relies on:

| Exit | `result`       | `reason`                                                                    |
|------|----------------|-----------------------------------------------------------------------------|
| 0    | `rendered`     | none                                                                        |
| 1    | `refused`      | `content_hash_mismatch`, `binding_mismatch`                                 |
| 3    | `input_error`  | `missing_file`, `file_too_large`, `malformed_report`, `output_unwritable`   |
| 5    | `unrenderable` | `unsupported_schema_version`, `unsupported_characters`, `pdf_export_failed` |

`file_too_large` means the report is over 64 MiB, or the attestation or the bundle over 1 MiB.

Exit 5 means nothing says the report is wrong, the engine just cannot draw it: a schema newer than this binary reads, text outside the bundled DM Sans and JetBrains Mono fonts (Latin scripts, no emoji) listed on stderr, or a document the PDF writer refuses. Newlines and tabs in report strings are drawn as spaces. Characters are never replaced on an official document.

The command needs the `pdf` Cargo feature, on in the release binary and the image.

## Publishing to a Perf Sentinel Hub

A Hub with disclosures on (PerfSentinelHub 0.7.0 or later, `Hub:Disclosures:ImportKey` set) keeps a disclosure, renders its PDF with the engine it embeds, and hands it on through a restricted screen, expiring share links and signed webhooks. The last step of the job that produces and signs the report sends the three files as one multipart request:

```bash
curl --fail -H "X-API-Key: $HUB_DISCLOSURES_KEY" \
  -F report=@report.json \
  -F attestation=@attestation.intoto.jsonl \
  -F bundle=@bundle.sig \
  https://hub.example.com/api/import/disclosures
```

The Hub runs `disclose-pdf` before it keeps anything. A report whose hash does not recompute, or whose attestation names another hash, is refused with 422 and the engine's reason. An unsigned report is accepted and shown as such, and publishing the signed copy of the same report later replaces it. The Hub never checks a signature: whoever receives the PDF runs `verify-hash` on it. See the Hub's `API.md` for the responses.

## Verifying a published disclosure

A third party verifies a published file with one command:

```bash
# Local mode: all three files already downloaded.
perf-sentinel verify-hash \
    --report report.json \
    --attestation attestation.intoto.jsonl \
    --bundle bundle.sig \
    --expected-identity release@example.fr \
    --expected-issuer https://accounts.google.com

# Remote mode: fetch the report and sidecars by HTTPS convention.
perf-sentinel verify-hash \
    --url https://example.fr/perf-sentinel-report.json \
    --expected-identity release@example.fr \
    --expected-issuer https://accounts.google.com
```

Both examples pass `--expected-identity` and `--expected-issuer` because
that is the safe default: without these flags `verify-hash` refuses to
invoke cosign and returns `Status::Fail` on the signature slot. See
[Identity verification](#identity-verification) below for the three
modes and their semantics. Reserve `--no-identity-check` for an
internal self-check before publication.

`verify-hash` chains four checks: deterministic content hash
recompute (pure Rust, always run), the attestation naming that
content hash (pure Rust, run when an attestation is supplied),
Sigstore signature over the attestation (`cosign verify-blob`), and
SLSA binary provenance (metadata summary plus a `gh attestation
verify` command pointing at the binary in
`integrity.binary_verification_url`). The signature covers the
attestation, not the report, so the second check is what ties a valid
signature to the figures in hand: an attestation that names another
content hash, typically after figures were edited and the hash
re-baked, makes the verdict UNTRUSTED. An attestation written by
perf-sentinel 0.27.0 or earlier pinned the report file bytes instead,
and stays bound only while those bytes are untouched. Once `integrity.signature` has been
patched in, it caps the verdict at PARTIAL.

Exit codes:

| Code | Meaning                                                                                                                                                                                           |
|------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `0`  | TRUSTED (content hash matched, the attestation names it, the signature over the attestation verified ok, and binary attestation not left unverified when the report carries attestation metadata) |
| `1`  | UNTRUSTED (a check returned a hard failure: hash mismatch, attestation naming other figures, signature invalid, attestation invalid, identity mismatch)                                           |
| `2`  | PARTIAL (no hard failure but at least one check could not complete: cosign absent, `gh` CLI absent, signature metadata absent, sidecars missing)                                                  |
| `3`  | INPUT_ERROR (report file unreadable, JSON invalid, missing `--report` or `--url`)                                                                                                                 |
| `4`  | NETWORK_ERROR (only `--url` mode: HTTP fetch failed, scheme rejected, body over the size cap)                                                                                                     |

A scripted `verify-hash && deploy` gate blocks on any non-zero code
and so still rejects PARTIAL, but a wrapper that distinguishes
PARTIAL (2) from UNTRUSTED (1) can tell a missing tool from a tamper
attempt.

When a report carries binary attestation metadata, `verify-hash` does
not skip that block silently: without `--verify-binary <path>` the
attestation stays unverified and the result caps at PARTIAL rather than
TRUSTED. Pass `--verify-binary <path>` to run `gh attestation verify`
against the producing binary (needs the `gh` CLI and network) so the
provenance is checked and exit `0` reflects it.

### Verifying a PDF

`verify-hash --report report.pdf` reads the report, the attestation and the bundle out of the PDF and runs the same checks, so a recipient who only holds the PDF can verify it. A copy re-saved by a tool that keeps attachments still verifies: the reader follows the PDF's own structure, not the layout the renderer wrote. Some viewers drop attachments when they save, macOS Preview among them: verify the original file. The check covers the attached report, not the printed pages: `verify-hash` says so, and the reader compares the printed figures with that report. `--attestation` and `--bundle` passed on the command line win over the attached files, here and in `--url` mode. `verify-hash --url` given the address of a PDF does the same and fetches no sidecar. A report whose schema is newer than the binary exits 5: upgrade perf-sentinel.

### Sidecar URL convention in `--url` mode

`verify-hash --url <REPORT_URL>` fetches three files from the same
directory, with **fixed filenames**:

```
https://example.fr/<report-filename>            (the report)
https://example.fr/attestation.intoto.jsonl     (in-toto statement sidecar)
https://example.fr/bundle.sig                   (cosign bundle sidecar)
```

The sidecar names are not derived from the report filename: they are
literally `attestation.intoto.jsonl` and `bundle.sig`. An operator
publishing a report must use these filenames at the same URL prefix
for `verify-hash --url` to find them automatically. A future
revision may surface the URLs in `integrity.signature.bundle_url`
so the convention becomes explicit per report, but that is not the
current behaviour.

### Identity verification

`verify-hash` requires the consumer to declare which identity should
have signed the report. Three modes:

- `--expected-identity <ID> --expected-issuer <URL>`: cosign verifies
  that the bundle was issued by exactly this OIDC identity. The
  values come from the auditor's prior knowledge of the publishing
  organization (the report itself declares them in
  `integrity.signature.signer_identity` / `.signer_issuer`, but
  treating those as authoritative is autosigning: any GitHub or
  Google account holder can publish a bundle claiming an identity).
- `--no-identity-check`: cosign verifies the cryptographic integrity
  without checking the identity. Useful for an internal self-check
  before publication, but logged as PARTIAL because the signer is not
  verified.
- Neither flag passed: `verify-hash` refuses to invoke cosign and
  returns `Status::Fail` on the signature slot. This is the safe
  default and forces an external consumer to declare intent.

### Binary build provenance

`integrity.binary_hash` is the SHA-256 of the Perf Sentinel binary
that produced the report. For an official disclosure, the value
should match an official release binary published on the project's
GitHub releases. Operators who build Perf Sentinel from source can
still produce official reports, but their `binary_hash` will not
match any published release. In that case
`integrity.binary_attestation` is absent (no SLSA provenance for a
local build) and `verify-hash` reports `[--] Binary attestation:
not provided`. The `integrity_level` is `signed`, not
`signed-with-attestation`. For maximum trust on a publication, use
the released binary matching the tag declared in
`integrity.binary_verification_url`.

## Computing a canonical content hash with `hash-bake` (0.7.2+)

For test fixtures and debugging workflows where you need a report whose `content_hash` already matches what Perf Sentinel itself would write, use `hash-bake`:

```bash
perf-sentinel hash-bake --report input.json --output output.json
```

`hash-bake` reads the report at `--report`, computes the canonical `content_hash` (applying the `POST_SIGN_FIELDS` blanking defined for the schema version), writes the hash into `integrity.content_hash`, and saves the result to `--output`. The same path as `--report` is allowed for in-place baking, with an atomic temp+rename that prevents partial corruption.

This command is intended for:

- Generating test fixtures with valid canonical hashes (for example, test suites that exercise `verify-hash` with TRUSTED or PARTIAL outcomes).
- Debugging a report whose hash has drifted from canonical (typically after manual edits to fields outside `POST_SIGN_FIELDS`).

Signed reports (where `integrity.signature` is non-null) are rejected by default. Re-baking does not invalidate the signature, since the canonical form blanks the signature anyway, but the operator should confirm intent with `--allow-signed`.

`hash-bake` does not modify `integrity.signature`, `integrity.binary_attestation` or `report_metadata.integrity_level`. It only writes `integrity.content_hash`.

Exit codes:

| Code | Meaning                                                                                     |
|------|---------------------------------------------------------------------------------------------|
| 0    | Hash baked, file written.                                                                   |
| 1    | Refused: report carries a signature and `--allow-signed` was not passed. No output written. |
| 3    | Input error: report unreadable, JSON invalid, or output cannot be written.                  |

## Common errors

- `Error: audited intent is reserved for a future release, use 'internal' or 'official' instead`: switch `--intent` to `internal` or `official`.
- `no archived reports fell within the requested period`: the archive contains lines but none match the `--from`/`--to` window. Check timestamps, especially around DST and timezone boundaries (the aggregator filters on UTC dates).
- `Error: report validation failed` followed by a bullet list: every line names the offending field. Fix in the org-config TOML or in the source archive.
- `strict_attribution` enabled and a window with no offenders: drop the flag or fix the per-service instrumentation that's hiding the offenders.

## Scope and limitations

The disclosure is a directional estimate with a `2x` multiplicative uncertainty bracket. It is not regulatory-grade and not suitable for CSRD or GHG Protocol Scope 3 reporting. See `docs/METHODOLOGY.md` for the full calculation chain and the calibration sources that can tighten the bracket (Scaphandre RAPL, cloud SPECpower, Electricity Maps).
