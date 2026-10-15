# A disclosure archive that holds a period

Status: approved on 2026-10-14. The implementation plan is `docs/superpowers/plans/2026-10-14-disclosure-daily-rollups.md`.

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

What the default 1.3 GB holds at these rates, and what a quarter (92 days, 529,920 lines) needs. Corrected on 2026-10-15: the first version counted 240 lines per day instead of per hour, so its quarter columns were 24 times too small. The conclusion does not change.

| Profile | Default archive holds | Quarter, current lines | Quarter, projected | Quarter, projected gzip | Quarter, daily rollups |
|---|---|---|---|---|---|
| Moderate | about 16 hours | about 175 GiB | about 4.3 GiB | about 0.5 GiB | under 1 MB |
| Sustained | about 6 hours | about 445 GiB | about 15 GiB | about 1.4 GiB | under 3 MB |

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

**B. Project and compress the window line.** Archive the projected line instead of the full `Report`, and gzip rotated files. 30 to 40 times smaller, roughly 300 times with gzip. A quarter still needs 0.5 to 1.4 GiB gzipped, 4 to 15 GiB uncompressed. It keeps the window-level granularity and the per-file chain. Cost: the archive is no longer a `Report`, so anything else reading it (none in the repository besides `disclose` and its TUI preview) must follow, and the chain still resets per file, so pruning stays undetectable.

**C. Daily rollups, recommended.** The daemon keeps an in-memory accumulator for the current UTC day, the same partial state the aggregator builds, and appends one rollup line per day to a monthly file beside the window archive. A quarter is 92 lines, a few MB at most. Details below.

## Recommended design: daily rollups

**Shared fold.** Split the measuring half of the aggregator's `Builder` into a serializable `PeriodState` with `fold_window(&Report, ts)` and `merge(PeriodState)`. It stays in `aggregator.rs`, so the fold keeps its history and SonarCloud does not count it as new code. The line format and the day accumulator go to a new `report/periodic/rollup.rs`. `aggregate_from_paths` becomes "fold windows into a state, merge rollups into the state, finalize". A property test asserts `finalize(fold(windows)) == finalize(merge(rollup_per_day(windows)))` on generated archives, and the existing `disclose` scenarios keep their figures.

**Daemon writer.** The archive task already receives every window (`archive::try_send`). It folds each window into the day's state, keyed on the window's own timestamp. At the first window of a new UTC day it writes the closed day as one line `{format, ts, day, last_ts, state, prev, seq, drops, hash}`, `ts` being the day's first window. Graceful shutdown writes the day so far the same way. A day can therefore own several lines, which merge, since the state is additive. Every 20 windows it also overwrites a partial snapshot atomically. At start, a snapshot the last line does not already cover is appended once, so a crash loses at most 20 windows, never the day and never twice.

**One chain, never reset.** Rollup lines chain across their monthly files and across restarts: the writer resumes from the last line of the newest file. A pruned or deleted day then shows as a `seq` gap or a `prev` mismatch, which today's per-file chain cannot reveal. `disclose` reports rollup days verified and gaps under `integrity.trace_integrity_chain` as it does for windows.

**Retention by age.** Rollups go to `<stem>-daily-YYYY-MM.ndjson` beside `[daemon.archive] path`. A monthly file is deleted whole once its last day is more than `rollup_retention_days` (default 400, a little over a year) before the newest closed day. The reader adopts the first surviving line as the chain's start, so retention is not a break, and the 0.28.0 reach check still refuses a period the rollups no longer reach.

**Raw windows become optional.** `[daemon.archive] windows = false` stops the window lines and keeps the rollups. The default stays `true`. Rollups have no switch and no path of their own: they are always written beside `path`, so the chart's PVC holds both with no chart change.

**Mixed inputs.** `disclose --input` accepts window files and rollup files together. A day present as a rollup wins over that day's windows, so a period that spans the upgrade aggregates once. The 0.28.0 reach check uses the oldest day across both.

**Horizon signal.** `/api/status` gains `disclosure_archive: {oldest_day, days, last_closed_day}`, backed by three label-free gauges, `perf_sentinel_archive_oldest_day_timestamp_seconds`, `perf_sentinel_archive_rollup_days` and `perf_sentinel_archive_last_closed_day_timestamp_seconds`. An operator sees the horizon shrink before a quarter closes, and the Hub can later show it per source.

**Schema.** The published `PeriodicReport` does not change: rollups are an input format. `report_metadata.generated_by` keeps its values. The rollup line format is versioned (`"format": "perf-sentinel-rollup/v1"`) so the fold can evolve.

## Slicing

1. Core: `PeriodState` with fold, merge, serde, and the equivalence property test. `aggregate_from_paths` rebuilt on it, figures unchanged.
2. Daemon: day accumulator, rollup writer with its never-reset chain, partial snapshot and resume, retention by age, status field and gauge. Feature `daemon`.
3. `disclose` reads rollups and mixed inputs, reports rollup chain verification, TUI preview included.
4. `docs/REPORTING.md`, `docs/CONFIGURATION.md`, `docs/design/08-PERIODIC-DISCLOSURE.md`, French mirrors, and a lab scenario that runs a daemon across a simulated day boundary and compares `disclose` over windows and over rollups.

Option B's projection can ship first as a smaller step if the rollups take longer, since it reuses the window format's reading path.

## Decisions

Approved on 2026-10-14.

- `rollup_retention_days` defaults to 400.
- Raw window archiving stays on by default once rollups exist.
- The chart's persistence default does not change.
