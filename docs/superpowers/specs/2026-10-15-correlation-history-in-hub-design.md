# Correlation history in the Hub

## Why

A daemon with `[daemon.correlation]` on reports pairs of findings that keep following each
other: every time the N+1 in `order-svc` fires, a pool saturation in `payment-svc` follows
within seconds. The correlator keeps a pair only while its counters cover the rolling window
(`window_minutes`, 10 by default) and drops the oldest pairs past `max_tracked_pairs`. Nothing
else keeps them: `[daemon.hub_export]` sends findings and incidents, the Hub never reads
`/api/correlations`, and the per-window archive writes `correlations: vec![]`. A correlation
that was active this morning is gone by noon, so it cannot be set against an incident or
followed over a week.

## Goal and success

The Hub keeps the periods during which each pair was active, as episodes, and shows them in
three places: an Incidents-like Correlations screen in the launcher, a block in an incident's
detail listing the episodes active during it, and a Grafana panel. Success is a lab scenario
where a daemon sees one pair active twice, separated by a silence longer than the episode gap,
and the Hub holds two episodes for that pair, one of them overlapping an incident posted
during it.

## Decisions taken

- **Episodes, not a series.** For one pair the Hub keeps one row per period of continuous
  activity, with its peaks. It answers "was it active during this incident" without a false
  positive and draws a timeline with its gaps, for a few rows per pair per day. A point per
  export cycle (a confidence curve) was rejected: the confidence is a rolling estimate, noisy by
  construction, and it would cost about a hundred times the storage.
- **The daemon cuts the episodes.** Only it knows whether a pair is still co-occurring. A Hub
  that rebuilt episodes from snapshots would close them wrongly on any push outage. The Hub
  stores what it is pushed, as for incidents.
- **Activity is read from `last_seen`, not from presence.** A pair stays above the reporting
  thresholds while its counters cover the window, which can be 7 days. An episode therefore
  follows the pair's last co-occurrence: it is active while `last_seen` advances and closes once
  `last_seen` has stood still for the episode gap, `min(window_ms, 30 min)`, 10 minutes by
  default.
- **Push only, no poll fallback.** The daemon gains no read route for episodes. Without
  `[daemon.hub_export]` the Hub has no correlation history.
- **Implicit activation.** `[daemon.hub_export]` plus `[daemon.correlation] enabled = true`.
  No new configuration key.
- **No Findings screen in the launcher.** It would duplicate an analysis report for the detail
  and the Grafana findings history for the timeline. An episode links to an analysis of its
  window instead.

## Engine (perf-sentinel)

### Pair and episode identity

- `pair_key`: 32 hex characters of a SHA-256 over the canonical source and target endpoints,
  each `(finding_type, service, template, grouping_key)`. Stable across restarts and daemons.
- Episode `id`: 32 hex characters of a SHA-256 over `pair_key` and `started_at_ms`. Stable
  across the pushes of one episode.

### The episode tracker

It lives in the Hub exporter, beside the findings and incident tables. The correlator does not
change.

- Each correlations round reads `active_correlations()` from the shared correlator, as
  `/api/correlations` does.
- **Open.** A pair in the snapshot with no open episode, whose `last_seen` is newer than any
  episode the tracker closed for it. `started_at_ms` is the correlator's `first_seen` for a
  pair the tracker has never seen, and the first `last_seen` observed after the gap otherwise,
  precise to one round.
- **Update.** An open episode whose pair's `last_seen` advanced. It keeps `last_seen_ms`, the
  peaks of confidence and co-occurrences, the latest confidence, co-occurrences and median
  lag, and the two sample trace ids.
- **Close.** An open episode whose `last_seen` has not advanced for the episode gap, or whose
  pair left the snapshot for that long. `ended_at_ms` is its last `last_seen`.
- **Graceful shutdown** closes every open episode at its `last_seen` before the drain, so a
  restart does not leave episodes open in the Hub.
- Memory: at most one open episode per tracked pair, so bounded by `max_tracked_pairs`.

### The pending episodes table

A third latest-value table keyed by episode id, with the incident table's revision logic.

- An episode is queued when it opens, when a peak rises, when `last_seen` advanced by at least
  60 seconds since its last queued copy, and when it closes. Round-to-round drift of the
  rolling values is not sent.
- Bounded by `max_pending`. Evicting counts on
  `perf_sentinel_hub_export_correlations_dropped_total`.

### The send loop

A third half in the exporter task, on its own backoff and pace, sharing the classification,
the jitter, the busy handling and the POST helper.

- Batches of at most 100 episodes and 1 MiB of JSON. An episode alone over 1 MiB is dropped
  and counted.
- Body: `{"producer_version": "<CARGO_PKG_VERSION>", "episodes": [<Episode>...]}`.
- URL: `endpoint` with its `/api/import/findings` suffix replaced by
  `/api/import/correlations`, plus the same `?source_id=`, same `X-API-Key`.
- A 404 or 405 stops correlation pushes until restart: one `WARN`, the table cleared and
  counted as dropped. Findings and incidents keep flowing.
- The graceful drain empties the three tables within the existing 10 s budget.

### Episode wire format

```json
{
  "id": "…32 hex…",
  "pair_key": "…32 hex…",
  "source": {"finding_type": "n_plus_one_sql", "service": "order-svc",
             "template": "SELECT … WHERE order_id = ?", "grouping_key": null},
  "target": {"finding_type": "pool_saturation", "service": "payment-svc",
             "template": "…", "grouping_key": null},
  "started_at_ms": 0, "last_seen_ms": 0, "ended_at_ms": null,
  "peak_confidence": 0.92, "peak_co_occurrences": 41,
  "last_confidence": 0.88, "last_co_occurrences": 37, "median_lag_ms": 1200.0,
  "sample_trace_id": "…", "source_sample_trace_id": "…",
  "gap_ms": 600000
}
```

`gap_ms` lets the Hub tell an open episode from one whose daemon died before closing it.

### Metrics

- `perf_sentinel_hub_export_correlations_pending` (gauge)
- `perf_sentinel_hub_export_correlations_dropped_total` (counter)

Both label-free.

## Hub (PerfSentinelHub)

### Storage

A schema migration adds `correlation_episodes`:

- key `(id, source_id)`, and `pair_key`.
- each side in columns, for filtering: `source_type`, `source_service`, `source_template`,
  `source_grouping`, and the same for `target_`.
- `started_at_ms`, `last_seen_ms`, `ended_at_ms` (null while open), `gap_ms`.
- `peak_confidence`, `peak_co_occurrences`, `last_confidence`, `last_co_occurrences`,
  `median_lag_ms`, `sample_trace_id`, `source_sample_trace_id`.
- `first_seen_ms`, `last_seen_hub_ms` on the Hub clock.
- indexes on `(started_at_ms DESC)`, `(source_id, pair_key, started_at_ms)` and
  `(last_seen_ms)`.

Merge rule, so that two copies converge in either order: `started_at` takes the minimum,
`last_seen` and the peaks the maximum, the latest values come from the copy with the newer
`last_seen`, and `ended_at` is kept once seen.

The daily purge deletes episodes whose `last_seen` is older than `Hub:Retention` (180 days by
default). No new option.

### Import route

`POST /api/import/correlations?source_id=<id>`, anonymous and admitted as the two other imports:
the same query validation, `X-API-Key` against a daemon source's `ImportApiKey` in constant
time (401 otherwise), the shared import gate (503 and `Retry-After: 1`), a write that waits at
most 5 s, and:

- a 1 MiB body cap (413), 1 to 100 episodes (413 above, 400 for none), duplicate properties
  refused.
- each episode validated alone (ids of 32 hex, known finding types, `started_at <= last_seen`,
  `ended_at` absent or `>= started_at`, confidences in `[0, 1]`), an invalid one rejected alone,
  400 when none is accepted.
- 200 `{accepted, rejected}` after the commit.
- refusals on `perf_sentinel_hub_correlation_import_rejected_total{reason}`, on the findings
  import's closed set of reasons.
- `source_imports.last_import_ms` upserted, reachability untouched.

### Read route

`GET /api/correlations`, with the same access scoping as `GET /api/incidents`: a member sees the
episodes of the sources and services they may read.

- Filters: `source_id`, `service` (either side), `pair_key`, `from` and `to` (episodes
  overlapping the span, open ones included), `active` (open only), `offset`, `limit` (200 by
  default, 1000 at most).
- Order: `started_at` descending.
- An open episode whose `last_seen` is older than `gap_ms` plus 15 minutes is served as ended at
  its `last_seen`, with `"ended_by": "stale"`. The row is not rewritten.

### Launcher

- **A Correlations tab** after Incidents, `#/correlations`: "Findings that keep following each
  other", one sentence and a "?". Filters: the existing range picker (last 24 hours by default),
  service, source daemon, and "active now only".
- **One row per pair**, grouped client-side from the episodes: both sides with their type pills
  and service, the number of episodes in the range, the last episode (`active` when open), the
  peak confidence, the median lag, the source daemon. When the read was cut at `limit`, a line
  says so.
- **A pair unfolds** into a timeline of its episodes over the range, with the same daemon's
  incidents in red, and the list of episodes: span, duration, peaks, the incidents it overlaps
  (links), "Analyse this window" (New analysis prefilled with the episode's window, one minute
  on each side, and the source side's service, as the incident button does), and "Explain the
  last pair" (both sample traces).
- **In an incident's detail**, a block "Correlations active during this incident": the episodes
  of the same source overlapping `[window_from_ms, ended_at_ms ?? window_to_ms]`, each with a
  link to all its pair's episodes. Empty states: "No correlation was active in this window." and,
  for a daemon that pushed none, "<Source> pushes no correlations, so none can show here."
- On a phone, the tables scroll horizontally, as the incident tables do.
- Screenshots and GIFs regenerated with `npm run demo`, and the demo fixtures gain episodes.

## Grafana

A panel in the existing `examples/grafana-findings-dashboard.json`, in its "History (Hub)" row:

- a State timeline, one line per pair, one segment per episode, from Infinity on
  `GET /api/correlations?from=${__from}&to=${__to}` with the dashboard's `service` and `source`
  variables.
- a table under it: pair, episodes, peak confidence, last episode, a link to the Hub's
  Correlations screen filtered on the pair.
- the Hub's incidents as annotations on the same range.

The compatibility badge's Hub dependency moves to the Hub version that serves the route. The
daemon floor does not move. The French variants are regenerated with
`scripts/translate-dashboards-fr.py`. Checked on the local Grafana bench against a fed Hub.

## Versions and release order

- perf-sentinel 0.30.0 (new behaviour and metrics), chart in lockstep.
- PerfSentinelHub 0.6.0 (new routes, table, screen).
- Ship the Hub first: an older Hub answers the new route with 404 (correlation pushes stop) or
  401 with sign-in on (retried and logged). Then repin the Hub's engine to 0.30.0.

## Documentation

Every English page changes with its French mirror in the same commit.

- Engine: `docs/CONFIGURATION.md` (`[daemon.hub_export]` and `[daemon.correlation]`: what
  travels, the episode gap), `docs/METRICS.md`, the chart's `values.yaml` comment, CHANGELOG.
- Hub: `docs/API.md` (both routes), `docs/OPERATIONS.md`, `docs/ARCHITECTURE.md`, the security
  route inventory, CHANGELOG, the launcher screenshots.

## Testing

- Engine unit tests on the tracker: open, update, close after the gap, reopen as a new episode
  with its own start, a pair leaving the snapshot, the long-window case (pair still above the
  thresholds while `last_seen` stands still), close on graceful shutdown, the 60 s throttle, the
  peak trigger. On the table and send loop: revision, oversized episode, 404 stop with findings
  and incidents still sent, drain.
- Engine integration: a fake Hub checking body shape, header and URL.
- Hub: parser (each validation), merge in both orders, admission (key, size, count, duplicates,
  gate), read filters and overlap, access scoping, the stale rule, the purge.
- Launcher: Playwright on the screen, the unfolded pair, the incident block and both empty
  states, light and dark, 390 px.
- Lab: a `hub-correlations-push` scenario. A daemon with `[daemon.correlation]` on and a short
  window, the correlated fault fired, a silence past the gap, the fault fired again with an
  incident posted during it. The Hub holds two episodes for the pair, the second overlapping the
  incident, and `GET /api/correlations?active=true` lists only the second while it runs.

## Out of scope

- Acknowledging a correlation.
- A Findings screen in the launcher.
- Filling `correlations` in the daemon's NDJSON archive.
- A confidence series per episode.
