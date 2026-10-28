# SARIF output reference

Perf Sentinel emits SARIF v2.1.0 via `--format sarif` on `analyze` and `diff`.
This reference lists the fields populated per `result`. For the cross-format
acknowledgments workflow, see [ACKNOWLEDGMENTS.md](./ACKNOWLEDGMENTS.md).

## Per-result fields

| Field                                                                       | Source                            | Notes                                                                                                                                                                                                                |
|-----------------------------------------------------------------------------|-----------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `ruleId`                                                                    | `Finding.finding_type`            | Snake-case rule id (`n_plus_one_sql`, `redundant_http`, ...).                                                                                                                                                        |
| `level`                                                                     | `Finding.severity`                | `error` for critical, `warning` for warning, `note` for info.                                                                                                                                                        |
| `message.text`                                                              | composed from `Finding` fields    | Human-readable summary including occurrences and window.                                                                                                                                                             |
| `properties.service`                                                        | `Finding.service`                 | Always present. BiDi and invisible-format characters are stripped. |
| `properties.endpoint`                                                       | `Finding.source_endpoint`         | Always present, same stripping. |
| `properties.confidence`                                                     | `Finding.confidence`              | One of `local_batch`, `ci_batch`, `daemon_staging`, `daemon_production`. Read by perf-lint to boost or reduce IDE severity.                                                                                                         |
| `properties.signature`                                                      | `Finding.signature`               | Canonical signature, also exposed in `fingerprints`. Skipped when empty (legacy baselines pre-0.5.17).                                                                                                               |
| `properties.acknowledged`                                                   | `--show-acknowledged`             | `true` for acknowledged entries, omitted for normal findings.                                                                                                                                                        |
| `properties.acknowledgmentReason` / `acknowledgmentBy` / `acknowledgmentAt` | ack metadata                      | BiDi and invisible-format characters are stripped before emission (Trojan Source defense).                                                                                                                           |
| `rank`                                                                      | `Finding.confidence.sarif_rank()` | Integer 0 to 100. `local_batch=15`, `ci_batch=30`, `daemon_staging=60`, `daemon_production=90`.                                                                                                                                        |
| `locations[0].physicalLocation`                                             | `Finding.code_location`           | Physical source location, populated when the instrumentation emits a file path (`code.file.path`, or the legacy `code.filepath`). `region.startLine` comes from `code.line.number` (legacy `code.lineno`) and is omitted when the line is missing or 0. Hostile filepaths (absolute, traversal, BiDi, percent-encoded) are rejected at emission. The result has this one `location` only when the file path is usable, with `logicalLocations` repeating the service (`kind: "module"`) and endpoint (`kind: "function"`). Without it the result has no `locations`: a location without `physicalLocation` is valid SARIF, but the Jenkins Warnings NG parser rejects the whole file on it. GitHub code scanning shows only results with this location. |
| `properties.suggestedFix`                                                   | `Finding.suggested_fix`           | Framework-level recommendation, followed by `(see: <url>)` when a reference URL exists. Skipped when absent. Not emitted as SARIF `fixes[]`, whose schema requires `artifactChanges`, a source patch Perf Sentinel does not produce. |
| `fingerprints["perfsentinel/v1"]`                                           | `Finding.signature`               | SARIF v2.1.0 section 3.27.17 fingerprint. Used by GitHub Code Scanning and GitLab SAST for deduplication across runs. Skipped when the signature is empty.                                                           |

## Tool

`runs[].tool.driver` is always `{ name: "perf-sentinel", version: <CARGO_PKG_VERSION>, informationUri: "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel" }`. The `rules` array pre-declares all 12 finding types regardless of how many results are emitted, so SARIF consumers can show rule descriptions even on a clean run.

## Schema

`$schema` points to `https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json`.
