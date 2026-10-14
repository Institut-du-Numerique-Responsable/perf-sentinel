# A disclosure archive that holds a period

Status: proposal, awaiting approval. No product code is written yet.

## Problem

`disclose` builds a periodic report from the daemon's `[daemon.archive]`, one NDJSON line per scoring window. A disclosure period is typically a calendar quarter. The archive cannot hold one.

- The daemon archives a full `Report` on every processing tick that saw traffic, every `trace_ttl_ms / 2`, 15 seconds by default (`daemon/mod.rs` `evict_ms`, the ticker in `daemon/event_loop.rs`).
- Rotation prunes by file count, not by age: `max_files` (12) files of `max_size_mb` (100 MiB), about 1.3 GB (`daemon/archive.rs` `rotate` and `prune`).
- The hash chain restarts from the seed in every rotated file (`prev` and `seq` reset on rotation), so a pruned file leaves no trace that `disclose` could detect.
- Until 0.28.0, the only signal was `temporal_coverage`, a warning. 0.28.0 makes an official `disclose` refuse a period whose first day precedes the oldest archived window (`archive_reaches_period_start`). That stops a silent wrong report. It does not make the archive hold the period.

## Measurements

Measured on 2026-10-14 against a local 0.28.0 daemon with default settings and `[daemon.archive]` on, fed by the simulation lab's `tracegen` over OTLP/HTTP for five minutes per profile.

| Profile | Lines per hour | Median line | Projected line | Projected, gzip |
|---|---|---|---|---|
| 10 services, 20 traces/s | 240 | 346 KiB | 8.5 KiB | 0.9 KiB |
| 50 services, 50 traces/s | 240 | 878 KiB | 29 KiB | 2.8 KiB |

What fills a line, 50-service profile: `findings` 841 KB (940 findings, one per trace occurrence, each carrying its trace id, timestamps, suggestion and green impact), `green_summary` 47 KB of which `top_offenders` is 37 KB (234 entries), `per_endpoint_io_ops` 23 KB. Everything else is under 1 KB.

"Projected" keeps only what the aggregator reads: `green_summary` without `top_offenders`, I/O ops folded per service, findings folded per `(service, type)` into a count and occurrences by service, `disclosure_waste`, `binary_version`, `analysis.traces_analyzed`.

What the default 1.3 GB holds at these rates, and what a quarter (92 days, 22,080 lines) needs:

| Profile | Default archive holds | Quarter, current lines | Quarter, projected | Quarter, projected gzip | Quarter, daily rollups |
|---|---|---|---|---|---|
| Moderate | about 16 hours | 7.4 GB | 190 MB | 20 MB | under 1 MB |
| Sustained | about 6 hours | 19 GB | 640 MB | 62 MB | under 3 MB |

tracegen exercises the anti-pattern catalogue on purpose, so its finding rate is high. A production service with a few hot N+1 endpoints lands between the two profiles. The orders of magnitude, not the exact figures, drive the decision.

## What the aggregator needs

Every field `aggregate_from_paths` accumulates (`Builder` in `report/periodic/aggregator.rs`) merges by a commutative, associative operation:

- sums: requests, I/O ops, energy, carbon terms, windows, runtime and fallback windows, avoidable figures per tier, database and messaging waste.
- set unions: energy models, binary versions, carbon methodologies, scoring coefficients, per-service energy models.
- minimum and maximum: `first_seen` and `last_seen` per `(service, pattern)`, the waste tiers' N+1 threshold (already reconciled by maximum).
- logical or: `calibration_applied`, `transport_coefficient_uncertain`.
- sum and count pairs: the per-service measured ratio.
- per-service accumulators, themselves sums and set unions.
- the set of observed days.

Only the chain counters and the drop-counter baseline describe how archive lines were read, not what they measured. So a day's windows can be folded into one partial state that merges into the period's state with exactly the result the window-by-window fold gives.

## Options

**A. Fail loudly.** Shipped in 0.28.0. Necessary, not sufficient.

**B. Project and compress the window line.** Archive the projected line instead of the full `Report`, and gzip rotated files. 30 to 40 times smaller, roughly 300 times with gzip. A quarter fits in 20 to 60 MB. It keeps the window-level granularity and the per-file chain. Cost: the archive is no longer a `Report`, so anything else reading it (none in the repository besides `disclose` and its TUI preview) must follow, and the chain still resets per file, so pruning stays undetectable.

**C. Daily rollups, recommended.** The daemon keeps an in-memory accumulator for the current UTC day, the same partial state the aggregator builds, and appends one rollup line per day to `rollups.ndjson`. A quarter is 92 lines, a few MB at most. Details below.

## Recommended design: daily rollups

**Shared fold.** Move the window fold out of `aggregator.rs` into a core `report/periodic/rollup.rs`: a serializable `PeriodState` with `fold_window(&Report, ts)` and `merge(&PeriodState)`. `aggregate_from_paths` becomes "fold windows into a state, merge rollups into the state, finalize". A property test asserts `finalize(fold(windows)) == finalize(merge(rollup_per_day(windows)))` on generated archives, and the existing `disclose` scenarios keep their figures.

**Daemon writer.** The archive task already receives every window (`archive::try_send`). It folds each window into the day's state and, at the first window of a new UTC day and on graceful shutdown, writes the closed day as one line `{day, first_window_ts, last_window_ts, windows, drops, state, prev, seq, hash}`. Every few minutes it also overwrites `rollup-partial.json` atomically (temp file and rename, as the ack store does) and reloads it at start, so a crash loses minutes, not the day.

**One chain, never reset.** Rollup lines chain across files and restarts: `resume_chain` reads the last line at start, rotation keeps `prev` and `seq`. A pruned or deleted day then shows as a `seq` gap or a `prev` mismatch, which today's per-file chain cannot reveal. `disclose` reports rollup days verified and gaps under `integrity.trace_integrity_chain` as it does for windows.

**Retention by age.** `[daemon.archive] rollup_retention_days`, default 400 (a year plus a quarter). A daily line is small enough that count-based rotation is not needed.

**Raw windows become optional.** `[daemon.archive] path` keeps writing window lines for drill-down when set. Rollups need only `[daemon.archive] rollup_path`. The chart, with persistence on, writes both under the PVC and can lower the default window retention.

**Mixed inputs.** `disclose --input` accepts window files and rollup files together. A day present as a rollup wins over that day's windows, so a period that spans the upgrade aggregates once. The 0.28.0 reach check uses the oldest day across both.

**Horizon signal.** `/api/status` gains `disclosure_archive: {oldest_day, days, last_closed_day}` and `/metrics` a bounded gauge `perf_sentinel_archive_oldest_day_timestamp_seconds`, so an operator sees the horizon shrink before a quarter closes, and the Hub can show it per source.

**Schema.** The published `PeriodicReport` does not change: rollups are an input format. `report_metadata.generated_by` keeps its values. The rollup line format is versioned (`"format": "perf-sentinel-rollup/v1"`) so the fold can evolve.

## Slicing

1. Core: `PeriodState` with fold, merge, serde, and the equivalence property test. `aggregate_from_paths` rebuilt on it, figures unchanged.
2. Daemon: day accumulator, rollup writer with its never-reset chain, partial snapshot and resume, retention by age, status field and gauge. Feature `daemon`.
3. `disclose` reads rollups and mixed inputs, reports rollup chain verification, TUI preview included.
4. Chart defaults and comments, `docs/REPORTING.md`, `docs/CONFIGURATION.md`, `docs/design/08-PERIODIC-DISCLOSURE.md`, French mirrors, and a lab scenario that runs a daemon across a simulated day boundary and compares `disclose` over windows and over rollups.

Option B's projection can ship first as a smaller step if the rollups take longer, since it reuses the window format's reading path.

## Questions for the owner

- Default `rollup_retention_days`: 400, or tied to the declared `[reporting] disclose_period`?
- Keep raw window archiving on by default once rollups exist, or make it opt-in?
- Chart: turn persistence on by default for the archive, or keep it opt-in with a louder warning?
