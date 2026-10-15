# Pushing daemon incidents to the Hub

## Why

The Hub is the fleet's long memory. Findings reach it two ways: the daemon pushes them
through `[daemon.hub_export]` within seconds of detection, and the Hub also polls every
daemon hourly as a safety net. Incidents only have the poll. A daemon keeps them in a FIFO
ring of `[daemon.incidents] max_retained` entries (200 by default) with no TTL, so an
incident recorded and then evicted between two polls never reaches the Hub. The daemon's
NDJSON archive keeps it, but nothing replays it. The Hub's `docs/OPERATIONS.md` names this
gap.

## Goal and success

An incident a daemon records reaches the Hub even if the daemon's ring evicts it before the
next poll, and so does every later state of it: its end when the alert resolves, and the
findings the settle pass adds. Success is a lab scenario where a daemon with
`max_retained = 2` records five incidents and the Hub holds all five, then holds the end of
one once its alert resolves.

## Decisions taken

- **Activation is implicit.** A daemon with both `[daemon.hub_export]` and
  `[daemon.incidents]` enabled pushes its incidents. No new configuration key.
- **The poll stays.** It remains the safety net for incidents, as it is for findings. The
  Hub already merges two copies of one incident.
- **The daemon still freezes and keeps incidents.** Freezing captures the findings live in
  the incident's window at that moment, from a ring that moves on within minutes; the Hub
  cannot rebuild that from per-signature first and last sightings. The ring is still needed
  for a daemon without a Hub, for the settle pass, which completes an incident about three
  TTLs after it, and for the poll. What changes is advice: with push on, `max_retained` can
  be lowered, since the ring no longer has to cover a poll interval.
- **A dedicated route, in the read format.** `POST /api/import/incidents`, carrying each
  incident exactly as `GET /api/incidents` serves it, so push and poll share one format and
  one parser. Rejected: an `incidents` field in the findings import (an older Hub would
  refuse the whole envelope, findings included, and the 2 MiB cap does not fit an incident),
  and a "new incident" notification the Hub would follow with a read (useless where the Hub
  cannot reach the daemon, which is where push matters).

## Engine (perf-sentinel)

### Feeding the exporter

`IncidentStore` already knows the three moments an incident changes: `record` (new),
`close` (the resolved end arrives) and the settle merge (frozen findings can only grow).
When the Hub exporter exists, each of these hands it a clone of the incident in its new
state. Freezing, the ring, the archive and the query API are unchanged.

### The pending incidents table

A second latest-value table in the exporter, keyed by incident id, beside the findings one:

- An incident already pending is replaced by the newer copy. Its revision increments, so a
  late acknowledgement of an older batch cannot remove the newer copy.
- It is bounded by the same `max_pending`. Evicting from it counts on
  `perf_sentinel_hub_export_incidents_dropped_total`.
- There is no hourly re-send, unlike findings: an incident changes rarely, and every change
  is sent.

### The send loop

The existing exporter task runs one round for findings, then one for incidents, with the
same backoff, jitter and response classification (2xx acknowledges, unretryable 4xx drops
and counts, 503 is busy, the rest retries).

- Batches hold at most 10 incidents and 4 MiB of JSON.
- An incident that alone exceeds 4 MiB is dropped and counted. The poll can still copy it.
- Body: `{"producer_version": "<CARGO_PKG_VERSION>", "incidents": [<Incident>...]}`, each
  incident serialized as the query API serves it.
- URL: the configured `endpoint` with its `/api/import/findings` suffix replaced by
  `/api/import/incidents`, plus the same `?source_id=`. Same `X-API-Key`.

### An older Hub

A 404 or 405 on the incidents route means the Hub predates it. The exporter stops sending
incidents until the daemon restarts: one `WARN`, the pending incidents table cleared and its
entries counted as dropped. Findings keep flowing.

### Shutdown

The graceful drain empties both tables within the existing 10 s budget.

### Metrics

Two new series, so no existing alert changes behaviour (adding a label to
`perf_sentinel_hub_export_dropped_total` would):

- `perf_sentinel_hub_export_incidents_pending` (gauge)
- `perf_sentinel_hub_export_incidents_dropped_total` (counter)

Both are label-free.

### Configuration

Nothing new. The incidents URL derives from `endpoint`, whose validation already requires the
`/api/import/findings` suffix.

## Hub (PerfSentinelHub)

### Route

`POST /api/import/incidents?source_id=<id>`, anonymous like the findings import and
protected the same way:

- the same query validation (one `source_id`, valid UTF-8);
- `X-API-Key` compared to the source's `ImportApiKey` in constant time, daemon sources only,
  401 for an unknown source or a wrong key;
- the import gate shared with the findings import (4 concurrent), else 503 with
  `Retry-After: 1`;
- a 4 MiB body cap, else 413.

### Parsing

- The envelope `{producer_version, incidents}` refuses duplicate properties.
  `producer_version` is a non-empty string of at most 64 characters.
- 1 to 10 incidents: more is 413, none is 400.
- Each incident goes through `IncidentParser`, the poll's parser. An invalid one is rejected
  alone. When none is accepted the answer is 400.

### Writing

- A variant of the incidents upsert that waits at most 5 s for the write gate, then answers
  503 with `Retry-After: 1`, as the findings import does. The poll's upsert keeps its
  unbounded wait.
- The merge rule is unchanged: the richer copy (by frozen finding count) wins, and an end is
  kept once seen. Push and poll therefore converge whatever order they arrive in.
- The push upserts `source_imports.last_import_ms`. It does not touch `incident_reads`,
  which stays the poll's read state, nor reachability.

### Response and metrics

- 200 `{accepted, rejected}` after the commit.
- `perf_sentinel_hub_incident_import_rejected_total{reason}`, with the findings import's
  closed set of reasons. The existing `perf_sentinel_hub_import_rejected_total` keeps its
  labels.

### Chart

Nothing new: a source already carries `importSecretName` and `importSecretKey`.

## Versions and release order

- perf-sentinel 0.29.0 (new behaviour and metrics, no public struct change), chart in
  lockstep.
- PerfSentinelHub 0.5.0 (new API route), after 0.4.2.
- Either order works thanks to the 404 handling. Ship the Hub first so a daemon's first
  push lands, then repin the Hub's embedded engine to 0.29.0.

## Documentation

Every English page changes with its French mirror in the same commit.

- Engine: `docs/CONFIGURATION.md` `[daemon.hub_export]` (incidents travel too, the URL
  derivation, the older-Hub behaviour), `docs/METRICS.md`, `docs/RUNBOOK.md` (lower
  `max_retained` when pushing), `docs/HELM-DEPLOYMENT.md`, the chart's `values.yaml`
  comment, and a note in `docs/design/06-INGESTION-AND-DAEMON.md`. CHANGELOG.
- Hub: `docs/API.md` (the route), `docs/OPERATIONS.md` (the gap closes when push is on),
  `docs/ARCHITECTURE.md` (push and poll). CHANGELOG. The `push-and-poll` diagram is drawn by
  hand and is left to its owner, with a note of what it should now show.

## Testing

- Engine unit tests on the incidents table: a new revision on close and on settle merge, a
  pending incident replaced, a late acknowledgement leaving the newer copy, an oversized
  incident dropped and counted, the 404 sleep with findings still sent, the shutdown drain.
- Engine integration test: a fake Hub receiving incidents, checking body shape, header and
  URL.
- Hub tests: key, body size, incident count, duplicate properties, 503 when the write gate
  does not open, and push then poll and poll then push converging on the same stored row.
- Lab: a new `hub-incidents-push` scenario. The daemon runs with `max_retained = 2` and
  `[daemon.hub_export]` on, the Hub with a long `PollInterval`. Five alerts are posted, and
  the Hub must hold five incidents while the daemon's ring holds two. One alert then
  resolves, and the Hub must carry its end.

## Out of scope

- Replaying the daemon's NDJSON incidents archive into the Hub.
- Pushing acknowledgments or the daemon's status.
- Any change to freezing, the ring's default size, or the poll.
